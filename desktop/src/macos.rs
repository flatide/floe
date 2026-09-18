//! The only native FFI boundary. All AppKit/WebKit objects stay on the main
//! thread; delegates and completion blocks are retained through their use.
use crate::service::Service;
use crate::transfers::{self, PendingFile};
use block2::{DynBlock, RcBlock};
use floe_app::embedded::{navigation_allowed, validate_ready, Ready, Session};
use floe_app_core::{Error, Result};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly, Message};
use objc2_app_kit::*;
use objc2_foundation::*;
use objc2_web_kit::*;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::BTreeMap;
use std::path::Path;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

const CLOSE_SCRIPT: &str =
    "(()=>{const b=document.getElementById('logout');if(b&&!b.disabled){b.click();return 'opened';}return 'unavailable';})()";

struct Download {
    object: Retained<WKDownload>,
    file: Option<PendingFile>,
}
struct TransferView {
    web: Retained<WKWebView>,
    url: String,
    started: bool,
    downloading: bool,
    start: Instant,
}

struct State {
    service: RefCell<Service>,
    origin: OnceCell<String>,
    window: OnceCell<Retained<NSWindow>>,
    web: OnceCell<Retained<WKWebView>>,
    failure: RefCell<Option<&'static str>>,
    panel_open: Cell<bool>,
    downloads: RefCell<BTreeMap<usize, Download>>,
    transfer_view: RefCell<Option<TransferView>>,
    recovery_probe: Cell<bool>,
    recovering: Cell<bool>,
    probe_next: Cell<Instant>,
    probe_count: Cell<u8>,
    smoke: bool,
    smoke_step: Cell<u8>,
    evaluating: Cell<bool>,
    smoke_probe: RefCell<String>,
    start: Instant,
}

define_class!(
    // SAFETY: NSObject has no subclass invariants; no custom Drop. Every
    // selector below matches the generated framework protocol signature.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = State]
    struct Host;

    unsafe impl NSObjectProtocol for Host {}
    impl Host {
        #[unsafe(method(recoverView:))]
        fn recover_view(&self, _sender: Option<&AnyObject>) { self.recover(); }
        #[unsafe(method(forceEndSession:))]
        fn force_end_session(&self, _sender: Option<&AnyObject>) { self.force_close(); }
    }
    unsafe impl NSApplicationDelegate for Host {
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            self.request_close();
            // Dock Quit / system termination must not bypass draft confirmation.
            NSApplicationTerminateReply::TerminateCancel
        }
    }
    unsafe impl NSWindowDelegate for Host {
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, _sender: &NSWindow) -> bool {
            self.request_close();
            false
        }
    }
    unsafe impl WKNavigationDelegate for Host {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn navigation(
            &self,
            web: &WKWebView,
            action: &WKNavigationAction,
            decision: &DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            if self.is_transfer_view(web) {
                let mut slot = self.ivars().transfer_view.borrow_mut();
                let transfer = slot.as_mut().unwrap();
                let request = unsafe { action.request() };
                let allow = !transfer.started && self.allowed_download(&request)
                    && request.HTTPMethod().is_some_and(|m| m.to_string() == "POST")
                    && request.URL().and_then(|u| u.absoluteString()).is_some_and(|u| u.to_string() == transfer.url);
                transfer.started = true;
                drop(slot);
                decision.call((if allow { WKNavigationActionPolicy::Allow } else { WKNavigationActionPolicy::Cancel },));
                if !allow { self.clear_transfer_view(); }
                return;
            }
            // SAFETY: Callback arguments are live WebKit objects, only queried
            // on this main-thread delegate; completion is called exactly once.
            let download = unsafe {
                action.sourceFrame().is_some_and(|f| self.owned_frame(&f)) && self.allowed_download(&action.request())
            };
            if download {
                // Action→Download drops form POST bodies on WebKit. A bounded
                // offscreen context keeps the original navigation until its
                // response, never a reconstructed request or credential bridge.
                let post = unsafe { action.request().HTTPMethod().is_some_and(|m| m.to_string() == "POST") };
                if self.ivars().panel_open.get() || !self.ivars().downloads.borrow().is_empty()
                    || self.ivars().transfer_view.borrow().is_some()
                    || (post && unsafe { action.targetFrame().is_some() }) {
                    decision.call((WKNavigationActionPolicy::Cancel,));
                    self.status("Download refused: finish the current dialog/download first");
                    return;
                }
                decision.call((if post { WKNavigationActionPolicy::Allow } else { WKNavigationActionPolicy::Download },));
                return;
            }
            let allow = unsafe {
                !action.shouldPerformDownload()
                    && action.targetFrame().is_some_and(|f| f.isMainFrame())
                    && action
                        .request()
                        .URL()
                        .and_then(|u| u.absoluteString())
                        .is_some_and(|u| {
                            self.ivars()
                                .origin
                                .get()
                                .is_some_and(|origin| navigation_allowed(origin, &u.to_string()))
                        })
            };
            decision.call((if allow {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
        }
        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn response(
            &self,
            web: &WKWebView,
            response: &WKNavigationResponse,
            decision: &DynBlock<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            if self.is_transfer_view(web) {
                let valid = self.transfer_response(web, response);
                if valid { self.ivars().transfer_view.borrow_mut().as_mut().unwrap().downloading = true; }
                decision.call((if valid { WKNavigationResponsePolicy::Download } else { WKNavigationResponsePolicy::Cancel },));
                if !valid {
                    let code = unsafe { response.response() }.downcast_ref::<NSHTTPURLResponse>().map(|r| r.statusCode());
                    self.status(&format!("Download refused: response {code:?} (no file saved)"));
                    self.clear_transfer_view();
                }
                return;
            }
            // Download actions are handled above; unexpected attachments never
            // navigate the UI or silently save a server error response.
            let allow = unsafe { response.canShowMIMEType() };
            decision.call((if allow {
                WKNavigationResponsePolicy::Allow
            } else {
                WKNavigationResponsePolicy::Cancel
            },));
        }
        #[unsafe(method(webView:navigationAction:didBecomeDownload:))]
        fn became_download(&self, _web: &WKWebView, action: &WKNavigationAction, download: &WKDownload) {
            let allow = unsafe { action.sourceFrame().is_some_and(|f| self.owned_frame(&f)) && self.allowed_download(&action.request()) };
            if !allow || self.ivars().panel_open.get() || !self.ivars().downloads.borrow().is_empty() {
                unsafe { download.cancel(None); }
                self.status("Download refused: finish the current dialog/download first");
                return;
            }
            self.ivars().downloads.borrow_mut().insert(download as *const _ as usize,
                Download { object: download.retain(), file: None });
            unsafe { download.setDelegate(Some(ProtocolObject::from_ref(self))); }
        }
        #[unsafe(method(webView:navigationResponse:didBecomeDownload:))]
        fn response_download(&self, web: &WKWebView, response: &WKNavigationResponse, download: &WKDownload) {
            if !self.transfer_response(web, response) || self.ivars().panel_open.get() || !self.ivars().downloads.borrow().is_empty() {
                unsafe { download.cancel(None); }
                self.clear_transfer_view();
                self.status("Download refused: invalid response or busy");
                return;
            }
            self.ivars().downloads.borrow_mut().insert(download as *const _ as usize,
                Download { object: download.retain(), file: None });
            unsafe { download.setDelegate(Some(ProtocolObject::from_ref(self))); }
        }
        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn failed_load(&self, web: &WKWebView, _nav: Option<&WKNavigation>, _error: &NSError) {
            if self.is_transfer_view(web) {
                // Navigation cancellation when becoming a download is expected.
                if !self.ivars().transfer_view.borrow().as_ref().is_some_and(|v| v.downloading) {
                    self.clear_transfer_view();
                    self.status("Download navigation failed — explicit retry only");
                }
                return;
            }
            // NSError can contain the bootstrap URL: never print it.
            self.fail("WebView load failed; no request was replayed");
        }
        #[unsafe(method(webView:didFinishNavigation:))]
        fn loaded(&self, _web: &WKWebView, _nav: Option<&WKNavigation>) {
            if self.ivars().recovering.replace(false) {
                self.ivars().probe_count.set(0);
                self.ivars().recovery_probe.set(true);
            }
        }
        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn web_crashed(&self, _web: &WKWebView) {
            self.fail("WebView process ended — use floe2 menu: Recover View or Force End Session");
        }
    }
    unsafe impl WKDownloadDelegate for Host {
        #[unsafe(method(download:decideDestinationUsingResponse:suggestedFilename:completionHandler:))]
        fn download_destination(&self, download: &WKDownload, response: &NSURLResponse,
            name: &NSString, completion: &DynBlock<dyn Fn(*mut NSURL)>) {
            self.save_download(download, response, name, completion);
        }
        #[unsafe(method(download:willPerformHTTPRedirection:newRequest:decisionHandler:))]
        fn download_redirect(&self, _download: &WKDownload, _response: &NSHTTPURLResponse,
            _request: &NSURLRequest, decision: &DynBlock<dyn Fn(WKDownloadRedirectPolicy)>) {
            decision.call((WKDownloadRedirectPolicy::Cancel,));
        }
        #[unsafe(method(downloadDidFinish:))]
        fn download_finished(&self, download: &WKDownload) {
            let slot = self.ivars().downloads.borrow_mut().remove(&(download as *const _ as usize));
            if let Some(file) = slot.and_then(|s| s.file) {
                match file.publish() {
                    Ok(()) => self.status("Download saved (new file; existing files unchanged)"),
                    Err(_) => self.status("Download publication not confirmed — check destination; no automatic retry"),
                }
            }
            self.clear_transfer_view();
        }
        #[unsafe(method(download:didFailWithError:resumeData:))]
        fn download_failed(&self, download: &WKDownload, _error: &NSError, _resume: Option<&NSData>) {
            let slot = self.ivars().downloads.borrow_mut().remove(&(download as *const _ as usize));
            if slot.is_some() { self.status("Download failed — partial file discarded; explicit retry only"); }
            self.clear_transfer_view();
        }
    }
    unsafe impl WKUIDelegate for Host {
        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_transfer_view(&self, web: &WKWebView, config: &WKWebViewConfiguration,
            action: &WKNavigationAction, _features: &WKWindowFeatures) -> Option<Retained<WKWebView>> {
            self.new_transfer_view(web, config, action)
        }
        #[unsafe(method(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:))]
        fn open_file(&self, _web: &WKWebView, parameters: &WKOpenPanelParameters,
            frame: &WKFrameInfo, completion: &DynBlock<dyn Fn(*mut NSArray<NSURL>)>) {
            if !self.owned_frame(frame) || unsafe { parameters.allowsDirectories() }
                || self.ivars().panel_open.replace(true) {
                completion.call((std::ptr::null_mut(),)); return;
            }
            let panel = NSOpenPanel::openPanel(self.mtm());
            panel.setCanChooseFiles(true); panel.setCanChooseDirectories(false);
            panel.setAllowsMultipleSelection(false);
            panel.setCanDownloadUbiquitousContents(false);
            // Every current web input accepts one file; format/size/scope remain
            // validated by the existing JS/server, not by the filename filter.
            let done = completion.copy();
            let host = self.retain(); let chosen = panel.clone();
            let callback = RcBlock::new(move |result| {
                host.ivars().panel_open.set(false);
                if result == NSModalResponseOK {
                    let urls = chosen.URLs();
                    done.call((Retained::as_ptr(&urls).cast_mut(),));
                } else { done.call((std::ptr::null_mut(),)); }
            });
            panel.beginSheetModalForWindow_completionHandler(self.ivars().window.get().unwrap(), &callback);
        }
        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        fn media(
            &self,
            _web: &WKWebView,
            _origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            _kind: WKMediaCaptureType,
            decision: &DynBlock<dyn Fn(WKPermissionDecision)>,
        ) {
            decision.call((WKPermissionDecision::Deny,));
        }
        #[unsafe(method(webView:requestDeviceOrientationAndMotionPermissionForOrigin:initiatedByFrame:decisionHandler:))]
        fn motion(
            &self,
            _web: &WKWebView,
            _origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            decision: &DynBlock<dyn Fn(WKPermissionDecision)>,
        ) {
            decision.call((WKPermissionDecision::Deny,));
        }
    }
);

impl Host {
    fn new_transfer_view(
        &self,
        web: &WKWebView,
        config: &WKWebViewConfiguration,
        action: &WKNavigationAction,
    ) -> Option<Retained<WKWebView>> {
        let valid = self
            .ivars()
            .web
            .get()
            .is_some_and(|main| std::ptr::eq(&**main, web))
            && unsafe {
                action.targetFrame().is_none()
                    && action.sourceFrame().is_some_and(|f| self.owned_frame(&f))
            }
            && unsafe {
                self.allowed_download(&action.request())
                    && action
                        .request()
                        .HTTPMethod()
                        .is_some_and(|m| m.to_string() == "POST")
            };
        if !valid
            || self.ivars().panel_open.get()
            || !self.ivars().downloads.borrow().is_empty()
            || self.ivars().transfer_view.borrow().is_some()
        {
            return None;
        }
        let url = unsafe { action.request().URL()?.absoluteString()? }.to_string();
        // WebKit supplies a copy of the owning configuration/data store and
        // performs the ORIGINAL POST. No window is shown; no response is
        // ever committed as HTML, and this view has no UI delegate/IPC.
        let child = unsafe {
            WKWebView::initWithFrame_configuration(
                WKWebView::alloc(self.mtm()),
                NSRect::ZERO,
                config,
            )
        };
        unsafe {
            child.setNavigationDelegate(Some(ProtocolObject::from_ref(self)));
        }
        *self.ivars().transfer_view.borrow_mut() = Some(TransferView {
            web: child.clone(),
            url,
            started: false,
            downloading: false,
            start: Instant::now(),
        });
        Some(child)
    }
    fn is_transfer_view(&self, web: &WKWebView) -> bool {
        self.ivars()
            .transfer_view
            .borrow()
            .as_ref()
            .is_some_and(|v| std::ptr::eq(&*v.web, web))
    }
    fn transfer_response(&self, web: &WKWebView, response: &WKNavigationResponse) -> bool {
        let slot = self.ivars().transfer_view.borrow();
        let Some(transfer) = slot.as_ref() else {
            return false;
        };
        if !std::ptr::eq(&*transfer.web, web) || !unsafe { response.isForMainFrame() } {
            return false;
        }
        let reply = unsafe { response.response() };
        reply
            .URL()
            .and_then(|u| u.absoluteString())
            .is_some_and(|u| u.to_string() == transfer.url)
            && reply
                .downcast_ref::<NSHTTPURLResponse>()
                .is_some_and(|r| r.statusCode() == 200)
            && reply
                .MIMEType()
                .is_some_and(|m| m.to_string() == "application/octet-stream")
    }
    fn clear_transfer_view(&self) {
        let slot = self.ivars().transfer_view.borrow_mut().take();
        if let Some(transfer) = slot {
            unsafe {
                transfer.web.setNavigationDelegate(None);
                transfer.web.stopLoading();
            }
        }
    }
    fn new(mtm: MainThreadMarker, service: Service, smoke: bool) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(State {
            service: RefCell::new(service),
            origin: OnceCell::new(),
            window: OnceCell::new(),
            web: OnceCell::new(),
            failure: RefCell::new(None),
            panel_open: Cell::new(false),
            downloads: RefCell::new(BTreeMap::new()),
            transfer_view: RefCell::new(None),
            recovery_probe: Cell::new(false),
            recovering: Cell::new(false),
            probe_next: Cell::new(Instant::now()),
            probe_count: Cell::new(0),
            smoke,
            smoke_step: Cell::new(0),
            evaluating: Cell::new(false),
            smoke_probe: RefCell::new(String::new()),
            start: Instant::now(),
        });
        // SAFETY: NSObject's initializer, on an allocated initialized-ivars instance.
        unsafe { msg_send![super(this), init] }
    }
    fn fail(&self, message: &'static str) {
        *self.ivars().failure.borrow_mut() = Some(message);
        self.status(message);
        if self.ivars().smoke {
            self.ivars().service.borrow().cancel();
        }
    }
    fn status(&self, message: &str) {
        if let Some(window) = self.ivars().window.get() {
            window.setTitle(&NSString::from_str(message));
        }
    }
    fn owned_frame(&self, frame: &WKFrameInfo) -> bool {
        // Frame URLs can be about:blank; trust the actual security origin and
        // require the owning top frame, never a cross-origin child.
        unsafe {
            let origin = frame.securityOrigin();
            frame.isMainFrame()
                && self.ivars().origin.get().is_some_and(|expected| {
                    *expected
                        == format!(
                            "{}://{}:{}",
                            origin.protocol(),
                            origin.host(),
                            origin.port()
                        )
                })
        }
    }
    fn allowed_download(&self, request: &NSURLRequest) -> bool {
        let Some(origin) = self.ivars().origin.get() else {
            return false;
        };
        let Some(url) = request.URL().and_then(|u| u.absoluteString()) else {
            return false;
        };
        let method = request
            .HTTPMethod()
            .map(|m| m.to_string())
            .unwrap_or_default();
        transfers::download_allowed(origin, &url.to_string(), &method)
    }
    fn save_download(
        &self,
        download: &WKDownload,
        response: &NSURLResponse,
        name: &NSString,
        completion: &DynBlock<dyn Fn(*mut NSURL)>,
    ) {
        let key = download as *const _ as usize;
        let status = response
            .downcast_ref::<NSHTTPURLResponse>()
            .map(|r| r.statusCode());
        let refusal = if status.is_some_and(|s| s != 200) {
            Some(format!(
                "Download refused: HTTP {} (no file saved)",
                status.unwrap()
            ))
        } else if response.expectedContentLength() > transfers::MAX_BYTES as i64 {
            Some("Download refused: exceeds 512 MiB".into())
        } else if !self.ivars().downloads.borrow().contains_key(&key) {
            Some("Download refused: no active transfer".into())
        } else if self.ivars().panel_open.get() {
            Some("Download refused: finish the current dialog first".into())
        } else {
            None
        };
        if let Some(reason) = refusal {
            self.ivars().downloads.borrow_mut().remove(&key);
            completion.call((std::ptr::null_mut(),));
            self.status(&reason);
            return;
        }
        self.ivars().panel_open.set(true);
        let panel = NSSavePanel::savePanel(self.mtm());
        panel.setNameFieldStringValue(&NSString::from_str(&transfers::suggested_name(
            &name.to_string(),
        )));
        panel.setMessage(Some(ns_string!("Save a new file. Existing files are never replaced. Cancelling leaves the server artifact available.")));
        let done = completion.copy();
        let host = self.retain();
        let chosen = panel.clone();
        let callback = RcBlock::new(move |result| {
            host.ivars().panel_open.set(false);
            if result == NSModalResponseOK {
                let file = chosen
                    .URL()
                    .filter(|u| u.isFileURL())
                    .and_then(|u| u.path())
                    .and_then(|p| PendingFile::new(Path::new(&p.to_string())).ok());
                if let Some(file) = file {
                    let url = NSURL::fileURLWithPath(&NSString::from_str(
                        &file.staging().to_string_lossy(),
                    ));
                    let mut slots = host.ivars().downloads.borrow_mut();
                    if let Some(slot) = slots.get_mut(&key) {
                        slot.file = Some(file);
                        drop(slots);
                        host.status("Downloading to private staging file…");
                        done.call((Retained::as_ptr(&url).cast_mut(),));
                        return;
                    }
                }
                host.status(
                    "Download not saved — select a new writable filename; existing file preserved",
                );
            } else {
                host.status("Download cancelled — no destination file written");
            }
            host.ivars().downloads.borrow_mut().remove(&key);
            done.call((std::ptr::null_mut(),));
        });
        panel.beginSheetModalForWindow_completionHandler(
            self.ivars().window.get().unwrap(),
            &callback,
        );
    }
    fn confirm(&self, title: &str, details: &str, accept: &str, action: impl Fn(&Host) + 'static) {
        if self.ivars().panel_open.replace(true) {
            return;
        }
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(details));
        let cancel = alert.addButtonWithTitle(ns_string!("Cancel"));
        cancel.setKeyEquivalent(ns_string!("\r"));
        alert.window().setInitialFirstResponder(Some(&cancel));
        alert
            .addButtonWithTitle(&NSString::from_str(accept))
            .setKeyEquivalent(ns_string!(""));
        let host = self.retain();
        let callback = RcBlock::new(move |result| {
            host.ivars().panel_open.set(false);
            if result == NSAlertSecondButtonReturn {
                action(&host);
            }
        });
        alert.beginSheetModalForWindow_completionHandler(
            self.ivars().window.get().unwrap(),
            Some(&callback),
        );
    }
    fn recover(&self) {
        if !self.ivars().downloads.borrow().is_empty()
            || self.ivars().transfer_view.borrow().is_some()
        {
            self.status("Finish the download before recovering the view");
            return;
        }
        self.confirm("Recover View?", "Reload the current local session. Unsaved editor text and captured pixels will be lost. Earlier approved saves may already have completed. Existing receipt records are checked, not automatically replayed. If session storage was lost, restart the app; the one-use login is never replayed.", "Reload View", |host| {
            if let (Some(web), Some(origin)) = (host.ivars().web.get(), host.ivars().origin.get()) {
                // Explicit GET of a credential-free root, using the SAME WebView
                // and data store (sessionStorage/cookies). Never reload bootstrap.
                let url = NSURL::URLWithString(&NSString::from_str(&format!("{origin}/"))).unwrap();
                host.ivars().recovering.set(true);
                host.ivars().recovery_probe.set(false);
                unsafe { web.loadRequest(&NSURLRequest::requestWithURL(&url)); }
                *host.ivars().failure.borrow_mut() = None;
                host.status("Recovering view — check connection and any pending save receipts");
            }
        });
    }
    fn force_close(&self) {
        self.confirm("Force End Session?", "Use only when the normal End session dialog is unavailable. Unsaved drafts and in-progress downloads will be discarded. Earlier approved writes may have completed: check the files before retrying. No save will be approved or replayed.", "End Session", |host| {
            host.cancel_downloads();
            host.ivars().service.borrow().cancel();
        });
    }
    fn cancel_downloads(&self) {
        self.clear_transfer_view();
        let slots = std::mem::take(&mut *self.ivars().downloads.borrow_mut());
        for (_, slot) in slots {
            unsafe {
                slot.object.setDelegate(None);
                slot.object.cancel(None);
            }
        }
    }
    fn request_close(&self) {
        if self.ivars().panel_open.get() {
            return;
        }
        if self.ivars().failure.borrow().is_some() {
            self.force_close();
            return;
        }
        if self.ivars().smoke {
            eprintln!("[desktop-smoke] native close callback");
        }
        if let Some(web) = self.ivars().web.get() {
            // Only opens the existing cancel-default dialog. It cannot approve
            // a save, select a file, or authorize the DELETE by itself.
            let host = self.retain();
            let callback = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
                let opened = unsafe { value.as_ref() }
                    .and_then(|v| v.downcast_ref::<NSString>())
                    .is_some_and(|s| s.to_string() == "opened");
                if !error.is_null() || !opened {
                    host.force_close();
                }
            });
            unsafe {
                web.evaluateJavaScript_completionHandler(
                    &NSString::from_str(CLOSE_SCRIPT),
                    Some(&callback),
                );
            }
        } else {
            // No web page, draft or user operation exists during initial startup.
            self.ivars().service.borrow().cancel();
        }
    }
    fn load(&self, ready: Ready) -> Result<()> {
        validate_ready(&ready)?;
        self.ivars()
            .origin
            .set(ready.origin)
            .map_err(|_| Error::input("duplicate desktop ready"))?;
        let window = self.ivars().window.get().unwrap();
        let mtm = self.mtm();
        // SAFETY: New configuration/data store are owned here, WK retains the
        // configuration, NSWindow retains the view; weak delegates outlive both.
        let web = unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
            config
                .preferences()
                .setJavaScriptCanOpenWindowsAutomatically(false);
            let web = WKWebView::initWithFrame_configuration(
                WKWebView::alloc(mtm),
                window.contentView().unwrap().bounds(),
                &config,
            );
            web.setNavigationDelegate(Some(ProtocolObject::from_ref(self)));
            web.setUIDelegate(Some(ProtocolObject::from_ref(self)));
            web.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            window.setContentView(Some(&web));
            let url = NSURL::URLWithString(&NSString::from_str(&ready.url))
                .ok_or_else(|| Error::input("invalid desktop launch URL"))?;
            web.loadRequest(&NSURLRequest::requestWithURL(&url));
            web
        };
        self.ivars()
            .web
            .set(web)
            .map_err(|_| Error::input("duplicate desktop WebView"))?;
        window.setTitle(ns_string!("floe2 — embedded preview"));
        Ok(())
    }
    fn poll(&self) {
        let expired = self
            .ivars()
            .transfer_view
            .borrow()
            .as_ref()
            .is_some_and(|v| !v.downloading && v.start.elapsed() > Duration::from_secs(30));
        if expired {
            self.clear_transfer_view();
            self.status("Download response timed out — explicit retry only");
        }
        if self.ivars().recovery_probe.get()
            && !self.ivars().evaluating.get()
            && Instant::now() >= self.ivars().probe_next.get()
        {
            self.probe_recovery();
        }
        let ready = self.ivars().service.borrow().ready.try_recv();
        if let Ok(ready) = ready {
            if self.load(ready).is_err() {
                self.fail("cannot create embedded view");
                self.ivars().service.borrow().cancel();
            }
        }
        if self.ivars().service.borrow().finished() {
            self.cancel_downloads();
            // NSApp.stop alone does not wake its blocking event read.
            let app = NSApplication::sharedApplication(self.mtm());
            app.stop(None);
            if let Some(event) = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
                NSEventType::ApplicationDefined, NSPoint::ZERO, NSEventModifierFlags::empty(), 0.0, 0, None, 0, 0, 0) {
                app.postEvent_atStart(&event, true);
            }
        } else {
            // Unknown content length is checked while transferring as well as
            // before publication. No failed download is ever resumed implicitly.
            let oversized: Vec<_> = self
                .ivars()
                .downloads
                .borrow()
                .iter()
                .filter_map(|(key, slot)| {
                    slot.file
                        .as_ref()
                        .and_then(|f| std::fs::metadata(f.staging()).ok())
                        .filter(|m| m.len() > transfers::MAX_BYTES)
                        .map(|_| *key)
                })
                .collect();
            for key in oversized {
                if let Some(slot) = self.ivars().downloads.borrow_mut().remove(&key) {
                    unsafe {
                        slot.object.setDelegate(None);
                        slot.object.cancel(None);
                    }
                    self.status("Download exceeded 512 MiB — cancelled, destination unchanged");
                }
            }
            if self.ivars().smoke {
                self.smoke_tick();
            }
        }
    }
    fn probe_recovery(&self) {
        self.ivars()
            .probe_next
            .set(Instant::now() + Duration::from_millis(500));
        let count = self.ivars().probe_count.get() + 1;
        self.ivars().probe_count.set(count);
        if count > 60 {
            self.ivars().recovery_probe.set(false);
            self.status("Recovery not confirmed — inspect connection/receipts or restart; no write replayed");
            return;
        }
        let Some(web) = self.ivars().web.get() else {
            return;
        };
        self.ivars().evaluating.set(true);
        let host = self.retain();
        let callback = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
            host.ivars().evaluating.set(false);
            let marker = unsafe { value.as_ref() }
                .and_then(|v| v.downcast_ref::<NSString>())
                .map(|s| s.to_string());
            if !error.is_null() {
                return;
            }
            match marker.as_deref() {
                Some("ready") => {
                    host.ivars().recovery_probe.set(false);
                    host.status(
                        "floe2 — authenticated page reloaded; check frame and save receipts",
                    );
                }
                Some("hidden") => {
                    host.status("View reloaded but hidden — activate this window to resume frames")
                }
                _ => (),
            }
        });
        // Fixed markers only. No auth, paths, note text or clipboard contents
        // leave the WebView. A fresh page's enabled End session follows auth.
        unsafe {
            web.evaluateJavaScript_completionHandler(&NSString::from_str("(()=>{if(document.hidden)return 'hidden';const b=document.getElementById('logout');return b&&!b.disabled?'ready':'waiting';})()"), Some(&callback));
        }
    }
    fn smoke_tick(&self) {
        if self.ivars().start.elapsed() > Duration::from_secs(120) {
            self.fail("native smoke deadline exceeded");
            return;
        }
        let Some(web) = self.ivars().web.get() else {
            return;
        };
        if self.ivars().evaluating.replace(true) {
            return;
        }
        let step = self.ivars().smoke_step.get();
        // Empty-workspace QA only, never accepts a source/reviewer/write scope.
        let js = match step {
            0 => "(()=>{const b=document.getElementById('logout');return b&&!b.disabled?'ready':JSON.stringify([!!b,typeof FloeProtocol==='object',typeof FloeSessionExit==='object',document.readyState==='complete',!!location.hash]);})()",
            1 | 3 => "(()=>{const p=document.getElementById('session-exit-dialog');return p&&!p.hidden&&document.activeElement.id==='session-exit-cancel'?'confirm':JSON.stringify([!!p,!!p&&p.hidden,document.activeElement.id==='session-exit-cancel',document.hasFocus()]);})()",
            2 => "document.getElementById('session-exit-dialog').hidden?'cancelled':'wait'",
            _ => { self.ivars().evaluating.set(false); return; },
        };
        let host = self.retain();
        let callback = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
            host.ivars().evaluating.set(false);
            if !error.is_null() {
                if host.ivars().smoke_probe.borrow().as_str() != "js-error" {
                    eprintln!("[desktop-smoke] step={step} JavaScript evaluation failed");
                    *host.ivars().smoke_probe.borrow_mut() = "js-error".into();
                }
                return;
            }
            // SAFETY: WebKit guarantees nullable live objects for this callback;
            // only a checked NSString is inspected, never arbitrary JS content.
            let text = unsafe { value.as_ref() }
                .and_then(|v| v.downcast_ref::<NSString>())
                .map(|s| s.to_string());
            let Some(text) = text else {
                return;
            };
            if *host.ivars().smoke_probe.borrow() != text {
                // Only fixed markers / five booleans can leave this QA probe.
                if ["ready", "confirm", "cancelled", "wait"].contains(&text.as_str())
                    || text.bytes().all(|b| b"[]truefals, ".contains(&b))
                {
                    eprintln!("[desktop-smoke] step={step} probe={text}");
                }
                *host.ivars().smoke_probe.borrow_mut() = text.clone();
            }
            let next = match (step, text.as_str()) {
                (0, "ready") => {
                    host.ivars().window.get().unwrap().performClose(None);
                    1
                }
                (1, "confirm") => {
                    host.eval("document.getElementById('session-exit-cancel').click()");
                    2
                }
                (2, "cancelled") => {
                    // Exercise the actual application delegate (Dock/menu Quit).
                    NSApplication::sharedApplication(host.mtm()).terminate(None);
                    3
                }
                (3, "confirm") => {
                    host.eval("document.getElementById('session-exit-confirm').click()");
                    4
                }
                _ => step,
            };
            host.ivars().smoke_step.set(next);
        });
        unsafe {
            web.evaluateJavaScript_completionHandler(&NSString::from_str(js), Some(&callback));
        }
    }
    fn eval(&self, script: &str) {
        // Fixed host-owned scripts only; there is no JS→native IPC bridge.
        if let Some(web) = self.ivars().web.get() {
            unsafe {
                web.evaluateJavaScript_completionHandler(&NSString::from_str(script), None);
            }
        }
    }
}

pub fn run(session: Session, smoke: bool) -> Result<i32> {
    if !objc2::available!(macos = 12.0) {
        return Err(Error::input("embedded preview requires macOS 12 or later"));
    }
    let mtm =
        MainThreadMarker::new().ok_or_else(|| Error::input("desktop requires the main thread"))?;
    let app = NSApplication::sharedApplication(mtm);
    let host = Host::new(mtm, Service::start(session)?, smoke);
    // SAFETY: Owned window never auto-releases on close. The main-thread host
    // remains retained until after timer invalidation and delegate detachment.
    let window = unsafe {
        let w = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(1200.0, 850.0)),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        );
        w.setReleasedWhenClosed(false);
        w
    };
    window.setTitle(ns_string!("floe2 — starting local service…"));
    window.setDelegate(Some(ProtocolObject::from_ref(&*host)));
    window.center();
    window.makeKeyAndOrderFront(None);
    host.ivars().window.set(window).unwrap();
    app.setDelegate(Some(ProtocolObject::from_ref(&*host)));
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let menu = NSMenu::new(mtm);
    let item = NSMenuItem::new(mtm);
    let submenu = NSMenu::new(mtm);
    // SAFETY: terminate: is NSApplication's standard action; target is resolved
    // by AppKit. Our applicationShouldTerminate: always guards it.
    unsafe {
        let recover = submenu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Recover View…"),
            Some(sel!(recoverView:)),
            ns_string!(""),
        );
        recover.setTarget(Some(&host));
        let force = submenu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Force End Session…"),
            Some(sel!(forceEndSession:)),
            ns_string!(""),
        );
        force.setTarget(Some(&host));
        submenu.addItem(&NSMenuItem::separatorItem(mtm));
        submenu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Quit floe2"),
            Some(sel!(terminate:)),
            ns_string!("q"),
        );
    }
    item.setSubmenu(Some(&submenu));
    menu.addItem(&item);
    let edit_item = NSMenuItem::new(mtm);
    let edit = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Edit"));
    // AppKit responder-chain actions preserve text selection/IME and WebKit's
    // user-gesture clipboard permission; no clipboard polling or JS bridge.
    for (title, action, key) in [
        ("Undo", sel!(undo:), "z"),
        ("Cut", sel!(cut:), "x"),
        ("Copy", sel!(copy:), "c"),
        ("Paste", sel!(paste:), "v"),
        ("Select All", sel!(selectAll:), "a"),
    ] {
        unsafe {
            edit.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            );
        }
    }
    edit_item.setSubmenu(Some(&edit));
    menu.addItem(&edit_item);
    app.setMainMenu(Some(&menu));
    let timer_host = host.clone();
    let block = RcBlock::new(move |_: NonNull<NSTimer>| timer_host.poll());
    // SAFETY: Timer scheduled and invalidated on this same main thread. The
    // block retains host; it never transfers UI objects to the service thread.
    let timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.05, true, &block) };
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    app.run();
    timer.invalidate();
    app.setDelegate(None);
    if let Some(web) = host.ivars().web.get() {
        unsafe {
            web.stopLoading();
            web.setNavigationDelegate(None);
            web.setUIDelegate(None);
        }
    }
    host.ivars().window.get().unwrap().setDelegate(None);
    let result = host.ivars().service.borrow_mut().join()?;
    if let Some(message) = *host.ivars().failure.borrow() {
        return Err(Error::input(message));
    }
    if smoke {
        if result != 0 || host.ivars().smoke_step.get() != 4 {
            return Err(Error::input(
                "native smoke did not complete confirmed shutdown",
            ));
        }
        println!("DESKTOP SMOKE: OK (WebKit auth; native close→cancel; application quit→confirm; service joined)");
    }
    Ok(result)
}
