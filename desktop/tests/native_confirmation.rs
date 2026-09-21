//! Explicit AppKit test, never part of ordinary cargo test/headless validation.
//! Uses the production helper, isolated empty windows, no WebView/files/service.
#[cfg(target_os = "macos")]
#[path = "../src/confirmation.rs"]
mod confirmation;

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("native confirmation QA requires macOS; not tested");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() {
    use block2::RcBlock;
    use objc2::MainThreadOnly;
    use objc2_app_kit::*;
    use objc2_foundation::*;
    use std::{cell::Cell, ptr::NonNull, rc::Rc};

    fn stop(app: &NSApplication) {
        app.stop(None);
        if let Some(event) = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined, NSPoint::ZERO, NSEventModifierFlags::empty(),
            0.0, 0, None, 0, 0, 0,
        ) { app.postEvent_atStart(&event, true); }
    }

    assert_eq!(
        std::env::args().len(),
        1,
        "no paths, PIDs or input accepted"
    );
    let mtm = MainThreadMarker::new().expect("native QA main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.finishLaunching();
    for (label, key) in [
        ("no key", None),
        ("Return", Some(("\r", 36))),
        ("Enter", Some(("\u{3}", 76))),
    ] {
        // SAFETY: AppKit objects and blocks stay on the main thread. This new
        // window is retained through completion and never releases on close.
        let window = unsafe {
            let w = NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::ZERO, NSSize::new(520.0, 200.0)),
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            );
            w.setReleasedWhenClosed(false);
            w
        };
        window.setTitle(ns_string!("floe synthetic Cancel-key test"));
        window.center();
        window.orderBack(None); // No global key event or user focus takeover.
        let alert = NSAlert::new(mtm);
        alert.setMessageText(ns_string!("Synthetic confirmation"));
        alert.setInformativeText(ns_string!("No design, file, service or user data is used."));
        let cancel = alert.addButtonWithTitle(ns_string!("Cancel"));
        let accept = alert.addButtonWithTitle(ns_string!("End Session"));
        alert.layout();
        cancel.setKeyEquivalent(ns_string!("\r"));
        accept.setKeyEquivalent(ns_string!(""));
        alert.window().setInitialFirstResponder(Some(&cancel));
        let outcome = Rc::new(Cell::new(None));
        let expired = Rc::new(Cell::new(false));
        let reply = outcome.clone();
        let callback_app = app.clone();
        let callback = RcBlock::new(move |result: NSModalResponse| {
            reply.set(Some(result));
            stop(&callback_app);
        });
        alert.beginSheetModalForWindow_completionHandler(&window, Some(&callback));
        assert!(confirmation::set_cancel_default(&alert, &cancel));
        assert!(
            accept.keyEquivalent().is_empty(),
            "accept must have no Return equivalent"
        );
        let sheet = alert.window();
        let dispatch = RcBlock::new(move |_: NonNull<NSTimer>| {
            let Some((text, code)) = key else {
                return;
            };
            let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                NSEventType::KeyDown, NSPoint::ZERO, NSEventModifierFlags::empty(),
                0.0, sheet.windowNumber(), None, &NSString::from_str(text),
                &NSString::from_str(text), false, code,
            ).expect("synthetic sheet key event");
            sheet.sendEvent(&event);
        });
        let deadline_app = app.clone();
        let timeout = expired.clone();
        let deadline = RcBlock::new(move |_: NonNull<NSTimer>| {
            timeout.set(true);
            stop(&deadline_app);
        });
        // SAFETY: Both timers run on this main thread and are invalidated before
        // disposing the sheet. Deadline cleanup cannot count as key success.
        let send_timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.15, false, &dispatch)
        };
        let limit_timer =
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(3.0, false, &deadline) };
        app.run();
        send_timer.invalidate();
        limit_timer.invalidate();
        // Capture the verdict BEFORE cleanup: ending a timed-out sheet must
        // never turn the missing-key negative control into a successful key.
        let passed = if key.is_some() {
            !expired.get() && outcome.get() == Some(NSAlertFirstButtonReturn)
        } else {
            expired.get() && outcome.get().is_none()
        };
        if outcome.get().is_none() {
            window.endSheet_returnCode(&alert.window(), NSAlertFirstButtonReturn);
        }
        window.orderOut(None);
        assert!(
            passed,
            "{label} did not produce the expected native sheet result"
        );
        println!("NATIVE CONFIRMATION: {label} passed; accept action not invoked");
    }
    println!("NATIVE CONFIRMATION: OK (Return/Enter; production helper; not physical keyboard or Escape acceptance)");
}
