//! The only native FFI boundary. All AppKit/WebKit objects stay on the main
//! thread; delegates and completion blocks are retained through their use.
use crate::actions::Action;
use crate::close_request::{CloseRequest, Event as CloseEvent};
use crate::recovery::{Event as RecoveryEvent, Recovery};
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
    recovery: RefCell<Recovery>,
    close_request: RefCell<CloseRequest>,
    navigation: RefCell<Option<Retained<WKNavigation>>>,
    smoke: bool,
    smoke_notices: bool,
    smoke_recovery: bool,
    smoke_review: bool,
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
        #[unsafe(method(openLayout:))]
        fn open_layout(&self, _sender: Option<&AnyObject>) { self.menu_action(Action::OpenLayout); }
        #[unsafe(method(openDrc:))]
        fn open_drc(&self, _sender: Option<&AnyObject>) { self.menu_action(Action::OpenDrc); }
        #[unsafe(method(showAbout:))]
        fn show_about(&self, _sender: Option<&AnyObject>) { self.menu_action(Action::About); }
        #[unsafe(method(recoverView:))]
        fn recover_view(&self, _sender: Option<&AnyObject>) { self.recover(); }
        #[unsafe(method(forceEndSession:))]
        fn force_end_session(&self, _sender: Option<&AnyObject>) { self.force_close(); }
    }
    unsafe impl NSApplicationDelegate for Host {
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, _app: &NSApplication, _visible: bool) -> bool {
            // Dock reactivation reveals this session, never replays open/save.
            if let Some(window) = self.ivars().window.get() {
                window.deminiaturize(None);
                window.makeKeyAndOrderFront(None);
            }
            false
        }
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
                if self.ivars().panel_open.get() || self.ivars().recovery.borrow().busy() || !self.ivars().downloads.borrow().is_empty()
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
            if !self.is_transfer_view(web) {
                unsafe { download.cancel(None); }
                return;
            }
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
        fn failed_load(&self, web: &WKWebView, nav: Option<&WKNavigation>, _error: &NSError) {
            self.navigation_failed(web, nav);
        }
        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn failed_committed_load(&self, web: &WKWebView, nav: Option<&WKNavigation>, _error: &NSError) {
            self.navigation_failed(web, nav);
        }
        #[unsafe(method(webView:didFinishNavigation:))]
        fn loaded(&self, web: &WKWebView, nav: Option<&WKNavigation>) {
            self.navigation_loaded(web, nav);
        }
        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn web_crashed(&self, web: &WKWebView) {
            if self.is_transfer_view(web) {
                self.cancel_downloads();
                self.status("Download WebView ended — partial file discarded; explicit retry only");
            } else if self.is_main_view(web) {
                self.cancel_downloads();
                self.fail("WebView process ended — use floe2 menu: Recover View or Force End Session");
            }
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
            let Some(slot) = slot else { return; };
            if let Some(file) = slot.file {
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
            if slot.is_none() { return; }
            self.status("Download failed — partial file discarded; explicit retry only");
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
            if !self.owned_frame(frame) || self.ivars().recovery.borrow().busy() || unsafe { parameters.allowsDirectories() }
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
    fn is_main_view(&self, web: &WKWebView) -> bool {
        self.ivars()
            .web
            .get()
            .is_some_and(|main| std::ptr::eq(&**main, web))
    }
    fn is_current_navigation(&self, web: &WKWebView, nav: Option<&WKNavigation>) -> bool {
        self.is_main_view(web)
            && nav.is_some_and(|nav| {
                self.ivars()
                    .navigation
                    .borrow()
                    .as_ref()
                    .is_some_and(|current| std::ptr::eq(&**current, nav))
            })
    }
    fn navigation_failed(&self, web: &WKWebView, nav: Option<&WKNavigation>) {
        if self.is_transfer_view(web) {
            // Becoming a download cancels its navigation normally. A retired
            // transfer's late callback must not disturb a newer transfer/view.
            if !self
                .ivars()
                .transfer_view
                .borrow()
                .as_ref()
                .is_some_and(|v| v.downloading)
            {
                self.clear_transfer_view();
                self.status("Download navigation failed — explicit retry only");
            }
        } else if self.is_current_navigation(web, nav) || (self.is_main_view(web) && nav.is_none())
        {
            // NSError can contain the bootstrap URL: never inspect/print it.
            self.fail(
                "WebView load failed — use Recover View or Force End Session; no request replayed",
            );
        }
    }
    fn navigation_loaded(&self, web: &WKWebView, nav: Option<&WKNavigation>) {
        if self.is_current_navigation(web, nav) {
            self.ivars().recovery.borrow_mut().loaded(Instant::now());
        }
    }
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
            || self.ivars().recovery.borrow().busy()
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
            if self.ivars().smoke_review {
                // The supplied configuration inherits the main content controller.
                // Do not copy QA observers into the separate transfer WebView.
                config.setUserContentController(&WKUserContentController::new(self.mtm()));
            }
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
    fn new(
        mtm: MainThreadMarker,
        service: Service,
        smoke: bool,
        smoke_notices: bool,
        smoke_recovery: bool,
        smoke_review: bool,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(State {
            service: RefCell::new(service),
            origin: OnceCell::new(),
            window: OnceCell::new(),
            web: OnceCell::new(),
            failure: RefCell::new(None),
            panel_open: Cell::new(false),
            downloads: RefCell::new(BTreeMap::new()),
            transfer_view: RefCell::new(None),
            recovery: RefCell::new(Recovery::default()),
            close_request: RefCell::new(CloseRequest::default()),
            navigation: RefCell::new(None),
            smoke,
            smoke_notices,
            smoke_recovery,
            smoke_review,
            smoke_step: Cell::new(if smoke_review { 20 } else { 0 }),
            evaluating: Cell::new(false),
            smoke_probe: RefCell::new(String::new()),
            start: Instant::now(),
        });
        // SAFETY: NSObject's initializer, on an allocated initialized-ivars instance.
        unsafe { msg_send![super(this), init] }
    }
    fn fail(&self, message: &'static str) {
        self.ivars().close_request.borrow_mut().invalidate();
        self.ivars().recovery.borrow_mut().fail();
        self.ivars().navigation.borrow_mut().take();
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
    fn menu_action(&self, action: Action) {
        if self.ivars().panel_open.get() || self.ivars().recovery.borrow().busy() {
            return;
        }
        let Some(web) = self.ivars().web.get() else {
            return;
        };
        let host = self.retain();
        let epoch = self.ivars().recovery.borrow().epoch();
        let callback = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
            if host.ivars().recovery.borrow().epoch() != epoch
                || host.ivars().failure.borrow().is_some()
            {
                return;
            }
            // Never expose arbitrary JS values or NSError (may contain URLs).
            let marker = if error.is_null() {
                unsafe { value.as_ref() }
                    .and_then(|v| v.downcast_ref::<NSString>())
                    .map(|v| v.to_string())
            } else {
                None
            };
            if host.ivars().smoke {
                let status = match marker.as_deref() {
                    Some("opened") => "opened",
                    Some("busy") => "busy",
                    Some("unavailable") => "unavailable",
                    _ => "evaluation-failed",
                };
                eprintln!("[desktop-smoke] menu={status}");
            }
            match marker.as_deref() {
                Some("opened") => host.status("floe2 — embedded preview"),
                Some("busy") => host.status("Finish or cancel the current dialog first"),
                _ => {
                    host.status("Menu action unavailable — wait for the view, or use Recover View")
                }
            }
        });
        // The action enum supplies only fixed button IDs. Existing web controls
        // retain all registered-root, indexing and review approval checks.
        unsafe {
            web.evaluateJavaScript_completionHandler(
                &NSString::from_str(&action.script()),
                Some(&callback),
            );
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
        let close_qa = self.ivars().smoke_recovery
            && self.ivars().smoke_step.get() == 15
            && title == "Force End Session?";
        let callback = RcBlock::new(move |result| {
            host.ivars().panel_open.set(false);
            if close_qa {
                if result != NSAlertFirstButtonReturn || host.ivars().service.borrow().finished() {
                    host.fail("native close timeout QA did not cancel safely");
                    return;
                }
                eprintln!("[desktop-smoke] close timeout: native Cancel; session preserved");
                host.ivars().smoke_step.set(16);
            }
            if result == NSAlertSecondButtonReturn {
                action(&host);
            }
        });
        alert.beginSheetModalForWindow_completionHandler(
            self.ivars().window.get().unwrap(),
            Some(&callback),
        );
        if close_qa {
            // Empty-workspace QA only: exercise the real NSAlert cancel action,
            // not an OS Return/keyboard acceptance test. Never accept Force End.
            if cancel.keyEquivalent().to_string() != "\r"
                || !alert.window().initialFirstResponder().is_some_and(|r| {
                    std::ptr::eq(
                        &*r as *const NSView,
                        &*cancel as *const NSButton as *const NSView,
                    )
                })
            {
                self.fail("native close timeout QA has a non-cancel default");
                return;
            }
            // SAFETY: Live Cancel control of this host-owned NSAlert, main thread.
            unsafe {
                cancel.performClick(None);
            }
        }
    }
    fn recover(&self) {
        if self.ivars().recovery.borrow().busy() {
            self.status("Recovery already in progress — wait, or use Force End Session");
            return;
        }
        if !self.ivars().downloads.borrow().is_empty()
            || self.ivars().transfer_view.borrow().is_some()
        {
            self.status("Finish the download before recovering the view");
            return;
        }
        self.confirm("Recover View?", "Reload the current local session. Unsaved editor text and captured pixels will be lost. Earlier approved saves may already have completed. Existing receipt records are checked, not automatically replayed. If session storage was lost, restart the app; the one-use login is never replayed.", "Reload View", |host| host.start_recovery());
    }
    fn start_recovery(&self) {
        if let (Some(web), Some(origin)) = (self.ivars().web.get(), self.ivars().origin.get()) {
            if self
                .ivars()
                .recovery
                .borrow_mut()
                .begin(Instant::now())
                .is_none()
            {
                self.status(
                    "Recovery already in progress or unavailable — use Force End Session if needed",
                );
                return;
            }
            // Explicit GET of a credential-free root, using the SAME WebView
            // and data store. Never reload bootstrap, POST, or an approval.
            self.ivars().close_request.borrow_mut().invalidate();
            let url = NSURL::URLWithString(&NSString::from_str(&format!("{origin}/"))).unwrap();
            let navigation = unsafe { web.loadRequest(&NSURLRequest::requestWithURL(&url)) };
            let started = navigation.is_some();
            *self.ivars().navigation.borrow_mut() = navigation;
            if started {
                self.status("Recovering view — check connection and any pending save receipts");
            } else {
                self.fail("Recovery could not start — use Recover View or Force End Session; no request replayed");
            }
        }
    }
    fn force_close(&self) {
        self.ivars().close_request.borrow_mut().invalidate();
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
        if self.ivars().failure.borrow().is_some() || self.ivars().recovery.borrow().busy() {
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
            let epoch = self.ivars().recovery.borrow().epoch();
            let Some(ticket) = self
                .ivars()
                .close_request
                .borrow_mut()
                .begin(Instant::now(), epoch)
            else {
                self.status("Waiting for the close dialog — Force End Session remains available");
                return;
            };
            if self.ivars().smoke_recovery && self.ivars().smoke_step.get() == 15 {
                // Intentionally omit this one evaluation/completion in the empty
                // QA session. The real timer/NSAlert still run; no WebKit kill.
                return;
            }
            let callback = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
                let opened = unsafe { value.as_ref() }
                    .and_then(|v| v.downcast_ref::<NSString>())
                    .is_some_and(|s| s.to_string() == "opened")
                    && error.is_null();
                let epoch = host.ivars().recovery.borrow().epoch();
                let event = host.ivars().close_request.borrow_mut().reply(
                    ticket,
                    Instant::now(),
                    epoch,
                    opened,
                );
                host.close_event(event);
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
            if self.ivars().smoke_review {
                // Only the no-argument QA mode creates this host, after generating
                // new synthetic inputs. Observe saves from document start so a
                // replay during startup cannot escape the counter. Main frame only.
                let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    WKUserScript::alloc(mtm),
                    &NSString::from_str(include_str!("../ui/review-transport.js")),
                    WKUserScriptInjectionTime::AtDocumentStart,
                    true,
                );
                config.userContentController().addUserScript(&script);
            }
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
            *self.ivars().navigation.borrow_mut() =
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
        let epoch = self.ivars().recovery.borrow().epoch();
        let close = self
            .ivars()
            .close_request
            .borrow_mut()
            .poll(Instant::now(), epoch);
        if !self.ivars().service.borrow().finished() {
            self.close_event(close);
        }
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
        let event = self.ivars().recovery.borrow_mut().poll(Instant::now());
        self.recovery_event(event);
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
            if self.ivars().smoke && !self.ivars().recovery.borrow().busy() {
                self.smoke_tick();
            }
        }
    }
    fn close_event(&self, event: CloseEvent) {
        if event == CloseEvent::OfferForceEnd {
            // This is an offer, not a shutdown deadline. The native sheet still
            // defaults to Cancel, including when the JS reply never arrives.
            self.status("Close dialog not confirmed — cancel or explicitly end the session");
            self.force_close();
        }
    }
    fn recovery_event(&self, event: RecoveryEvent) {
        match event {
            RecoveryEvent::None => (),
            RecoveryEvent::Probe(epoch) => self.probe_recovery(epoch),
            RecoveryEvent::Ready => {
                *self.ivars().failure.borrow_mut() = None;
                self.status("floe2 — authenticated page reloaded; check frame and save receipts");
            }
            RecoveryEvent::ReadyHidden => {
                *self.ivars().failure.borrow_mut() = None;
                self.status("Authenticated page reloaded — activate this window; check frame and save receipts");
            }
            RecoveryEvent::Hidden => {
                self.status("View reloaded but hidden — activate this window to resume frames")
            }
            RecoveryEvent::TimedOut => {
                self.fail("Recovery not confirmed within 30 s — check connection/receipts or restart; no write replayed");
                if let Some(web) = self.ivars().web.get() {
                    unsafe {
                        web.stopLoading();
                    }
                }
            }
        }
    }
    fn probe_recovery(&self, epoch: u64) {
        let Some(web) = self.ivars().web.get() else {
            return;
        };
        let host = self.retain();
        let callback = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
            let marker = if error.is_null() {
                unsafe { value.as_ref() }
                    .and_then(|v| v.downcast_ref::<NSString>())
                    .map(|s| s.to_string())
            } else {
                None
            };
            let event =
                host.ivars()
                    .recovery
                    .borrow_mut()
                    .reply(epoch, Instant::now(), marker.as_deref());
            host.recovery_event(event);
        });
        // Fixed markers only. No auth, paths, note text or clipboard contents
        // leave the WebView. A fresh page's enabled End session follows auth.
        unsafe {
            web.evaluateJavaScript_completionHandler(&NSString::from_str("(()=>{const b=document.getElementById('logout');if(b&&!b.disabled)return document.hidden?'ready-hidden':'ready';return document.hidden?'hidden':'waiting';})()"), Some(&callback));
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
        // Ordinary smoke is empty-workspace only. Review smoke creates its own
        // fresh fixture; no QA invocation accepts caller paths/reviewer scopes.
        let js = match step {
            0 => "(()=>{const b=document.getElementById('logout'),p=document.getElementById('browse-dialog'),c=document.getElementById('browse-close'),r=document.getElementById('browse-refresh');return b&&!b.disabled&&p&&!p.hidden&&c&&!c.disabled&&r&&!r.disabled?'ready':JSON.stringify([!!b,typeof FloeProtocol==='object',typeof FloeSessionExit==='object',document.readyState==='complete',!!location.hash]);})()",
            1 => "document.getElementById('browse-dialog').hidden?'dismissed':'wait'",
            3 if self.ivars().smoke_notices => "(()=>{const a=document.getElementById('about-dialog'),b=document.getElementById('browse-dialog'),c=document.getElementById('notice-catalog'),n=document.getElementById('notice-list-next');return a&&!a.hidden&&b&&b.hidden&&c&&!c.hidden&&document.getElementById('notice-files').children.length===64&&n&&!n.disabled?'about':'wait';})()",
            2 | 3 => "(()=>{const a=document.getElementById('about-dialog'),b=document.getElementById('browse-dialog');return a&&!a.hidden&&b&&b.hidden?'about':'wait';})()",
            4 => "document.getElementById('about-dialog').hidden?'dismissed':'wait'",
            5 | 7 => "(()=>{const p=document.getElementById('session-exit-dialog');return p&&!p.hidden&&document.activeElement.id==='session-exit-cancel'?'confirm':JSON.stringify([!!p,!!p&&p.hidden,document.activeElement.id==='session-exit-cancel',document.hasFocus()]);})()",
            6 => "document.getElementById('session-exit-dialog').hidden?'cancelled':'wait'",
            9 => "document.getElementById('notice-page-status').textContent.startsWith('Page 1 / ')&&document.getElementById('notice-text').textContent.length>0?'notice-read':'wait'",
            10 => "document.getElementById('notice-list-status').textContent.startsWith('65–128 of ')&&document.getElementById('notice-text').textContent===''?'notice-page':'wait'",
            11 => include_str!("../ui/ime-probe.js"),
            12 => concat!("(", include_str!("../ui/recovery-probe.js"), ")('arm')"),
            13 => concat!("(", include_str!("../ui/recovery-probe.js"), ")('check')"),
            14 => "document.getElementById('browse-dialog').hidden?'dismissed':'wait'",
            15 => { self.ivars().evaluating.set(false); return; },
            16 => "(()=>{const b=document.getElementById('logout'),d=document.getElementById('session-exit-dialog');return b&&!b.disabled&&d&&d.hidden?'close-cancelled':'wait';})()",
            20 => concat!("(", include_str!("../ui/review-probe.js"), ")('note-start')"),
            21 => concat!("(", include_str!("../ui/review-probe.js"), ")('note-check')"),
            22 => concat!("(", include_str!("../ui/review-probe.js"), ")('waive-start')"),
            23 => concat!("(", include_str!("../ui/review-probe.js"), ")('waive-check')"),
            24 => concat!("(", include_str!("../ui/review-probe.js"), ")('read-back')"),
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
                if [
                    "ready",
                    "about",
                    "dismissed",
                    "confirm",
                    "cancelled",
                    "notice-read",
                    "notice-page",
                    "ime-ok",
                    "ime-failed",
                    "armed",
                    "recovered",
                    "recovery-failed",
                    "review-failed",
                    "note-lost",
                    "note-resolved",
                    "waive-lost",
                    "waive-resolved",
                    "review-ok",
                    "review-wait-list",
                    "review-wait-read",
                    "review-wait-snapshot",
                    "close-cancelled",
                    "wait",
                ]
                .contains(&text.as_str())
                    || text.bytes().all(|b| b"[]truefals, ".contains(&b))
                {
                    eprintln!("[desktop-smoke] step={step} probe={text}");
                }
                *host.ivars().smoke_probe.borrow_mut() = text.clone();
            }
            let next = match (step, text.as_str()) {
                (20..=24, "review-failed") => {
                    host.fail("synthetic native review recovery QA failed");
                    step
                }
                (20, "note-lost") | (22, "waive-lost") => {
                    host.start_recovery();
                    step + 1
                }
                (21, "note-resolved") | (23, "waive-resolved") => step + 1,
                (24, "review-ok") => {
                    host.ivars().window.get().unwrap().performClose(None);
                    5
                }
                (0, "ready") => 11,
                (11, "ime-failed") => {
                    host.fail("native synthetic composition-key guard failed");
                    step
                }
                (11, "ime-ok") => {
                    // Empty workspaces open the file picker during startup.
                    // Wait for its initial catalogue request to finish before
                    // closing; a queued read temporarily disables Close.
                    host.eval("document.getElementById('browse-close').click()");
                    1
                }
                (1, "dismissed") if host.ivars().smoke_recovery => 12,
                (12, "armed") => {
                    let old = host.ivars().navigation.borrow().clone();
                    host.start_recovery();
                    let web = host.ivars().web.get().unwrap();
                    // Deterministic late-callback injection, not a process kill.
                    // Retaining old prevents pointer reuse by the new request.
                    if let Some(old) = old {
                        host.navigation_loaded(web, Some(&old));
                        host.navigation_failed(web, Some(&old));
                    } else {
                        host.fail("native recovery QA had no initial navigation");
                    }
                    let event = host.ivars().recovery.borrow_mut().poll(Instant::now());
                    if event != RecoveryEvent::None || !host.ivars().recovery.borrow().busy() {
                        host.fail("retired navigation changed a newer recovery attempt");
                    }
                    13
                }
                (12 | 13, "recovery-failed") => {
                    host.fail("native recovery document/storage check failed");
                    step
                }
                (13, "recovered") => {
                    host.eval("document.getElementById('browse-close').click()");
                    14
                }
                (14, "dismissed") => {
                    host.ivars().smoke_step.set(15);
                    // Duplicate native requests must keep one original deadline.
                    host.ivars().window.get().unwrap().performClose(None);
                    NSApplication::sharedApplication(host.mtm()).terminate(None);
                    15
                }
                (1, "dismissed") | (16, "close-cancelled") => {
                    host.menu_action(Action::About);
                    2
                }
                (2, "about") => {
                    // A native menu action must not replace an existing modal.
                    host.menu_action(Action::OpenLayout);
                    3
                }
                (3, "about") if host.ivars().smoke_notices => {
                    host.eval("document.getElementById('notice-files').children[0].click()");
                    9
                }
                (9, "notice-read") => {
                    host.eval("document.getElementById('notice-list-next').click()");
                    10
                }
                (3, "about") | (10, "notice-page") => {
                    host.eval("document.getElementById('about-close').click()");
                    4
                }
                (4, "dismissed") => {
                    host.ivars().window.get().unwrap().performClose(None);
                    5
                }
                (5, "confirm") => {
                    host.eval("document.getElementById('session-exit-cancel').click()");
                    6
                }
                (6, "cancelled") => {
                    // Exercise the actual application delegate (Dock/menu Quit).
                    NSApplication::sharedApplication(host.mtm()).terminate(None);
                    7
                }
                (7, "confirm") => {
                    host.eval("document.getElementById('session-exit-confirm').click()");
                    8
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

/// LaunchServices discards terminal stderr. Report startup/service failures
/// visibly, without writing an error file or replaying a failed operation.
pub fn show_error(message: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    finish_before_startup_modal(&app);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    let alert = NSAlert::new(mtm);
    alert.setMessageText(ns_string!("floe2 Desktop — session error"));
    alert.setInformativeText(&NSString::from_str(&format!(
        "{message}\n\nWhen launching with open --args, use absolute file paths. For relative paths use tools/run_desktop_macos_dev.sh."
    )));
    alert.addButtonWithTitle(ns_string!("Close"));
    alert.runModal();
}

fn finish_before_startup_modal(app: &NSApplication) {
    // run() normally completes AppKit launch, but these modal loops precede
    // it. Do not leave LaunchServices waiting for startup throughout a dialog.
    if !NSRunningApplication::currentApplication().isFinishedLaunching() {
        app.finishLaunching();
    }
}

pub fn run(
    mut session: Session,
    smoke: bool,
    smoke_notices: bool,
    smoke_recovery: bool,
    smoke_review: bool,
) -> Result<i32> {
    if !objc2::available!(macos = 12.0) {
        return Err(Error::input("embedded preview requires macOS 12 or later"));
    }
    let mtm =
        MainThreadMarker::new().ok_or_else(|| Error::input("desktop requires the main thread"))?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    if !smoke && session.needs_initial_directory() {
        // Finder has no meaningful working directory. The user chooses the
        // initial scope; never silently grant / or the entire home directory.
        finish_before_startup_modal(&app);
        let panel = NSOpenPanel::openPanel(mtm);
        panel.setTitle(Some(ns_string!("Choose floe2 working folder")));
        panel.setMessage(Some(ns_string!("Allow this session to browse layouts and DRC files inside this folder. Selecting a folder does not index or save files.")));
        panel.setPrompt(Some(ns_string!("Use Folder")));
        panel.setCanChooseFiles(false);
        panel.setCanChooseDirectories(true);
        panel.setAllowsMultipleSelection(false);
        panel.setCanCreateDirectories(false);
        panel.setCanDownloadUbiquitousContents(false);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        if panel.runModal() != NSModalResponseOK {
            return Ok(0);
        }
        let folder = panel
            .URL()
            .filter(|u| u.isFileURL())
            .and_then(|u| u.path())
            .ok_or_else(|| Error::input("no local working folder selected"))?;
        session.set_initial_directory(Path::new(&folder.to_string()))?;
    }
    let host = Host::new(
        mtm,
        Service::start(session)?,
        smoke,
        smoke_notices,
        smoke_recovery,
        smoke_review,
    );
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
    if smoke_review {
        // Programmatic synthetic QA must not steal the user's typing focus.
        // The window/WebView remains real and visible, behind existing windows.
        window.orderBack(None);
    } else {
        window.makeKeyAndOrderFront(None);
    }
    host.ivars().window.set(window).unwrap();
    app.setDelegate(Some(ProtocolObject::from_ref(&*host)));
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let menu = NSMenu::new(mtm);
    let item = NSMenuItem::new(mtm);
    let submenu = NSMenu::new(mtm);
    // SAFETY: terminate: is NSApplication's standard action; target is resolved
    // by AppKit. Our applicationShouldTerminate: always guards it.
    unsafe {
        let about = submenu.addItemWithTitle_action_keyEquivalent(
            ns_string!("About floe2…"),
            Some(sel!(showAbout:)),
            ns_string!(""),
        );
        about.setTarget(Some(&host));
        submenu.addItem(&NSMenuItem::separatorItem(mtm));
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
    let file_item = NSMenuItem::new(mtm);
    let file = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("File"));
    unsafe {
        let open = file.addItemWithTitle_action_keyEquivalent(
            ns_string!("Open Layout…"),
            Some(sel!(openLayout:)),
            ns_string!("o"),
        );
        open.setTarget(Some(&host));
        let drc = file.addItemWithTitle_action_keyEquivalent(
            ns_string!("Open DRC Results…"),
            Some(sel!(openDrc:)),
            ns_string!(""),
        );
        drc.setTarget(Some(&host));
        file.addItem(&NSMenuItem::separatorItem(mtm));
        file.addItemWithTitle_action_keyEquivalent(
            ns_string!("Close Window…"),
            Some(sel!(performClose:)),
            ns_string!("w"),
        );
    }
    file_item.setSubmenu(Some(&file));
    menu.addItem(&file_item);
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
    let window_item = NSMenuItem::new(mtm);
    let window_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Window"));
    unsafe {
        window_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Minimize"),
            Some(sel!(performMiniaturize:)),
            ns_string!("m"),
        );
        window_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Bring All to Front"),
            Some(sel!(arrangeInFront:)),
            ns_string!(""),
        );
    }
    window_item.setSubmenu(Some(&window_menu));
    menu.addItem(&window_item);
    app.setWindowsMenu(Some(&window_menu));
    app.setMainMenu(Some(&menu));
    let timer_host = host.clone();
    let block = RcBlock::new(move |_: NonNull<NSTimer>| timer_host.poll());
    // SAFETY: Timer scheduled and invalidated on this same main thread. The
    // block retains host; it never transfers UI objects to the service thread.
    let timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.05, true, &block) };
    if !smoke_review {
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
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
        if result != 0 || host.ivars().smoke_step.get() != 8 {
            return Err(Error::input(
                "native smoke did not complete confirmed shutdown",
            ));
        }
        if smoke_review {
            println!("DESKTOP REVIEW RECOVERY: OK (synthetic-only; dropped save ACKs; authenticated reloads; no automatic POST replay; explicit identical receipt resolution; UI read-back; native close/cancel/quit; service joined)");
        } else {
            println!("DESKTOP SMOKE: OK (WebKit auth; synthetic composition-key guard; native menu About + modal guard; native close→cancel; application quit→confirm; service joined)");
        }
        if smoke_notices {
            println!(
                "DESKTOP NOTICE UI: OK (packaged list; verified text read; next catalogue page)"
            );
        }
        if smoke_recovery {
            println!("DESKTOP RECOVERY: OK (explicit root GET; retained session storage; retired navigation ignored; new authenticated document; no bootstrap replay)");
            println!("DESKTOP CLOSE TIMEOUT: OK (empty workspace; omitted JS close completion; real 5 s timer; duplicate requests bounded; native cancel-default sheet cancelled; session preserved; normal close still works)");
        }
    }
    Ok(result)
}
