use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSApplication, NSTextField};
use objc2_foundation::{NSSize, NSString};

/// Input field size in points; wide enough for a colour or an interval.
const FIELD_SIZE: NSSize = NSSize::new(260.0, 24.0);

/// An `NSAlert` with a text field accessory; OK is the default button, Cancel answers Escape.
pub fn prompt_text(title: &str, message: &str, default: &str) -> Option<String> {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("text prompt skipped: not on the main thread");
        return None;
    };
    activate(mtm);
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(message));
    // Buttons are numbered in the order they are added: OK is the first, Cancel the second.
    alert.addButtonWithTitle(&NSString::from_str("OK"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    let field = NSTextField::textFieldWithString(&NSString::from_str(default), mtm);
    field.setFrameSize(FIELD_SIZE);
    alert.setAccessoryView(Some(&field));
    // Laying out first creates the window, so the field can take the keyboard focus.
    alert.layout();
    alert.window().makeFirstResponder(Some(&field));
    (alert.runModal() == NSAlertFirstButtonReturn).then(|| field.stringValue().to_string())
}

/// An `NSAlert` with `title` in bold, `message` below it and one OK button; skipped (logged)
/// off the main thread.
pub fn notice(title: &str, message: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("notice skipped: not on the main thread");
        return;
    };
    activate(mtm);
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(message));
    alert.runModal();
}

/// Brings the accessory app to the front so its dialogs open above other windows.
pub fn activate_app() {
    match MainThreadMarker::new() {
        Some(mtm) => activate(mtm),
        None => eprintln!("activating the app skipped: not on the main thread"),
    }
}

fn activate(mtm: MainThreadMarker) {
    // `activate()` needs macOS 14; the minimum supported version is 13.
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
}
