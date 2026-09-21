//! NSAlert may clear Return during presentation even when the default-cell
//! identity and initial responder still point to Cancel. Call AFTER presenting.
use objc2_app_kit::{NSAlert, NSButton, NSButtonCell, NSCell};

pub fn set_cancel_default(alert: &NSAlert, cancel: &NSButton) -> bool {
    let Some(cell) = cancel.cell() else {
        return false;
    };
    let Some(button) = cell.downcast_ref::<NSButtonCell>() else {
        return false;
    };
    let sheet = alert.window();
    sheet.setDefaultButtonCell(Some(button));
    sheet
        .defaultButtonCell()
        .is_some_and(|d| std::ptr::eq(&*d as *const NSButtonCell as *const NSCell, &*cell))
        && cancel.keyEquivalent().to_string() == "\r"
}
