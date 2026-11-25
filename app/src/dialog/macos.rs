use std::io;
use std::process::Command;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSApplication, NSTextField};
use objc2_foundation::{NSSize, NSString};
use ui_model::IconColor;

use crate::config::{hex, parse_color};

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

const COLOR_PROMPT: &str = "Type a colour as #RRGGBB, for example #FF8800.";
const COLOR_HINT: &str = "That is not a colour. Type #RRGGBB, for example #FF8800.";
const COLOR_TITLE: &str = "Custom icon colour";

/// `osascript`'s error text when the owner closes the colour panel with Cancel.
const USER_CANCELED: &str = "(-128)";

/// The system colour panel seeded with `initial`; `None` on Cancel. When `osascript` cannot run
/// or answers something unreadable, a `#RRGGBB` prompt asks instead.
pub fn pick_color(initial: (u8, u8, u8)) -> Option<(u8, u8, u8)> {
    match run_color_panel(initial) {
        Ok(picked) => picked,
        Err(error) => {
            eprintln!("colour panel unavailable ({error}); asking for a hex colour");
            super::prompt_parsed(
                COLOR_TITLE,
                COLOR_PROMPT,
                &format!("#{}", hex(initial.0, initial.1, initial.2)),
                parse_hex_color,
            )
        }
    }
}

/// `choose color` floats above other windows by itself, so the app needs no activation first.
/// `Ok(None)` is Cancel; any other failure is an error.
// ponytail: `Command::output()` blocks the event loop while the panel is open (tray frozen,
// like a modal); upgrade path = run it on a thread and post a UserEvent with the result.
fn run_color_panel((red, green, blue): (u8, u8, u8)) -> io::Result<Option<(u8, u8, u8)>> {
    // AppleScript colours are 16-bit per channel; 257 maps 0..=255 onto 0..=65535.
    let script = format!(
        "choose color default color {{{}, {}, {}}}",
        u16::from(red) * 257,
        u16::from(green) * 257,
        u16::from(blue) * 257
    );
    let output = Command::new("osascript").args(["-e", &script]).output()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        return parse_rgb16(&text).map(Some).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unreadable colour {text:?}"),
            )
        });
    }
    let error = String::from_utf8_lossy(&output.stderr);
    if error.contains(USER_CANCELED) {
        Ok(None)
    } else {
        Err(io::Error::other(error.trim().to_owned()))
    }
}

/// `r, g, b` with 16-bit channels, as `choose color` prints them, scaled down to 8 bits.
fn parse_rgb16(text: &str) -> Option<(u8, u8, u8)> {
    let mut channels = text.trim().split(',').map(|part| {
        let wide: u32 = part.trim().parse().ok()?;
        u8::try_from((wide + 128) / 257).ok()
    });
    let colour = (channels.next()??, channels.next()??, channels.next()??);
    channels.next().is_none().then_some(colour)
}

/// Only a `#RRGGBB` colour (the `#` is optional) counts as a custom colour.
fn parse_hex_color(text: &str) -> Result<(u8, u8, u8), String> {
    match parse_color(text.trim()) {
        Some(IconColor::Rgb(red, green, blue)) => Ok((red, green, blue)),
        _ => Err(COLOR_HINT.to_owned()),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb16_output_scales_to_8_bits() {
        assert_eq!(parse_rgb16("65535, 0, 32896\n"), Some((255, 0, 128)));
        assert_eq!(parse_rgb16("0,0,0"), Some((0, 0, 0)));
    }

    #[test]
    fn rgb16_output_rejects_malformed_text() {
        assert_eq!(parse_rgb16("1, 2"), None);
        assert_eq!(parse_rgb16("1, 2, 3, 4"), None);
        assert_eq!(parse_rgb16("red, 0, 0"), None);
        assert_eq!(parse_rgb16("70000, 0, 0"), None);
        assert_eq!(parse_rgb16(""), None);
    }
}
