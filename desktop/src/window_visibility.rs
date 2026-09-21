//! Reveal only this host's window after an explicit reopen/close request.
//! No WebView reload, input dispatch, approval, or application-wide activation.
use objc2_app_kit::NSWindow;

pub fn reveal(window: &NSWindow) {
    window.deminiaturize(None);
    window.makeKeyAndOrderFront(None);
}
