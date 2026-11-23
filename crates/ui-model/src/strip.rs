use crate::caption::captions;
use crate::device::{DeviceKind, DeviceView};
use crate::display::{DisplayOptions, IconChoice, IconColor};

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
    pub caption: Option<String>,
    pub text: String,
    pub style: SegmentStyle,
    /// Never `Auto`: resolved to `Kind(kind)` when the strip is built.
    pub icon: IconChoice,
    /// `Original` only with an SVG icon.
    pub color: IconColor,
    /// Battery level to draw; `None` only for the placeholder.
    pub gauge: Option<u8>,
}

/// One segment per shown device, in the given order; a single placeholder when none qualify.
/// `options` returns the display options of a device id.
pub fn build_strip(
    devices: &[DeviceView],
    low_threshold: Option<u8>,
    hidden: &[String],
    show_disconnected: bool,
    options: &dyn Fn(&str) -> DisplayOptions,
) -> Vec<Segment> {
    let shown: Vec<(&DeviceView, u8, DisplayOptions)> = devices
        .iter()
        .filter(|d| !hidden.contains(&d.name) && (d.connected || show_disconnected))
        .filter_map(|d| {
            d.shown_percent()
                .map(|percent| (d, percent, resolve_options(d.kind, options(&d.id))))
        })
        .collect();
    if shown.is_empty() {
        return vec![placeholder()];
    }

    let threshold = low_threshold.unwrap_or(DEFAULT_LOW_PERCENT);
    let device_captions = icon_group_captions(&shown);
    shown
        .into_iter()
        .zip(device_captions)
        .map(|((device, percent, options), caption)| Segment {
            caption,
            text: if options.show_percent {
                format!("{percent}%")
            } else {
                String::new()
            },
            style: style_for(device, percent, threshold),
            icon: options.icon,
            color: options.color,
            gauge: Some(percent),
        })
        .collect()
}

/// Replaces `Auto` icon by the device kind and drops `Original` colour without an SVG.
fn resolve_options(kind: DeviceKind, options: DisplayOptions) -> DisplayOptions {
    let icon = match options.icon {
        IconChoice::Auto => IconChoice::Kind(kind),
        chosen => chosen,
    };
    let color = match (&icon, options.color) {
        (IconChoice::Svg(_), color) => color,
        (_, IconColor::Original) => IconColor::Auto,
        (_, color) => color,
    };
    DisplayOptions {
        icon,
        color,
        ..options
    }
}

fn placeholder() -> Segment {
    Segment {
        caption: None,
        text: "–".to_owned(),
        style: SegmentStyle::Placeholder,
        icon: IconChoice::Kind(DeviceKind::Other),
        color: IconColor::Auto,
        gauge: None,
    }
}

fn style_for(device: &DeviceView, percent: u8, threshold: u8) -> SegmentStyle {
    // A disconnected or stale reading is dimmed, never alarming.
    if !device.connected || device.stale {
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

/// Captions per group of devices sharing the same resolved icon, in the order of `shown`.
fn icon_group_captions(shown: &[(&DeviceView, u8, DisplayOptions)]) -> Vec<Option<String>> {
    let mut result = vec![None; shown.len()];
    for (position, (_, _, options)) in shown.iter().enumerate() {
        let group: Vec<usize> = (0..shown.len())
            .filter(|&i| shown[i].2.icon == options.icon)
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
    use crate::display::{DisplayOptions, IconChoice, IconColor};

    fn default_options(_id: &str) -> DisplayOptions {
        DisplayOptions::default()
    }

    fn one(device: DeviceView, options: DisplayOptions) -> Segment {
        build_strip(&[device], None, &[], false, &move |_| options.clone()).remove(0)
    }

    fn percent(name: &str, value: u8) -> DeviceView {
        test_device(name, Level::Percent(value))
    }

    fn style_of(device: DeviceView, threshold: Option<u8>) -> SegmentStyle {
        build_strip(&[device], threshold, &[], false, &default_options)[0].style
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
        let segments = build_strip(
            &devices,
            Some(15),
            &["hidden".to_owned()],
            false,
            &default_options,
        );
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "50%");
    }

    #[test]
    fn strip_disconnected_only_when_enabled() {
        let mut device = percent("a", 40);
        device.connected = false;
        let devices = [device];
        let off = build_strip(&devices, Some(15), &[], false, &default_options);
        assert_eq!(off.len(), 1);
        assert_eq!(off[0].style, SegmentStyle::Placeholder);
        let on = build_strip(&devices, Some(15), &[], true, &default_options);
        assert_eq!(on[0].style, SegmentStyle::Dimmed);
        assert_eq!(on[0].text, "40%");
    }

    #[test]
    fn strip_dims_stale_connected_device() {
        let mut device = percent("a", 40);
        device.stale = true;
        let segments = build_strip(&[device], Some(15), &[], false, &default_options);
        assert_eq!(segments[0].style, SegmentStyle::Dimmed);
        assert_eq!(segments[0].text, "40%");
    }

    #[test]
    fn strip_empty_shows_placeholder() {
        let segments = build_strip(&[], Some(15), &[], false, &default_options);
        assert_eq!(
            segments,
            [Segment {
                caption: None,
                text: "–".to_owned(),
                style: SegmentStyle::Placeholder,
                icon: IconChoice::Kind(DeviceKind::Other),
                color: IconColor::Auto,
                gauge: None,
            }]
        );
    }

    #[test]
    fn strip_text_is_percent() {
        let segments = build_strip(&[percent("a", 84)], None, &[], false, &default_options);
        assert_eq!(segments[0].text, "84%");
    }

    #[test]
    fn strip_captions_only_same_kind_devices() {
        let mut mouse = percent("MX Master", 70);
        mouse.kind = DeviceKind::Mouse;
        let devices = [percent("WH-1000XM4", 50), percent("WH-1000XM5", 60), mouse];
        let segments = build_strip(&devices, None, &[], false, &default_options);
        assert_eq!(segments[0].caption.as_deref(), Some("XM4"));
        assert_eq!(segments[1].caption.as_deref(), Some("XM5"));
        assert_eq!(segments[2].caption, None);
    }

    #[test]
    fn gauge_always_present() {
        let segment = one(percent("a", 40), DisplayOptions::default());
        assert_eq!(segment.gauge, Some(40));
        assert_eq!(segment.text, "40%");
    }

    #[test]
    fn percent_off_hides_text() {
        let options = DisplayOptions {
            show_percent: false,
            ..DisplayOptions::default()
        };
        let segment = one(percent("a", 40), options);
        assert_eq!(segment.text, "");
        assert_eq!(segment.gauge, Some(40));
        assert_eq!(segment.style, SegmentStyle::Normal);
    }

    #[test]
    fn auto_icon_resolves_to_kind() {
        let mut mouse = percent("a", 40);
        mouse.kind = DeviceKind::Mouse;
        assert_eq!(
            one(mouse, DisplayOptions::default()).icon,
            IconChoice::Kind(DeviceKind::Mouse)
        );
    }

    #[test]
    fn original_color_needs_svg() {
        let original = |icon| DisplayOptions {
            icon,
            color: IconColor::Original,
            ..DisplayOptions::default()
        };
        let builtin = one(
            percent("a", 40),
            original(IconChoice::Kind(DeviceKind::Mouse)),
        );
        assert_eq!(builtin.color, IconColor::Auto);
        let svg = one(percent("a", 40), original(IconChoice::Svg("a.svg".into())));
        assert_eq!(svg.color, IconColor::Original);
    }

    #[test]
    fn rgb_color_passes_through() {
        let options = DisplayOptions {
            color: IconColor::Rgb(1, 2, 3),
            ..DisplayOptions::default()
        };
        assert_eq!(
            one(percent("a", 40), options).color,
            IconColor::Rgb(1, 2, 3)
        );
    }

    #[test]
    fn earbuds_gauge_uses_shown_percent() {
        let device = test_device(
            "Pods",
            Level::Earbuds {
                left: Some(71),
                right: None,
                case: Some(54),
            },
        );
        assert_eq!(one(device, DisplayOptions::default()).gauge, Some(71));
    }

    #[test]
    fn options_looked_up_by_device_id() {
        let devices = [percent("a", 40), percent("b", 50)];
        let options = |id: &str| DisplayOptions {
            show_percent: id != "bt:a",
            ..DisplayOptions::default()
        };
        let segments = build_strip(&devices, None, &[], false, &options);
        assert_eq!(segments[0].text, "");
        assert_eq!(segments[1].text, "50%");
    }

    #[test]
    fn captions_group_by_resolved_icon() {
        let mut first = percent("MX Master", 70);
        first.kind = DeviceKind::Mouse;
        let mut second = percent("G Pro", 60);
        second.kind = DeviceKind::Mouse;
        let custom = |id: &str| DisplayOptions {
            icon: if id == "bt:G Pro" {
                IconChoice::Svg("g.svg".into())
            } else {
                IconChoice::Auto
            },
            ..DisplayOptions::default()
        };
        let segments = build_strip(&[first, second], None, &[], false, &custom);
        assert_eq!(segments[0].caption, None);
        assert_eq!(segments[1].caption, None);
    }

    #[test]
    fn auto_and_explicit_same_icon_get_captions() {
        let mut first = percent("Mouse-AB1", 70);
        first.kind = DeviceKind::Mouse;
        let second = percent("Mouse-CD2", 60);
        let options = |id: &str| DisplayOptions {
            icon: if id == "bt:Mouse-CD2" {
                IconChoice::Kind(DeviceKind::Mouse)
            } else {
                IconChoice::Auto
            },
            ..DisplayOptions::default()
        };
        let segments = build_strip(&[first, second], None, &[], false, &options);
        assert_eq!(segments[0].caption.as_deref(), Some("AB1"));
        assert_eq!(segments[1].caption.as_deref(), Some("CD2"));
    }

    #[test]
    fn same_svg_path_gets_captions() {
        let devices = [percent("WH-1000XM4", 50), percent("Pods-AB1", 60)];
        let shared = |_: &str| DisplayOptions {
            icon: IconChoice::Svg("x.svg".into()),
            ..DisplayOptions::default()
        };
        let segments = build_strip(&devices, None, &[], false, &shared);
        assert_eq!(segments[0].caption.as_deref(), Some("XM4"));
        assert_eq!(segments[1].caption.as_deref(), Some("AB1"));
    }
}
