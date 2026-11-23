use std::time::SystemTime;

use ui_model::{DeviceView, EMPTY_MENU_TEXT, menu_line};

/// Plain-text device list for `juicetray scan`; device ids stay out of the output.
pub fn scan_report(devices: &[DeviceView], now: SystemTime) -> String {
    let mut lines = vec!["Bluetooth (OS-stored battery)".to_owned()];
    if devices.is_empty() {
        lines.push(EMPTY_MENU_TEXT.to_owned());
    } else {
        lines.extend(devices.iter().map(|device| menu_line(device, now)));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ui_model::{DeviceKind, Level, Path};

    use super::*;

    fn device(id: &str, name: &str, level: Level, now: SystemTime) -> DeviceView {
        DeviceView {
            id: id.to_owned(),
            name: name.to_owned(),
            kind: DeviceKind::Headset,
            path: Path::Bluetooth,
            level,
            charging: false,
            connected: true,
            updated_at: now - Duration::from_secs(120),
        }
    }

    #[test]
    fn scan_report_lists_devices_without_ids() {
        let now = SystemTime::now();
        let devices = [
            device("bt:AA-BB-CC-DD-EE-01", "FIIL Atom", Level::Percent(84), now),
            device(
                "bt:AA-BB-CC-DD-EE-02",
                "X",
                Level::NoData("no battery info".into()),
                now,
            ),
        ];

        let report = scan_report(&devices, now);

        assert!(report.starts_with("Bluetooth (OS-stored battery)"));
        for device in &devices {
            assert!(report.contains(&menu_line(device, now)));
            assert!(!report.contains(&device.id));
        }
        assert!(
            scan_report(&[], now)
                .lines()
                .any(|line| line == EMPTY_MENU_TEXT)
        );
    }
}
