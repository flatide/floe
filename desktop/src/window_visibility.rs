//! Reveal only this host's window after an explicit reopen/close request.
//! No WebView reload, input dispatch, approval, or application-wide activation.
use objc2_app_kit::{
    NSApplication, NSApplicationOcclusionState, NSView, NSWindow, NSWindowOcclusionState,
};

pub fn reveal(window: &NSWindow) {
    window.deminiaturize(None);
    window.makeKeyAndOrderFront(None);
}

/// Read only this host's AppKit state during explicit synthetic QA. This does
/// not order/activate any window or override WebKit's document.hidden policy.
pub fn snapshot(app: &NSApplication, window: &NSWindow, view: &NSView) -> String {
    let bounds = view.bounds().size;
    let visible = view.visibleRect().size;
    let attached = view.window().is_some_and(|w| std::ptr::eq(&*w, window));
    let content = window
        .contentView()
        .is_some_and(|v| std::ptr::eq(&*v, view));
    format!(
        "app_active={} app_hidden={} app_unoccluded={} window_visible={} window_key={} window_main={} window_mini={} window_unoccluded={} view_attached={} content_matches={} view_hidden={} bounds={}x{} visible_rect={}x{}",
        app.isActive(), app.isHidden(), app.occlusionState().contains(NSApplicationOcclusionState::Visible),
        window.isVisible(), window.isKeyWindow(), window.isMainWindow(), window.isMiniaturized(),
        window.occlusionState().contains(NSWindowOcclusionState::Visible), attached, content,
        view.isHiddenOrHasHiddenAncestor(), bounds.width, bounds.height, visible.width, visible.height
    )
}
