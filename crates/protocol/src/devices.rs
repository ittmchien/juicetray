//! Devices the probe may query, with the family that decides which frame they get.

use crate::Family;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownDevice {
    pub vid: u16,
    pub pid: u16,
    pub name: &'static str,
    pub family: Family,
}

const fn device(vid: u16, pid: u16, name: &'static str, family: Family) -> KnownDevice {
    KnownDevice {
        vid,
        pid,
        name,
        family,
    }
}

// 0x373B:0x1204 is serial-only: it must never receive a Compx frame.
pub const KNOWN: &[KnownDevice] = &[
    device(0x3837, 0x6011, "MCHOSE V9 Pro 2 (dongle)", Family::Mchose),
    device(0x3837, 0x6010, "MCHOSE V9 Pro 2 (cable)", Family::Mchose),
    device(0x373B, 0x1107, "ATK X1 SE NK", Family::Compx),
    device(0x373B, 0x1299, "ATK X1 SE Nearlink", Family::Compx),
    device(0x373B, 0x130E, "ATK X1 SE NK", Family::Compx),
    device(0x373B, 0x1216, "ATK X1 SE (dongle)", Family::Compx),
    device(0x3554, 0xFA09, "VGN Dragonfly F3 (dongle)", Family::Compx),
    device(0x373B, 0x11CB, "ATK N9 Pro (dongle)", Family::AtkSerial),
    device(0x373B, 0x11CA, "ATK N9 Pro (cable)", Family::AtkSerial),
    device(0x373B, 0x1204, "ATK N9 Ultra (dongle)", Family::AtkSerial),
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
            (0x373B, 0x1216, Family::Compx),
            (0x3554, 0xFA09, Family::Compx),
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
