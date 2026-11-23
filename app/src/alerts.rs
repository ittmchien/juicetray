use std::collections::HashMap;

use notify_rust::Notification;
use ui_model::DeviceView;

/// Remembers each device's last reading so a low-battery alert fires once per drop.
#[derive(Default)]
pub struct AlertState {
    last_percent: HashMap<String, u8>,
}

impl AlertState {
    /// True when the device just crossed to or below `threshold` while connected and not charging.
    /// Readings without a number are ignored, so a gap never re-arms or fires an alert.
    pub fn check(&mut self, device: &DeviceView, threshold: Option<u8>) -> bool {
        let Some(percent) = device.shown_percent() else {
            return false;
        };
        let previous = self.last_percent.insert(device.id.clone(), percent);
        let Some(threshold) = threshold else {
            return false;
        };
        device.connected
            && !device.charging
            && percent <= threshold
            && previous.is_none_or(|previous| previous > threshold)
    }
}

/// Posts the notification from a short-lived thread so the event loop never waits on the OS.
pub fn notify_low_battery(device: &DeviceView) {
    let Some(percent) = device.shown_percent() else {
        return;
    };
    let name = device.name.clone();
    let spawned = std::thread::Builder::new()
        .name("notify".into())
        .spawn(move || {
            let shown = Notification::new()
                .summary(&format!("{name} battery low"))
                .body(&format!("{percent}% left"))
                .show();
            if let Err(error) = shown {
                eprintln!("notification failed: {error}");
            }
        });
    if let Err(error) = spawned {
        eprintln!("notification thread failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use ui_model::{DeviceKind, Level, Path};

    use super::*;

    fn device(level: Level) -> DeviceView {
        DeviceView {
            id: "bt:AA".to_owned(),
            name: "FIIL Atom".to_owned(),
            kind: DeviceKind::Headset,
            path: Path::Bluetooth,
            level,
            charging: false,
            connected: true,
            updated_at: SystemTime::UNIX_EPOCH,
        }
    }

    fn no_data() -> DeviceView {
        device(Level::NoData("no battery info".into()))
    }

    #[test]
    fn alert_once_per_drop() {
        let mut state = AlertState::default();
        let at = |state: &mut AlertState, percent| {
            state.check(&device(Level::Percent(percent)), Some(15))
        };
        assert!(!at(&mut state, 20));
        assert!(at(&mut state, 15));
        assert!(!at(&mut state, 14));
        assert!(!at(&mut state, 40));
        assert!(at(&mut state, 10));
    }

    #[test]
    fn alert_ignores_gaps() {
        let mut state = AlertState::default();
        assert!(!state.check(&device(Level::Percent(20)), Some(15)));
        assert!(!state.check(&no_data(), Some(15)));
        assert!(state.check(&device(Level::Percent(14)), Some(15)));
        assert!(!state.check(&no_data(), Some(15)));
        assert!(!state.check(&device(Level::Percent(13)), Some(15)));
    }

    #[test]
    fn no_alert_when_charging_disconnected_or_off() {
        let mut state = AlertState::default();
        let mut charging = device(Level::Percent(10));
        charging.charging = true;
        assert!(!state.check(&charging, Some(15)));

        let mut state = AlertState::default();
        let mut disconnected = device(Level::Percent(10));
        disconnected.connected = false;
        assert!(!state.check(&disconnected, Some(15)));

        let mut state = AlertState::default();
        assert!(!state.check(&device(Level::Percent(10)), None));
    }
}
