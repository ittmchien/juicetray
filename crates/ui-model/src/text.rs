use std::time::{Duration, SystemTime};

use crate::device::{DeviceView, KIND_GROUPS, Level};

pub const EMPTY_MENU_TEXT: &str = "No devices with battery info";

const NO_BATTERY_TEXT: &str = "no info";

/// A reading older than this is shown with its age even when the last query succeeded; twice the
/// longest query floor (headsets, 5 min), so a device read on schedule never shows an age.
const OLD_READING: Duration = Duration::from_secs(10 * 60);

/// One menu line: `name — path · level[ ⚡][ · age][ · note]`; the age shows only for an old
/// reading, and the menu marks a disconnected line instead of labelling it.
pub fn menu_line(d: &DeviceView, now: SystemTime) -> String {
    let mut level = battery_text(&d.level);
    if d.charging && d.shown_percent().is_some() {
        level.push_str(" ⚡");
    }
    let mut parts = vec![d.path.to_string(), level];
    parts.extend(shown_age(d, now));
    parts.extend(d.note.clone());
    format!("{} — {}", d.name, parts.join(" · "))
}

/// The reading's age when worth showing: stale or older than `OLD_READING`. Only a connected
/// device's reading time is reliable; the OS gives no last-seen time otherwise.
pub fn shown_age(d: &DeviceView, now: SystemTime) -> Option<String> {
    if !d.connected || d.shown_percent().is_none() {
        return None;
    }
    let age = now.duration_since(d.updated_at).unwrap_or_default();
    (d.stale || age > OLD_READING).then(|| age_text(age))
}

/// Devices with a number grouped under a kind header (`name 60%` indented two spaces), kinds in
/// the shared group order and devices in input order. Over `max_utf16`, whole lines are dropped
/// strongest battery first (the latest on a tie), so the weakest devices stay; a header goes
/// with its last line.
pub fn tooltip_compact(devices: &[DeviceView], max_utf16: usize) -> String {
    // (group index, header, line, percent) in display order.
    let items: Vec<(usize, &str, String, u8)> = KIND_GROUPS
        .iter()
        .enumerate()
        .flat_map(|(group, &(kind, header))| {
            devices
                .iter()
                .filter(move |d| d.kind == kind)
                .filter_map(move |d| {
                    let percent = d.shown_percent()?;
                    Some((group, header, format!("  {} {percent}%", d.name), percent))
                })
        })
        .collect();
    let mut kept = vec![true; items.len()];
    loop {
        let mut text = String::new();
        let mut last_group = None;
        for (group, header, line, _) in items.iter().zip(&kept).filter_map(|(i, &k)| k.then_some(i))
        {
            if !text.is_empty() {
                text.push('\n');
            }
            if last_group != Some(*group) {
                text.push_str(header);
                text.push('\n');
                last_group = Some(*group);
            }
            text.push_str(line);
        }
        if text.encode_utf16().count() <= max_utf16 {
            return text;
        }
        // Over budget, so something is still kept.
        let strongest = (0..items.len())
            .filter(|&i| kept[i])
            .max_by_key(|&i| (items[i].3, i));
        let Some(strongest) = strongest else {
            return text;
        };
        kept[strongest] = false;
    }
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

/// `5m ago`, `2h ago`, `3d ago`: a reading's age, at least one minute; shared with the app's menu.
pub fn age_text(age: Duration) -> String {
    match age.as_secs() {
        secs @ 0..3600 => format!("{}m ago", (secs / 60).max(1)),
        secs @ 3600..86400 => format!("{}h ago", secs / 3600),
        secs => format!("{}d ago", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{DeviceKind, Path, test_device};

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
        assert_eq!(menu_line(&atom, now()), "FIIL Atom — BT · 84% · 2h ago");

        let fresh = updated_ago(test_device("FIIL Atom", Level::Percent(84)), 120);
        assert_eq!(menu_line(&fresh, now()), "FIIL Atom — BT · 84%");

        // A headset read on its 5 min floor, a little late, still shows no age.
        let on_floor = updated_ago(test_device("V9", Level::Percent(90)), 5 * 60 + 20);
        assert_eq!(menu_line(&on_floor, now()), "V9 — BT · 90%");

        let mut charging = updated_ago(test_device("Pad", Level::Percent(52)), 10);
        charging.charging = true;
        assert_eq!(menu_line(&charging, now()), "Pad — BT · 52% ⚡");

        let days = updated_ago(test_device("Old", Level::Percent(9)), 3 * 86400);
        assert_eq!(menu_line(&days, now()), "Old — BT · 9% · 3d ago");

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
        assert_eq!(menu_line(&pods, now()), "AirPods — BT · L 71% · case 54%");

        let silent = test_device("X", Level::NoData(NO_BATTERY_TEXT.into()));
        assert_eq!(menu_line(&silent, now()), "X — BT · no info");

        let mut away = updated_ago(test_device("Mouse", Level::Percent(40)), 3 * 3600);
        away.connected = false;
        away.path = Path::Dongle;
        assert_eq!(menu_line(&away, now()), "Mouse — 2.4G · 40%");
    }

    #[test]
    fn menu_line_appends_note_after_age() {
        let mut mouse = updated_ago(test_device("Mouse", Level::Percent(40)), 180);
        mouse.path = Path::Dongle;
        mouse.stale = true;
        mouse.note = Some("sleep".to_owned());
        assert_eq!(
            menu_line(&mouse, now()),
            "Mouse — 2.4G · 40% · 3m ago · sleep"
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
        // The strongest line (V9 90%) goes first, then X1 52% with its header.
        let without_strongest = "Headset\n  N9 40%\nMouse\n  X1 52%";
        assert_eq!(
            tooltip_compact(&devices, without_strongest.len()),
            without_strongest
        );
        assert_eq!(tooltip_compact(&devices, 16), "Headset\n  N9 40%");
        assert_eq!(tooltip_compact(&devices, 5), "");
        let full = tooltip_compact(&devices, 127);
        assert!(full.ends_with("Mouse\n  X1 52%"));
    }

    #[test]
    fn tooltip_compact_keeps_the_weakest_even_in_the_last_group() {
        let devices = [
            kinded("N9", DeviceKind::Headset, 80),
            kinded("V9", DeviceKind::Headset, 90),
            kinded("X1", DeviceKind::Mouse, 70),
            kinded("Pad", DeviceKind::Gamepad, 5),
        ];
        let weakest = "Controller\n  Pad 5%";
        assert_eq!(tooltip_compact(&devices, weakest.len()), weakest);
        // Display order survives: Mouse before Controller.
        let two = "Mouse\n  X1 70%\nController\n  Pad 5%";
        assert_eq!(tooltip_compact(&devices, two.len()), two);
    }

    #[test]
    fn tooltip_compact_empty_without_numbers() {
        let devices = [test_device("X", Level::NoData("no info".into()))];
        assert_eq!(tooltip_compact(&devices, 127), "");
    }
}
