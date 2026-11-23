//! The OS UI font and size for the strip's number; anything unusable gives the embedded Inter.

use std::path::Path;

use strip_render::TextStyle;

#[cfg(target_os = "macos")]
pub use mac::menu_bar_text;
#[cfg(windows)]
pub use win::status_text;

/// Maps `font_file` at `size_px`; a size that is not a positive number gives the fallback.
fn text_style(font_file: &Path, size_px: f32, height_px: u32) -> TextStyle {
    if size_px.is_finite() && size_px > 0.0 {
        TextStyle::from_font_file(font_file, size_px, height_px)
    } else {
        eprintln!("OS font size {size_px} unusable; using embedded Inter");
        TextStyle::fallback(height_px)
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::path::Path;

    use objc2_app_kit::NSFont;
    use strip_render::TextStyle;

    /// SF Pro, the menu bar's system font, read at its default instance.
    const SYSTEM_FONT_FILE: &str = "/System/Library/Fonts/SFNS.ttf";
    /// Menu bar thickness in points; the strip bitmap's full height stands for it.
    const MENU_BAR_POINTS: f32 = 22.0;

    /// The menu bar font at its current size, scaled to a strip bitmap `height_px` tall.
    pub fn menu_bar_text(height_px: u32) -> TextStyle {
        // Size 0 asks for the menu bar's default size.
        let points = NSFont::menuBarFontOfSize(0.0).pointSize() as f32;
        let size_px = points * height_px as f32 / MENU_BAR_POINTS;
        super::text_style(Path::new(SYSTEM_FONT_FILE), size_px, height_px)
    }
}

#[cfg(windows)]
mod win {
    use std::env;
    use std::path::PathBuf;

    use strip_render::TextStyle;
    use windows::Win32::UI::HiDpi::SystemParametersInfoForDpi;
    use windows::Win32::UI::WindowsAndMessaging::{NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS};

    const VARIABLE_FACE_PREFIX: &str = "Segoe UI Variable";
    const VARIABLE_FONT_FILE: &str = "SegUIVar.ttf";
    const CLASSIC_FONT_FILE: &str = "segoeui.ttf";
    const DEFAULT_WINDOWS_DIR: &str = r"C:\Windows";

    /// The status-bar font (`lfStatusFont`) at `dpi`: `|lfHeight|` pixels per em.
    pub fn status_text(dpi: u32, height_px: u32) -> TextStyle {
        match status_font(dpi) {
            Ok((face, size_px)) => {
                let file = fonts_dir().join(font_file_for_face(&face));
                super::text_style(&file, size_px, height_px)
            }
            Err(error) => {
                eprintln!("reading the status bar font failed: {error}; using embedded Inter");
                TextStyle::fallback(height_px)
            }
        }
    }

    /// Face name and pixel size of the status-bar font at `dpi`.
    fn status_font(dpi: u32) -> windows::core::Result<(String, f32)> {
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        // SAFETY: `metrics` is writable and its `cbSize` states its own size, as the call requires.
        unsafe {
            SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS.0,
                metrics.cbSize,
                Some((&raw mut metrics).cast()),
                0,
                dpi,
            )
        }?;
        let font = metrics.lfStatusFont;
        let face_end = font
            .lfFaceName
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(font.lfFaceName.len());
        let face = String::from_utf16_lossy(&font.lfFaceName[..face_end]);
        Ok((face, font.lfHeight.unsigned_abs() as f32))
    }

    fn fonts_dir() -> PathBuf {
        env::var_os("SystemRoot")
            .map_or_else(|| PathBuf::from(DEFAULT_WINDOWS_DIR), PathBuf::from)
            .join("Fonts")
    }

    /// Windows 11 uses the variable Segoe UI file; every other face maps to classic Segoe UI.
    fn font_file_for_face(face: &str) -> &'static str {
        if face.starts_with(VARIABLE_FACE_PREFIX) {
            VARIABLE_FONT_FILE
        } else {
            CLASSIC_FONT_FILE
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn face_name_picks_font_file() {
            assert_eq!(font_file_for_face("Segoe UI Variable Text"), "SegUIVar.ttf");
            assert_eq!(font_file_for_face("Segoe UI Variable"), "SegUIVar.ttf");
            assert_eq!(font_file_for_face("Segoe UI"), "segoeui.ttf");
            assert_eq!(font_file_for_face("Tahoma"), "segoeui.ttf");
        }
    }
}

#[cfg(test)]
mod tests {
    use strip_render::FontData;

    use super::*;

    #[test]
    fn bad_size_gives_fallback() {
        for size in [0.0, -12.0, f32::NAN, f32::INFINITY] {
            let style = text_style(Path::new("no/such/font.ttf"), size, 44);
            assert!(matches!(style.font, FontData::Embedded), "{size}");
            assert_eq!(style.size_px, TextStyle::fallback(44).size_px);
        }
    }
}
