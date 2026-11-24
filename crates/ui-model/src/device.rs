use std::fmt;
use std::str::FromStr;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Headset,
    Earbuds,
    Mouse,
    Keyboard,
    Gamepad,
    Other,
}

impl DeviceKind {
    /// Stable lowercase name used in config and menu ids.
    pub fn as_str(self) -> &'static str {
        match self {
            DeviceKind::Headset => "headset",
            DeviceKind::Earbuds => "earbuds",
            DeviceKind::Mouse => "mouse",
            DeviceKind::Keyboard => "keyboard",
            DeviceKind::Gamepad => "gamepad",
            DeviceKind::Other => "other",
        }
    }
}

impl FromStr for DeviceKind {
    type Err = UnknownDeviceKind;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "headset" => Ok(DeviceKind::Headset),
            "earbuds" => Ok(DeviceKind::Earbuds),
            "mouse" => Ok(DeviceKind::Mouse),
            "keyboard" => Ok(DeviceKind::Keyboard),
            "gamepad" => Ok(DeviceKind::Gamepad),
            "other" => Ok(DeviceKind::Other),
            _ => Err(UnknownDeviceKind),
        }
    }
}

/// A string that names no `DeviceKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownDeviceKind;

/// How the OS reaches the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    Bluetooth,
    Dongle,
    Cable,
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            // Short owner-facing labels: 2.4 GHz receiver, USB cable, Bluetooth.
            Path::Bluetooth => "BT",
            Path::Dongle => "2.4G",
            Path::Cable => "USB",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Level {
    Percent(u8),
    Earbuds {
        left: Option<u8>,
        right: Option<u8>,
        case: Option<u8>,
    },
    NoData(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub path: Path,
    pub level: Level,
    pub charging: bool,
    pub connected: bool,
    pub updated_at: SystemTime,
    /// The last attempt failed or the device is asleep; the strip shows it dimmed.
    pub stale: bool,
    /// Short reason shown at the end of the menu line.
    pub note: Option<String>,
}

impl DeviceView {
    /// The single number the strip shows: the weakest present bud, else the case.
    pub fn shown_percent(&self) -> Option<u8> {
        match &self.level {
            Level::Percent(percent) => Some(*percent),
            Level::Earbuds { left, right, case } => {
                [*left, *right].into_iter().flatten().min().or(*case)
            }
            Level::NoData(_) => None,
        }
    }

    /// The one notion of offline: Bluetooth disconnected, or a receiver whose last query
    /// failed or that sleeps, so the shown reading is an old stored one.
    pub fn is_offline(&self) -> bool {
        !self.connected || self.stale
    }

    /// True when the device has been offline longer than `hide_after` (`None` = never hide).
    /// A disconnected device counts as offline for good: Bluetooth `updated_at` is the scan
    /// time, not the last contact, so only a stale receiver's reading age is measured.
    pub fn hidden_when_offline(&self, hide_after: Option<Duration>, now: SystemTime) -> bool {
        let Some(limit) = hide_after else {
            return false;
        };
        if !self.is_offline() {
            return false;
        }
        let age = now.duration_since(self.updated_at).unwrap_or_default();
        !self.connected || age > limit
    }
}

/// Weakest first; devices without a number go last, ties break by name.
pub fn sort_weakest_first(devices: &mut [DeviceView]) {
    devices.sort_by(|a, b| {
        let (a_percent, b_percent) = (a.shown_percent(), b.shown_percent());
        (a_percent.is_none(), a_percent, &a.name).cmp(&(b_percent.is_none(), b_percent, &b.name))
    });
}

/// Display order and label of each kind: the one table behind the strip, menu and tooltip groups.
pub(crate) const KIND_GROUPS: [(DeviceKind, &str); 6] = [
    (DeviceKind::Headset, "Headset"),
    (DeviceKind::Earbuds, "Earbuds"),
    (DeviceKind::Mouse, "Mouse"),
    (DeviceKind::Keyboard, "Keyboard"),
    (DeviceKind::Gamepad, "Controller"),
    (DeviceKind::Other, "Other"),
];

/// Position of the kind's group in the fixed display order; a kind missing from the table sorts last.
pub fn group_order(kind: DeviceKind) -> usize {
    KIND_GROUPS
        .iter()
        .position(|(group, _)| *group == kind)
        .unwrap_or(KIND_GROUPS.len())
}

/// Header text of the kind's group (`Gamepad` shows as "Controller"); `Other` for an unlisted kind.
pub fn group_label(kind: DeviceKind) -> &'static str {
    KIND_GROUPS
        .iter()
        .find(|(group, _)| *group == kind)
        .map_or("Other", |(_, label)| label)
}

/// Grouped by kind in display order, weakest first within a group (the sort is stable).
pub fn sort_grouped(devices: &mut [DeviceView]) {
    sort_weakest_first(devices);
    devices.sort_by_key(|d| group_order(d.kind));
}

#[cfg(test)]
pub(crate) fn test_device(name: &str, level: Level) -> DeviceView {
    DeviceView {
        id: format!("bt:{name}"),
        name: name.to_owned(),
        kind: DeviceKind::Headset,
        path: Path::Bluetooth,
        level,
        charging: false,
        connected: true,
        updated_at: SystemTime::UNIX_EPOCH,
        stale: false,
        note: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn earbuds(left: Option<u8>, right: Option<u8>, case: Option<u8>) -> DeviceView {
        test_device("AirPods", Level::Earbuds { left, right, case })
    }

    #[test]
    fn device_kind_name_roundtrip() {
        for kind in [
            DeviceKind::Headset,
            DeviceKind::Earbuds,
            DeviceKind::Mouse,
            DeviceKind::Keyboard,
            DeviceKind::Gamepad,
            DeviceKind::Other,
        ] {
            assert_eq!(kind.as_str().parse(), Ok(kind));
        }
        assert_eq!("mouse".parse(), Ok(DeviceKind::Mouse));
        assert!("toaster".parse::<DeviceKind>().is_err());
    }

    #[test]
    fn earbuds_one_bud_uses_present_bud() {
        assert_eq!(earbuds(Some(71), None, Some(54)).shown_percent(), Some(71));
    }

    #[test]
    fn earbuds_only_case() {
        assert_eq!(earbuds(None, None, Some(54)).shown_percent(), Some(54));
    }

    #[test]
    fn offline_is_disconnected_or_stale() {
        let online = test_device("a", Level::Percent(50));
        assert!(!online.is_offline());
        let disconnected = DeviceView {
            connected: false,
            ..online.clone()
        };
        assert!(disconnected.is_offline());
        let stale = DeviceView {
            stale: true,
            ..online
        };
        assert!(stale.is_offline());
    }

    #[test]
    fn hidden_when_offline_by_threshold() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let limit = Some(Duration::from_secs(15 * 60));
        let stale_for = |secs| DeviceView {
            stale: true,
            updated_at: now - Duration::from_secs(secs),
            ..test_device("a", Level::Percent(50))
        };
        assert!(!stale_for(14 * 60).hidden_when_offline(limit, now));
        assert!(!stale_for(15 * 60).hidden_when_offline(limit, now));
        assert!(stale_for(15 * 60 + 1).hidden_when_offline(limit, now));
        assert!(!stale_for(9_000).hidden_when_offline(None, now));

        // Disconnected: hidden whenever a threshold is set, whatever `updated_at` says.
        let disconnected = DeviceView {
            connected: false,
            updated_at: now,
            ..test_device("a", Level::Percent(50))
        };
        assert!(disconnected.hidden_when_offline(limit, now));
        assert!(!disconnected.hidden_when_offline(None, now));

        // An online device is never hidden, however old its reading.
        let online = DeviceView {
            updated_at: SystemTime::UNIX_EPOCH,
            ..test_device("a", Level::Percent(50))
        };
        assert!(!online.hidden_when_offline(limit, now));
    }

    #[test]
    fn no_data_has_no_percent() {
        let device = test_device("X", Level::NoData("no battery info".into()));
        assert_eq!(device.shown_percent(), None);
    }

    #[test]
    fn sort_weakest_first_none_last() {
        let mut devices = vec![
            test_device("none", Level::NoData("no battery info".into())),
            test_device("b", Level::Percent(50)),
            test_device("a", Level::Percent(50)),
            test_device("low", Level::Percent(9)),
        ];
        sort_weakest_first(&mut devices);
        let names: Vec<_> = devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["low", "a", "b", "none"]);
    }

    fn kinded(name: &str, kind: DeviceKind, level: Level) -> DeviceView {
        DeviceView {
            kind,
            ..test_device(name, level)
        }
    }

    #[test]
    fn sort_grouped_orders_groups_then_weakest() {
        let mut devices = vec![
            kinded("pad", DeviceKind::Gamepad, Level::Percent(5)),
            kinded("mouse", DeviceKind::Mouse, Level::Percent(70)),
            kinded("hs-none", DeviceKind::Headset, Level::NoData("x".into())),
            kinded("hs-high", DeviceKind::Headset, Level::Percent(90)),
            kinded("other", DeviceKind::Other, Level::Percent(1)),
            kinded("hs-low", DeviceKind::Headset, Level::Percent(20)),
            kinded("mouse-low", DeviceKind::Mouse, Level::Percent(10)),
        ];
        sort_grouped(&mut devices);
        let names: Vec<_> = devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "hs-low",
                "hs-high",
                "hs-none",
                "mouse-low",
                "mouse",
                "pad",
                "other"
            ]
        );
    }

    #[test]
    fn every_kind_has_a_group() {
        for kind in [
            DeviceKind::Headset,
            DeviceKind::Earbuds,
            DeviceKind::Mouse,
            DeviceKind::Keyboard,
            DeviceKind::Gamepad,
            DeviceKind::Other,
        ] {
            assert!(group_order(kind) < KIND_GROUPS.len());
        }
        assert_eq!(group_label(DeviceKind::Gamepad), "Controller");
    }
}
