//! Devices the probe may query, with the family that decides which frame they get.

use crate::Family;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownDevice {
    pub vid: u16,
    pub pid: u16,
    pub name: &'static str,
    pub family: Family,
    /// A real reply was seen on this model; only confirmed devices are queried unasked.
    pub confirmed: bool,
    /// Wired to the PC (cable), not a receiver.
    pub cable: bool,
    /// No reply to the known commands: never opened or queried until a capture shows the frame.
    pub needs_capture: bool,
}

const fn device(
    vid: u16,
    pid: u16,
    name: &'static str,
    family: Family,
    confirmed: bool,
    cable: bool,
    needs_capture: bool,
) -> KnownDevice {
    KnownDevice {
        vid,
        pid,
        name,
        family,
        confirmed,
        cable,
        needs_capture,
    }
}

// Columns: vid, pid, name, family, confirmed, cable, needs_capture.
// 0x373B:0x1204 is serial-only: it must never receive a Compx frame.
pub const KNOWN: &[KnownDevice] = &[
    device(
        0x3837,
        0x6011,
        "MCHOSE V9 Pro 2 (dongle)",
        Family::Mchose,
        true,
        false,
        false,
    ),
    device(
        0x3837,
        0x6010,
        "MCHOSE V9 Pro 2 (cable)",
        Family::Mchose,
        false,
        true,
        false,
    ),
    device(
        0x373B,
        0x1107,
        "ATK X1 SE NK",
        Family::Compx,
        false,
        false,
        false,
    ),
    device(
        0x373B,
        0x1299,
        "ATK X1 SE Nearlink",
        Family::Compx,
        false,
        false,
        false,
    ),
    device(
        0x373B,
        0x130E,
        "ATK X1 SE NK",
        Family::Compx,
        false,
        false,
        false,
    ),
    device(
        0x373B,
        0x1216,
        "ATK X1 SE (dongle)",
        Family::AtkBitmouse,
        false,
        false,
        false,
    ),
    device(
        0x3554,
        0xFA09,
        "VGN Dragonfly 3 Master+ (dongle)",
        Family::Report13,
        false,
        false,
        true,
    ),
    device(
        0x391D,
        0x1A05,
        "VGN Dragonfly 3 Master+ (cable)",
        Family::Compx,
        false,
        true,
        true,
    ),
    device(
        0x373B,
        0x11CB,
        "ATK N9 Pro (dongle)",
        Family::AtkSerial,
        false,
        false,
        false,
    ),
    device(
        0x373B,
        0x11CA,
        "ATK N9 Pro (cable)",
        Family::AtkSerial,
        false,
        true,
        false,
    ),
    device(
        0x373B,
        0x1204,
        "ATK N9 Ultra (dongle)",
        Family::AtkSerial,
        false,
        false,
        false,
    ),
];

pub fn known(vid: u16, pid: u16) -> Option<&'static KnownDevice> {
    KNOWN
        .iter()
        .find(|device| device.vid == vid && device.pid == pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Link;
    use std::collections::HashSet;

    #[test]
    fn known_table_resolves() {
        let expected = [
            (0x3837, 0x6011, Family::Mchose),
            (0x3837, 0x6010, Family::Mchose),
            (0x373B, 0x1107, Family::Compx),
            (0x373B, 0x1299, Family::Compx),
            (0x373B, 0x130E, Family::Compx),
            (0x373B, 0x1216, Family::AtkBitmouse),
            (0x3554, 0xFA09, Family::Report13),
            (0x391D, 0x1A05, Family::Compx),
            (0x373B, 0x11CB, Family::AtkSerial),
            (0x373B, 0x11CA, Family::AtkSerial),
            (0x373B, 0x1204, Family::AtkSerial),
        ];
        assert_eq!(KNOWN.len(), expected.len());
        for (vid, pid, family) in expected {
            assert_eq!(known(vid, pid).map(|d| d.family), Some(family));
        }
    }

    #[test]
    fn confirmed_and_cable_flags() {
        let ids = |pick: fn(&KnownDevice) -> bool| -> Vec<(u16, u16)> {
            KNOWN
                .iter()
                .filter(|d| pick(d))
                .map(|d| (d.vid, d.pid))
                .collect()
        };
        assert_eq!(ids(|d| d.confirmed), [(0x3837, 0x6011)]);
        assert_eq!(
            ids(|d| d.cable),
            [(0x3837, 0x6010), (0x391D, 0x1A05), (0x373B, 0x11CA)]
        );
    }

    #[test]
    fn needs_capture_flags() {
        let ids: Vec<(u16, u16)> = KNOWN
            .iter()
            .filter(|d| d.needs_capture)
            .map(|d| (d.vid, d.pid))
            .collect();
        assert_eq!(ids, [(0x3554, 0xFA09), (0x391D, 0x1A05)]);
        assert!(KNOWN.iter().all(|d| !(d.needs_capture && d.confirmed)));
    }

    #[test]
    fn fa09_is_report13_and_unconfirmed() {
        let fa09 = known(0x3554, 0xFA09).expect("FA09 is in the table");
        assert_eq!(fa09.name, "VGN Dragonfly 3 Master+ (dongle)");
        assert_eq!(fa09.family, Family::Report13);
        assert!(!fa09.confirmed);
    }

    #[test]
    fn x1_se_dongle_is_bitmouse_and_dragonfly_cable_is_compx() {
        let dongle = known(0x373B, 0x1216).expect("X1 SE dongle is in the table");
        assert_eq!(dongle.family, Family::AtkBitmouse);
        assert!(!dongle.confirmed);
        let cable = known(0x391D, 0x1A05).expect("Dragonfly cable is in the table");
        assert_eq!(cable.family, Family::Compx);
        assert!(cable.cable && !cable.confirmed);
    }

    #[test]
    fn n9_ultra_is_serial_only() {
        let n9 = known(0x373B, 0x1204).expect("N9 Ultra is in the table");
        assert_eq!(n9.family, Family::AtkSerial);
        assert_eq!(n9.family.link(), Link::Serial);
    }

    #[test]
    fn unknown_is_none() {
        assert!(known(0x046D, 0xC52B).is_none());
        assert!(known(0x373B, 0x9999).is_none());
        assert!(known(0x3554, 0x0001).is_none());
    }

    #[test]
    fn known_table_has_no_duplicates() {
        let ids: HashSet<_> = KNOWN.iter().map(|d| (d.vid, d.pid)).collect();
        assert_eq!(ids.len(), KNOWN.len());
    }
}
