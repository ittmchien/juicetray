use std::time::{Duration, SystemTime};

use crate::device::{DeviceView, Level};

pub const EMPTY_MENU_TEXT: &str = "No devices with battery info";

const NO_BATTERY_TEXT: &str = "no level";

/// One menu line: `name — battery · path · age` (no age when disconnected), then the device's
/// note when it has one; the menu greys a disconnected line out instead of labelling it.
pub fn menu_line(d: &DeviceView, now: SystemTime) -> String {
    let mut parts = vec![battery_text(&d.level), d.path.to_string()];
    // Only a connected device's reading time is reliable; the OS gives no last-seen time otherwise.
    if d.connected && d.shown_percent().is_some() {
        parts.push(age_text(
            now.duration_since(d.updated_at).unwrap_or_default(),
        ));
    }
    parts.extend(d.note.clone());
    format!("{} — {}", d.name, parts.join(" · "))
}

/// `name 60%` per line for devices with a number, cut with `…` to fit `max_utf16` units.
pub fn tooltip_compact(devices: &[DeviceView], max_utf16: usize) -> String {
    let text = devices
        .iter()
        .filter_map(|d| {
            d.shown_percent()
                .map(|percent| format!("{} {percent}%", d.name))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.encode_utf16().count() <= max_utf16 {
        return text;
    }

    // Keep room for the one-unit ellipsis.
    let budget = max_utf16.saturating_sub(1);
    let mut used = 0;
    let mut cut: String = text
        .chars()
        .take_while(|c| {
            used += c.len_utf16();
            used <= budget
        })
        .collect();
    if max_utf16 > 0 {
        cut.push('…');
    }
    cut
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
        0..60 => "just now".to_owned(),
        secs @ 60..3600 => format!("{} min ago", secs / 60),
        secs => format!("{} h ago", secs / 3600),
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
        let atom = updated_ago(test_device("FIIL Atom", Level::Percent(84)), 120);
        assert_eq!(menu_line(&atom, now()), "FIIL Atom — 84% · BT · 2 min ago");

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
        assert_eq!(
            menu_line(&pods, now()),
            "AirPods — L 71% · case 54% · BT · just now"
        );

        let silent = test_device("X", Level::NoData(NO_BATTERY_TEXT.into()));
        assert_eq!(menu_line(&silent, now()), "X — no level · BT");

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
        mouse.note = Some("asleep".to_owned());
        assert_eq!(
            menu_line(&mouse, now()),
            "Mouse — 40% · 2.4G · 3 min ago · asleep"
        );
    }

    #[test]
    fn tooltip_compact_fits() {
        let devices: Vec<_> = (0..10)
            .map(|i| {
                test_device(
                    &format!("A very long device name number {i}"),
                    Level::Percent(50),
                )
            })
            .collect();
        let tooltip = tooltip_compact(&devices, 127);
        assert!(tooltip.encode_utf16().count() <= 127);
        assert!(tooltip.ends_with('…'));
        assert!(tooltip.starts_with("A very long device name number 0 50%\n"));
    }

    #[test]
    fn tooltip_compact_keeps_short_text_whole() {
        let devices = [test_device("Atom", Level::Percent(60))];
        assert_eq!(tooltip_compact(&devices, 127), "Atom 60%");
    }
}
