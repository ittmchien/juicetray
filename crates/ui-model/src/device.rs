use std::fmt;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Headset,
    Earbuds,
    Mouse,
    Keyboard,
    Gamepad,
    Other,
}

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
            Path::Bluetooth => "Bluetooth",
            Path::Dongle => "dongle",
            Path::Cable => "cable",
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
}

/// Weakest first; devices without a number go last, ties break by name.
pub fn sort_weakest_first(devices: &mut [DeviceView]) {
    devices.sort_by(|a, b| {
        let (a_percent, b_percent) = (a.shown_percent(), b.shown_percent());
        (a_percent.is_none(), a_percent, &a.name).cmp(&(b_percent.is_none(), b_percent, &b.name))
    });
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn earbuds(left: Option<u8>, right: Option<u8>, case: Option<u8>) -> DeviceView {
        test_device("AirPods", Level::Earbuds { left, right, case })
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
}
