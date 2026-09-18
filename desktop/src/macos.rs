//! The only native FFI boundary. All AppKit/WebKit objects stay on the main
//! thread; delegates and completion blocks are retained through their use.
use crate::service::Service;
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
use std::ptr::NonNull;
use std::time::{Duration, Instant};

const CLOSE_SCRIPT: &str =
    "(()=>{const b=document.getElementById('logout');if(b&&!b.disabled){b.click();}})()";

struct State {
    service: RefCell<Service>,
    origin: OnceCell<String>,
    window: OnceCell<Retained<NSWindow>>,
    web: OnceCell<Retained<WKWebView>>,
    failure: RefCell<Option<&'static str>>,
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
            _web: &WKWebView,
            action: &WKNavigationAction,
            decision: &DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            // SAFETY: Callback arguments are live WebKit objects, only queried
            // on this main-thread delegate; completion is called exactly once.
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
            _web: &WKWebView,
            response: &WKNavigationResponse,
            decision: &DynBlock<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            // D1 never starts native downloads. A response that WebKit cannot
            // display is rejected instead of silently writing to Downloads.
            let allow = unsafe { response.canShowMIMEType() };
            decision.call((if allow {
                WKNavigationResponsePolicy::Allow
            } else {
                WKNavigationResponsePolicy::Cancel
            },));
        }
        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn failed_load(&self, _web: &WKWebView, _nav: Option<&WKNavigation>, _error: &NSError) {
            // NSError can contain the bootstrap URL: never print it.
            self.fail("WebView load failed; no request was replayed");
        }
        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn web_crashed(&self, _web: &WKWebView) {
            // Keep drafts/window visible; never auto-reload/replay writes.
            self.fail("WebView process ended; use Ctrl+C to stop the service");
        }
    }
    unsafe impl WKUIDelegate for Host {
        // Omitting createWebView/runOpenPanel denies popups and file uploads
        // per WKUIDelegate's documented defaults. D2 adds explicit dialogs.
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
    fn new(mtm: MainThreadMarker, service: Service, smoke: bool) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(State {
            service: RefCell::new(service),
            origin: OnceCell::new(),
            window: OnceCell::new(),
            web: OnceCell::new(),
            failure: RefCell::new(None),
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
        if let Some(window) = self.ivars().window.get() {
            window.setTitle(&NSString::from_str(message));
        }
        if self.ivars().smoke {
            self.ivars().service.borrow().cancel();
        }
    }
    fn request_close(&self) {
        if self.ivars().smoke {
            eprintln!("[desktop-smoke] native close callback");
        }
        if let Some(web) = self.ivars().web.get() {
            // Only opens the existing cancel-default dialog. It cannot approve
            // a save, select a file, or authorize the DELETE by itself.
            unsafe {
                web.evaluateJavaScript_completionHandler(&NSString::from_str(CLOSE_SCRIPT), None);
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
        window.setTitle(ns_string!(
            "floe2 — embedded preview (file transfers pending)"
        ));
        Ok(())
    }
    fn poll(&self) {
        let ready = self.ivars().service.borrow().ready.try_recv();
        if let Ok(ready) = ready {
            if self.load(ready).is_err() {
                self.fail("cannot create embedded view");
                self.ivars().service.borrow().cancel();
            }
        }
        if self.ivars().service.borrow().finished() {
            // NSApp.stop alone does not wake its blocking event read.
            let app = NSApplication::sharedApplication(self.mtm());
            app.stop(None);
            if let Some(event) = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
                NSEventType::ApplicationDefined, NSPoint::ZERO, NSEventModifierFlags::empty(), 0.0, 0, None, 0, 0, 0) {
                app.postEvent_atStart(&event, true);
            }
        } else if self.ivars().smoke {
            self.smoke_tick();
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
        submenu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Quit floe2"),
            Some(sel!(terminate:)),
            ns_string!("q"),
        );
    }
    item.setSubmenu(Some(&submenu));
    menu.addItem(&item);
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
