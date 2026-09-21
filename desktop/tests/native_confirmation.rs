//! Explicit AppKit test, never part of ordinary cargo test/headless validation.
//! Uses the production helper, isolated empty windows, no WebView/files/service.
#[cfg(target_os = "macos")]
#[path = "../src/confirmation.rs"]
mod confirmation;
#[cfg(target_os = "macos")]
#[path = "../src/window_visibility.rs"]
mod window_visibility;

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("native confirmation QA requires macOS; not tested");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() {
    use block2::RcBlock;
    use objc2::{MainThreadOnly, Message};
    use objc2_app_kit::*;
    use objc2_foundation::*;
    use std::{cell::Cell, ptr::NonNull, rc::Rc, time::Instant};

    fn stop(app: &NSApplication) {
        app.stop(None);
        if let Some(event) = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined, NSPoint::ZERO, NSEventModifierFlags::empty(),
            0.0, 0, None, 0, 0, 0,
        ) { app.postEvent_atStart(&event, true); }
    }

    fn wait_for(app: &NSApplication, predicate: impl Fn() -> bool + 'static) -> bool {
        let passed = Rc::new(Cell::new(false));
        let result = passed.clone();
        let runner = app.retain();
        let start = Instant::now();
        let poll = RcBlock::new(move |_: NonNull<NSTimer>| {
            if predicate() {
                result.set(true);
                stop(&runner);
            } else if start.elapsed().as_secs_f64() >= 3.0 {
                stop(&runner);
            }
        });
        // AppKit miniaturization may animate. Observe state on its run loop;
        // timing out is failure, not an alternate path that restores the window.
        let timer =
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.05, true, &poll) };
        app.run();
        timer.invalidate();
        passed.get()
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
    // The production reveal operation must restore this owned window without
    // approving/dismissing an existing sheet. No other application is activated.
    let window = unsafe {
        let w = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(520.0, 200.0)),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Miniaturizable,
            NSBackingStoreType::Buffered,
            false,
        );
        w.setReleasedWhenClosed(false);
        w
    };
    window.setTitle(ns_string!("floe synthetic confirmation visibility test"));
    window.center();
    assert!(
        !window.isVisible(),
        "negative control: new window is hidden"
    );
    window_visibility::reveal(&window);
    assert!(
        wait_for(&app, {
            let w = window.clone();
            move || w.isVisible() && !w.isMiniaturized()
        }),
        "hidden window was not revealed"
    );
    window.miniaturize(None);
    assert!(
        wait_for(&app, {
            let w = window.clone();
            move || w.isMiniaturized()
        }),
        "negative control: window must really miniaturize"
    );
    window_visibility::reveal(&window);
    assert!(
        wait_for(&app, {
            let w = window.clone();
            move || w.isVisible() && !w.isMiniaturized()
        }),
        "miniaturized window was not restored"
    );
    let alert = NSAlert::new(mtm);
    alert.setMessageText(ns_string!("Synthetic pending sheet"));
    alert.addButtonWithTitle(ns_string!("Cancel"));
    alert.addButtonWithTitle(ns_string!("End Session"));
    let replied = Rc::new(Cell::new(false));
    let reply = replied.clone();
    let callback = RcBlock::new(move |_: NSModalResponse| reply.set(true));
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&callback));
    window.orderOut(None);
    assert!(
        !window.isVisible(),
        "negative control: sheet parent is hidden"
    );
    window_visibility::reveal(&window);
    assert!(
        wait_for(&app, {
            let w = window.clone();
            move || w.isVisible()
        }),
        "pending-sheet parent was not revealed"
    );
    assert!(window
        .attachedSheet()
        .is_some_and(|s| std::ptr::eq(&*s, &*alert.window())));
    assert!(
        !replied.get(),
        "revealing the parent must not resolve the sheet"
    );
    window.endSheet_returnCode(&alert.window(), NSAlertFirstButtonReturn);
    window.orderOut(None);
    println!("NATIVE VISIBILITY: OK (hidden/minimized owned window; pending sheet preserved; not Dock/physical input acceptance)");
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
