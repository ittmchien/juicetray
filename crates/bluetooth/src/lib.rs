//! Bluetooth battery levels the OS already stores; nothing is ever sent to a device.

use ui_model::{DeviceKind, DeviceView};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// All paired/known Bluetooth devices, connected or not; empty on other platforms.
pub fn read_devices() -> anyhow::Result<Vec<DeviceView>> {
    #[cfg(target_os = "macos")]
    return macos::read_devices();
    #[cfg(windows)]
    return windows::read_devices();
    #[cfg(not(any(target_os = "macos", windows)))]
    return Ok(Vec::new());
}

/// A stored battery value; `0` and anything above `100` mean unknown, never `0%`.
pub fn normalize(raw: i64) -> Option<u8> {
    u8::try_from(raw)
        .ok()
        .filter(|percent| (1..=100).contains(percent))
}

/// Splits a raw 24-bit Class of Device into IOBluetooth's `(major, minor)` form.
pub fn split_cod(cod: u32) -> (u32, u32) {
    ((cod >> 8) & 0x1F, (cod >> 2) & 0x3F)
}

/// Device kind from the Bluetooth class (IOBluetooth's split form); earbuds are told by name.
pub fn kind_from_class(major: u32, minor: u32, name: &str) -> DeviceKind {
    let lowercase = name.to_lowercase();
    if ["buds", "airpods", "pods"]
        .iter()
        .any(|marker| lowercase.contains(marker))
    {
        return DeviceKind::Earbuds;
    }
    match (major, minor) {
        (0x04, 0x01 | 0x02 | 0x06) => DeviceKind::Headset,
        (0x05, minor) if matches!(minor & 0x0F, 1 | 2) => DeviceKind::Gamepad,
        (0x05, minor) if minor & 0x30 == 0x20 => DeviceKind::Mouse,
        (0x05, minor) if matches!(minor & 0x30, 0x10 | 0x30) => DeviceKind::Keyboard,
        _ => DeviceKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_bounds() {
        assert_eq!(normalize(0), None);
        assert_eq!(normalize(1), Some(1));
        assert_eq!(normalize(100), Some(100));
        assert_eq!(normalize(101), None);
        assert_eq!(normalize(-1), None);
    }

    #[test]
    fn class_mapping() {
        assert_eq!(kind_from_class(0x04, 0x06, "X"), DeviceKind::Headset);
        assert_eq!(kind_from_class(0x05, 0x20, "X"), DeviceKind::Mouse);
        assert_eq!(kind_from_class(0x05, 0x10, "X"), DeviceKind::Keyboard);
        assert_eq!(kind_from_class(0x05, 0x30, "X"), DeviceKind::Keyboard);
        assert_eq!(kind_from_class(0x05, 0x01, "X"), DeviceKind::Gamepad);
        assert_eq!(kind_from_class(0x05, 0x02, "X"), DeviceKind::Gamepad);
        assert_eq!(kind_from_class(0x01, 0x00, "X"), DeviceKind::Other);
    }

    #[test]
    fn earbuds_by_name() {
        assert_eq!(
            kind_from_class(0x04, 0x06, "AirPods Pro"),
            DeviceKind::Earbuds
        );
    }

    #[test]
    fn split_cod_mouse() {
        let (major, minor) = split_cod(0x002580);
        assert_eq!((major, minor), (0x05, 0x20));
        assert_eq!(kind_from_class(major, minor, "MX"), DeviceKind::Mouse);
    }
}
