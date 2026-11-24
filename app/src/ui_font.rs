//! The OS UI font and size for the strip's number. macOS falls back to the embedded Inter;
//! Windows falls back to another system font file, so Inter is not compiled in there.

#[cfg(target_os = "macos")]
pub use mac::menu_bar_text;
#[cfg(windows)]
pub use win::status_text;

#[cfg(target_os = "macos")]
mod mac {
    use std::path::Path;

    use objc2_app_kit::NSFont;
    use strip_render::TextStyle;

    /// Maps `font_file` at `size_px`; a size that is not a positive number gives the fallback.
    pub(super) fn text_style(font_file: &Path, size_px: f32, height_px: u32) -> TextStyle {
        if size_px.is_finite() && size_px > 0.0 {
            TextStyle::from_font_file(font_file, size_px, height_px)
        } else {
            eprintln!("OS font size {size_px} unusable; using embedded Inter");
            TextStyle::fallback(height_px)
        }
    }

    /// SF Pro, the menu bar's system font, read at its default instance.
    const SYSTEM_FONT_FILE: &str = "/System/Library/Fonts/SFNS.ttf";
    /// Menu bar thickness in points; the strip bitmap's full height stands for it.
    const MENU_BAR_POINTS: f32 = 22.0;

    /// The menu bar font at its current size, scaled to a strip bitmap `height_px` tall.
    pub fn menu_bar_text(height_px: u32) -> TextStyle {
        // Size 0 asks for the menu bar's default size.
        let points = NSFont::menuBarFontOfSize(0.0).pointSize() as f32;
        let size_px = points * height_px as f32 / MENU_BAR_POINTS;
        text_style(Path::new(SYSTEM_FONT_FILE), size_px, height_px)
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
    /// Tried in order after the status-bar face's own file.
    const FALLBACK_FONT_FILES: [&str; 3] = [CLASSIC_FONT_FILE, "arial.ttf", "tahoma.ttf"];
    const DEFAULT_WINDOWS_DIR: &str = r"C:\Windows";

    /// The status-bar font (`lfStatusFont`) at `dpi`: `|lfHeight|` pixels per em. A font or size
    /// that cannot be used falls back to the next system font file; with none the strip has no text.
    pub fn status_text(dpi: u32, height_px: u32) -> TextStyle {
        let (face, size_px) = match status_font(dpi) {
            Ok((face, size)) if size.is_finite() && size > 0.0 => (Some(face), size),
            unusable => {
                eprintln!("status bar font unusable ({unusable:?}); using a system font");
                (None, TextStyle::default_size_px(height_px))
            }
        };
        let fonts = fonts_dir();
        let preferred = face.as_deref().map(font_file_for_face);
        for file in font_candidates(preferred) {
            match TextStyle::try_from_font_file(&fonts.join(file), size_px) {
                Ok(style) => return style,
                Err(reason) => eprintln!("font {file} unusable ({reason})"),
            }
        }
        eprintln!("no system font usable; the strip draws no text");
        TextStyle::without_font(size_px)
    }

    /// Font files to try, best first: the status-bar face's own, then the fixed fallbacks.
    fn font_candidates(preferred: Option<&'static str>) -> Vec<&'static str> {
        preferred.into_iter().chain(FALLBACK_FONT_FILES).collect()
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

        #[test]
        fn candidates_start_with_the_face_file() {
            assert_eq!(
                font_candidates(Some("SegUIVar.ttf")),
                ["SegUIVar.ttf", "segoeui.ttf", "arial.ttf", "tahoma.ttf"]
            );
            assert_eq!(
                font_candidates(None),
                ["segoeui.ttf", "arial.ttf", "tahoma.ttf"]
            );
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::path::Path;

    use strip_render::{FontData, TextStyle};

    use super::mac::text_style;

    #[test]
    fn bad_size_gives_fallback() {
        for size in [0.0, -12.0, f32::NAN, f32::INFINITY] {
            let style = text_style(Path::new("no/such/font.ttf"), size, 44);
            assert!(matches!(style.font, FontData::Embedded), "{size}");
            assert_eq!(style.size_px, TextStyle::fallback(44).size_px);
        }
    }
}
