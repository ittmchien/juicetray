//! Windows taskbar strip: pure layout and pixel conversion, plus the Win32 window that shows it.

#[cfg(any(windows, test))]
mod layout;
#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::{Signal, StripContent, TaskbarHandle, describe_environment, windows_build};

/// Straight RGBA to the premultiplied BGRA that `UpdateLayeredWindow` blends.
/// Fully transparent pixels get alpha 1 so the whole strip still takes clicks.
#[cfg(any(windows, test))]
fn premultiplied_bgra([r, g, b, a]: [u8; 4]) -> [u8; 4] {
    if a == 0 {
        return [0, 0, 0, 1];
    }
    let scale = |channel: u8| ((u16::from(channel) * u16::from(a) + 127) / 255) as u8;
    [scale(b), scale(g), scale(r), a]
}

#[cfg(test)]
mod tests {
    use super::premultiplied_bgra;

    #[test]
    fn premultiplies_and_swaps_channels() {
        // 50 % red: straight r = 229 becomes ~115 premultiplied, in BGRA order.
        assert_eq!(premultiplied_bgra([229, 0, 0, 128]), [0, 0, 115, 128]);
        assert_eq!(premultiplied_bgra([10, 20, 30, 255]), [30, 20, 10, 255]);
    }

    #[test]
    fn transparent_pixels_keep_alpha_one() {
        assert_eq!(premultiplied_bgra([200, 100, 50, 0]), [0, 0, 0, 1]);
    }
}
