use std::time::{Duration, SystemTime};

use crate::device::{DeviceKind, DeviceView, Level};

pub const EMPTY_MENU_TEXT: &str = "No devices with battery info";

const NO_BATTERY_TEXT: &str = "no info";

/// A reading older than this is shown with its age even when the last query succeeded.
const OLD_READING: Duration = Duration::from_secs(5 * 60);

/// One menu line: `name — level[ ⚡] · path[ · age][ · note]`; the age shows only for an old
/// reading, and the menu greys a disconnected line out instead of labelling it.
pub fn menu_line(d: &DeviceView, now: SystemTime) -> String {
    let mut level = battery_text(&d.level);
    if d.charging && d.shown_percent().is_some() {
        level.push_str(" ⚡");
    }
    let mut parts = vec![level, d.path.to_string()];
    // Only a connected device's reading time is reliable; the OS gives no last-seen time otherwise.
    if d.connected && d.shown_percent().is_some() {
        let age = now.duration_since(d.updated_at).unwrap_or_default();
        if d.stale || age > OLD_READING {
            parts.push(age_text(age));
        }
    }
    parts.extend(d.note.clone());
    format!("{} — {}", d.name, parts.join(" · "))
}

/// Devices with a number grouped under a kind header (`name 60%` indented two spaces), kinds in
/// a fixed order and devices in input order; whole lines are dropped from the end to fit `max_utf16`.
pub fn tooltip_compact(devices: &[DeviceView], max_utf16: usize) -> String {
    const GROUPS: [(DeviceKind, &str); 6] = [
        (DeviceKind::Headset, "Headset"),
        (DeviceKind::Earbuds, "Earbuds"),
        (DeviceKind::Mouse, "Mouse"),
        (DeviceKind::Keyboard, "Keyboard"),
        (DeviceKind::Gamepad, "Controller"),
        (DeviceKind::Other, "Other"),
    ];
    let mut text = String::new();
    let mut used = 0;
    for (kind, header) in GROUPS {
        let mut header_pending = true;
        for d in devices.iter().filter(|d| d.kind == kind) {
            let Some(percent) = d.shown_percent() else {
                continue;
            };
            let line = format!("  {} {percent}%", d.name);
            // Header and its first item must fit together, so no header is left without items.
            let mut add = String::new();
            if header_pending {
                add.push_str(header);
                add.push('\n');
            }
            add.push_str(&line);
            let cost = add.encode_utf16().count() + usize::from(!text.is_empty());
            if used + cost > max_utf16 {
                return text;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&add);
            used += cost;
            header_pending = false;
        }
    }
    text
}

fn battery_text(level: &Level) -> String {
    match level {
        Level::Percent(percent) => format!("{percent}%"),
        Level::Earbuds { left, right, case } => {
            let parts: Vec<String> = [("L", left), ("R", right), ("case", case)]
                .into_iter()
                .filter_map(|(label, value)| value.map(|v| format!("{label} {v}%")))
                .collect();
            if parts.is_empty() {
                NO_BATTERY_TEXT.to_owned()
            } else {
                parts.join(" · ")
            }
        }
        Level::NoData(message) => message.clone(),
    }
}

fn age_text(age: Duration) -> String {
    match age.as_secs() {
        secs @ 0..3600 => format!("{}m ago", (secs / 60).max(1)),
        secs @ 3600..86400 => format!("{}h ago", secs / 3600),
        secs => format!("{}d ago", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Path, test_device};

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    fn updated_ago(mut device: DeviceView, secs: u64) -> DeviceView {
        device.updated_at = now() - Duration::from_secs(secs);
        device
    }

    #[test]
    fn menu_line_formats() {
        let atom = updated_ago(test_device("FIIL Atom", Level::Percent(84)), 3600 * 2);
        assert_eq!(menu_line(&atom, now()), "FIIL Atom — 84% · BT · 2h ago");

        let fresh = updated_ago(test_device("FIIL Atom", Level::Percent(84)), 120);
        assert_eq!(menu_line(&fresh, now()), "FIIL Atom — 84% · BT");

        let mut charging = updated_ago(test_device("Pad", Level::Percent(52)), 10);
        charging.charging = true;
        assert_eq!(menu_line(&charging, now()), "Pad — 52% ⚡ · BT");

        let days = updated_ago(test_device("Old", Level::Percent(9)), 3 * 86400);
        assert_eq!(menu_line(&days, now()), "Old — 9% · BT · 3d ago");

        let pods = updated_ago(
            test_device(
                "AirPods",
                Level::Earbuds {
                    left: Some(71),
                    right: None,
                    case: Some(54),
                },
            ),
            10,
        );
        assert_eq!(menu_line(&pods, now()), "AirPods — L 71% · case 54% · BT");

        let silent = test_device("X", Level::NoData(NO_BATTERY_TEXT.into()));
        assert_eq!(menu_line(&silent, now()), "X — no info · BT");

        let mut away = updated_ago(test_device("Mouse", Level::Percent(40)), 3 * 3600);
        away.connected = false;
        away.path = Path::Dongle;
        assert_eq!(menu_line(&away, now()), "Mouse — 40% · 2.4G");
    }

    #[test]
    fn menu_line_appends_note_after_age() {
        let mut mouse = updated_ago(test_device("Mouse", Level::Percent(40)), 180);
        mouse.path = Path::Dongle;
        mouse.stale = true;
        mouse.note = Some("sleep".to_owned());
        assert_eq!(
            menu_line(&mouse, now()),
            "Mouse — 40% · 2.4G · 3m ago · sleep"
        );
    }

    fn kinded(name: &str, kind: DeviceKind, percent: u8) -> DeviceView {
        let mut device = test_device(name, Level::Percent(percent));
        device.kind = kind;
        device
    }

    #[test]
    fn tooltip_compact_groups_in_fixed_order_and_omits_empty() {
        let devices = [
            kinded("Pad", DeviceKind::Gamepad, 10),
            kinded("X1", DeviceKind::Mouse, 52),
            kinded("N9", DeviceKind::Headset, 40),
            kinded("V9", DeviceKind::Headset, 90),
            test_device("Silent", Level::NoData("no info".into())),
        ];
        assert_eq!(
            tooltip_compact(&devices, 127),
            "Headset\n  N9 40%\n  V9 90%\nMouse\n  X1 52%\nController\n  Pad 10%"
        );
    }

    #[test]
    fn tooltip_compact_drops_whole_lines_over_the_cap() {
        let devices = [
            kinded("N9", DeviceKind::Headset, 40),
            kinded("V9", DeviceKind::Headset, 90),
            kinded("X1", DeviceKind::Mouse, 52),
        ];
        let headset = "Headset\n  N9 40%\n  V9 90%";
        assert_eq!(tooltip_compact(&devices, headset.len()), headset);
        // The Mouse header and its item do not fit together: neither is emitted.
        assert_eq!(tooltip_compact(&devices, headset.len() + 5), headset);
        // Only the first Headset item fits.
        assert_eq!(tooltip_compact(&devices, 16), "Headset\n  N9 40%");
        assert_eq!(tooltip_compact(&devices, 5), "");
        let full = tooltip_compact(&devices, 127);
        assert!(full.ends_with("Mouse\n  X1 52%"));
    }

    #[test]
    fn tooltip_compact_empty_without_numbers() {
        let devices = [test_device("X", Level::NoData("no info".into()))];
        assert_eq!(tooltip_compact(&devices, 127), "");
    }
}
