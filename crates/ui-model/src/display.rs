use crate::device::DeviceKind;

/// Which icon a device shows on the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconChoice {
    /// Resolved to `Kind(device.kind)` when the strip is built.
    Auto,
    Kind(DeviceKind),
    /// Path of the owner's SVG file.
    Svg(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconColor {
    /// Theme ink.
    Auto,
    /// The SVG's own colours; only meaningful for `IconChoice::Svg`.
    Original,
    Rgb(u8, u8, u8),
}

/// Per-device strip options, keyed by device id by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayOptions {
    pub show_percent: bool,
    pub icon: IconChoice,
    pub color: IconColor,
}

impl Default for DisplayOptions {
    fn default() -> Self {
        Self {
            show_percent: true,
            icon: IconChoice::Auto,
            color: IconColor::Auto,
        }
    }
}
