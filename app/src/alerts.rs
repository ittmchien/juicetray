use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use notify_rust::Notification;
use ui_model::DeviceView;

/// Readings older than this say nothing about the battery now, so they never alert.
const MAX_READING_AGE: Duration = Duration::from_secs(15 * 60);

/// Remembers each device's last reading so a low-battery alert fires once per drop.
#[derive(Default)]
pub struct AlertState {
    last_percent: HashMap<String, u8>,
}

impl AlertState {
    /// True when the device just crossed to or below `threshold` while connected and not charging.
    /// Readings without a number, or older than `MAX_READING_AGE`, are ignored: a gap never
    /// re-arms or fires an alert and a stale reading never changes the remembered state.
    pub fn check(&mut self, device: &DeviceView, threshold: Option<u8>) -> bool {
        self.check_at(device, threshold, SystemTime::now())
    }

    fn check_at(&mut self, device: &DeviceView, threshold: Option<u8>, now: SystemTime) -> bool {
        // A reading stamped in the future counts as fresh.
        let age = now.duration_since(device.updated_at).unwrap_or_default();
        if age > MAX_READING_AGE {
            return false;
        }
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
    use ui_model::{DeviceKind, Level, Path};

    use super::*;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    fn device(level: Level) -> DeviceView {
        DeviceView {
            id: "bt:AA".to_owned(),
            name: "FIIL Atom".to_owned(),
            kind: DeviceKind::Headset,
            path: Path::Bluetooth,
            level,
            charging: false,
            connected: true,
            updated_at: now(),
            stale: false,
            note: None,
        }
    }

    fn no_data() -> DeviceView {
        device(Level::NoData("no battery info".into()))
    }

    #[test]
    fn alert_once_per_drop() {
        let mut state = AlertState::default();
        let at = |state: &mut AlertState, percent| {
            state.check_at(&device(Level::Percent(percent)), Some(15), now())
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
        assert!(!state.check_at(&device(Level::Percent(20)), Some(15), now()));
        assert!(!state.check_at(&no_data(), Some(15), now()));
        assert!(state.check_at(&device(Level::Percent(14)), Some(15), now()));
        assert!(!state.check_at(&no_data(), Some(15), now()));
        assert!(!state.check_at(&device(Level::Percent(13)), Some(15), now()));
    }

    #[test]
    fn no_alert_when_charging_disconnected_or_off() {
        let mut state = AlertState::default();
        let mut charging = device(Level::Percent(10));
        charging.charging = true;
        assert!(!state.check_at(&charging, Some(15), now()));

        let mut state = AlertState::default();
        let mut disconnected = device(Level::Percent(10));
        disconnected.connected = false;
        assert!(!state.check_at(&disconnected, Some(15), now()));

        let mut state = AlertState::default();
        assert!(!state.check_at(&device(Level::Percent(10)), None, now()));
    }

    #[test]
    fn alert_ignores_old_reading_but_fires_for_fresh() {
        let mut state = AlertState::default();
        let mut old = device(Level::Percent(10));
        old.updated_at = now() - Duration::from_secs(16 * 60);
        assert!(!state.check_at(&old, Some(15), now()));
        // The old reading left no state, so a fresh one below the threshold still fires.
        let mut fresh = device(Level::Percent(10));
        fresh.updated_at = now() - Duration::from_secs(14 * 60);
        assert!(state.check_at(&fresh, Some(15), now()));
    }
}
