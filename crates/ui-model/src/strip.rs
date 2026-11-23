use crate::caption::captions;
use crate::device::{DeviceKind, DeviceView};

/// Red threshold used when the low-battery alert is switched off.
const DEFAULT_LOW_PERCENT: u8 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentStyle {
    Normal,
    Low,
    Charging,
    Full,
    Dimmed,
    Placeholder,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub kind: DeviceKind,
    pub caption: Option<String>,
    pub text: String,
    pub style: SegmentStyle,
}

/// One segment per shown device, in the given order; a single placeholder when none qualify.
pub fn build_strip(
    devices: &[DeviceView],
    low_threshold: Option<u8>,
    hidden: &[String],
    show_disconnected: bool,
) -> Vec<Segment> {
    let shown: Vec<(&DeviceView, u8)> = devices
        .iter()
        .filter(|d| !hidden.contains(&d.name) && (d.connected || show_disconnected))
        .filter_map(|d| d.shown_percent().map(|percent| (d, percent)))
        .collect();
    if shown.is_empty() {
        return vec![placeholder()];
    }

    let threshold = low_threshold.unwrap_or(DEFAULT_LOW_PERCENT);
    let device_captions = kind_group_captions(&shown);
    shown
        .iter()
        .zip(device_captions)
        .map(|(&(device, percent), caption)| Segment {
            kind: device.kind,
            caption,
            text: format!("{percent}%"),
            style: style_for(device, percent, threshold),
        })
        .collect()
}

fn placeholder() -> Segment {
    Segment {
        kind: DeviceKind::Other,
        caption: None,
        text: "–".to_owned(),
        style: SegmentStyle::Placeholder,
    }
}

fn style_for(device: &DeviceView, percent: u8, threshold: u8) -> SegmentStyle {
    if !device.connected {
        SegmentStyle::Dimmed
    } else if device.charging && percent >= 100 {
        SegmentStyle::Full
    } else if device.charging {
        SegmentStyle::Charging
    } else if percent <= threshold {
        SegmentStyle::Low
    } else {
        SegmentStyle::Normal
    }
}

/// Captions per kind group, returned in the order of `shown`.
fn kind_group_captions(shown: &[(&DeviceView, u8)]) -> Vec<Option<String>> {
    let mut result = vec![None; shown.len()];
    for (position, (device, _)) in shown.iter().enumerate() {
        let group: Vec<usize> = (0..shown.len())
            .filter(|&i| shown[i].0.kind == device.kind)
            .collect();
        // Fill each group once, from its first member.
        if group[0] != position {
            continue;
        }
        let names: Vec<&str> = group.iter().map(|&i| shown[i].0.name.as_str()).collect();
        for (&index, caption) in group.iter().zip(captions(&names)) {
            result[index] = caption;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Level, test_device};

    fn percent(name: &str, value: u8) -> DeviceView {
        test_device(name, Level::Percent(value))
    }

    fn style_of(device: DeviceView, threshold: Option<u8>) -> SegmentStyle {
        build_strip(&[device], threshold, &[], false)[0].style
    }

    #[test]
    fn strip_low_is_red_at_threshold() {
        assert_eq!(style_of(percent("a", 15), Some(15)), SegmentStyle::Low);
        assert_eq!(style_of(percent("a", 16), Some(15)), SegmentStyle::Normal);
        assert_eq!(style_of(percent("a", 15), None), SegmentStyle::Low);
    }

    #[test]
    fn strip_charging_and_full() {
        let mut charging = percent("a", 80);
        charging.charging = true;
        assert_eq!(style_of(charging, Some(15)), SegmentStyle::Charging);
        let mut full = percent("a", 100);
        full.charging = true;
        assert_eq!(style_of(full, Some(15)), SegmentStyle::Full);
    }

    #[test]
    fn strip_skips_no_data_and_hidden() {
        let devices = [
            percent("shown", 50),
            percent("hidden", 40),
            test_device("nodata", Level::NoData("no battery info".into())),
        ];
        let segments = build_strip(&devices, Some(15), &["hidden".to_owned()], false);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "50%");
    }

    #[test]
    fn strip_disconnected_only_when_enabled() {
        let mut device = percent("a", 40);
        device.connected = false;
        let devices = [device];
        let off = build_strip(&devices, Some(15), &[], false);
        assert_eq!(off.len(), 1);
        assert_eq!(off[0].style, SegmentStyle::Placeholder);
        let on = build_strip(&devices, Some(15), &[], true);
        assert_eq!(on[0].style, SegmentStyle::Dimmed);
        assert_eq!(on[0].text, "40%");
    }

    #[test]
    fn strip_empty_shows_placeholder() {
        let segments = build_strip(&[], Some(15), &[], false);
        assert_eq!(
            segments,
            [Segment {
                kind: DeviceKind::Other,
                caption: None,
                text: "–".to_owned(),
                style: SegmentStyle::Placeholder,
            }]
        );
    }

    #[test]
    fn strip_text_is_percent() {
        let segments = build_strip(&[percent("a", 84)], None, &[], false);
        assert_eq!(segments[0].text, "84%");
    }

    #[test]
    fn strip_captions_only_same_kind_devices() {
        let mut mouse = percent("MX Master", 70);
        mouse.kind = DeviceKind::Mouse;
        let devices = [percent("WH-1000XM4", 50), percent("WH-1000XM5", 60), mouse];
        let segments = build_strip(&devices, None, &[], false);
        assert_eq!(segments[0].caption.as_deref(), Some("XM4"));
        assert_eq!(segments[1].caption.as_deref(), Some("XM5"));
        assert_eq!(segments[2].caption, None);
    }
}
