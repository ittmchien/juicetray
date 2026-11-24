//! Native dialogs opened from a menu click: a one-line text prompt, the SVG file picker and, on
//! Windows, the colour picker. All run modally on the main thread and return when the owner
//! closes them.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use std::path::PathBuf;

#[cfg(target_os = "macos")]
use self::macos as platform;
#[cfg(windows)]
use self::windows as platform;

/// Asks for one line of text pre-filled with `default`; `None` when the owner cancels.
pub fn prompt_text(title: &str, message: &str, default: &str) -> Option<String> {
    platform::prompt_text(title, message, default)
}

/// Asks until `parse` accepts the text or the owner cancels; a rejected entry is shown again
/// with `parse`'s error as the message.
pub fn prompt_parsed<T>(
    title: &str,
    message: &str,
    default: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Option<T> {
    let mut shown_message = message.to_owned();
    let mut text = default.to_owned();
    loop {
        text = prompt_text(title, &shown_message, &text)?;
        match parse(&text) {
            Ok(value) => return Some(value),
            Err(error) => shown_message = error,
        }
    }
}

/// The system colour dialog seeded with `initial` as `(red, green, blue)`; `None` when
/// cancelled. macOS has no such dialog here and asks for a hex colour instead.
#[cfg(windows)]
pub fn pick_color(initial: (u8, u8, u8)) -> Option<(u8, u8, u8)> {
    platform::pick_color(initial)
}

/// The owner's choice of one `.svg` file; `None` when cancelled.
pub fn pick_svg() -> Option<PathBuf> {
    // An accessory app must come to the front first, or the panel opens behind other windows.
    #[cfg(target_os = "macos")]
    macos::activate_app();
    rfd::FileDialog::new()
        .set_title("Choose an SVG icon")
        .add_filter("SVG image", &["svg"])
        .pick_file()
}
