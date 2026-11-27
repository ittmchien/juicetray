//! The device catalogue: every known model and its family. `devices/*.toml` is compiled by
//! build.rs into `KNOWN` and `CATALOGUE_FILES`; nothing is parsed at run time here.

use crate::Family;

/// What a catalogue model is; the app maps it to its own icon and group kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Mouse,
    Headset,
    Keyboard,
    Gamepad,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownDevice {
    pub vid: u16,
    pub pid: u16,
    pub name: &'static str,
    pub family: Family,
    pub kind: Kind,
    /// Read on real hardware; only verified devices are queried unasked.
    pub verified: bool,
    /// Wired to the PC (`connection = "cable"`), not a receiver.
    pub cable: bool,
    /// No reply to the known commands: never opened or queried until a capture shows the frame.
    pub needs_capture: bool,
}

include!(concat!(env!("OUT_DIR"), "/known.rs"));

pub fn known(vid: u16, pid: u16) -> Option<&'static KnownDevice> {
    KNOWN
        .iter()
        .find(|device| device.vid == vid && device.pid == pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Link, is_recovery};
    use std::collections::HashSet;

    /// Ids of the rows `pick` selects, sorted so file order does not matter.
    fn ids(pick: fn(&KnownDevice) -> bool) -> Vec<(u16, u16)> {
        let mut ids: Vec<(u16, u16)> = KNOWN
            .iter()
            .filter(|d| pick(d))
            .map(|d| (d.vid, d.pid))
            .collect();
        ids.sort();
        ids
    }

    #[test]
    fn owner_rows_resolve() {
        // The owner's table survives the catalogue; 1299 and 130E are BITMOUSE as in the HUB.
        let expected = [
            (0x3837, 0x6011, Family::Mchose, Kind::Headset),
            (0x3837, 0x6010, Family::Mchose, Kind::Headset),
            (0x373B, 0x1107, Family::Compx, Kind::Mouse),
            (0x373B, 0x1299, Family::AtkBitmouse, Kind::Mouse),
            (0x373B, 0x130E, Family::AtkBitmouse, Kind::Mouse),
            (0x373B, 0x1216, Family::AtkBitmouse, Kind::Mouse),
            (0x3554, 0xFA09, Family::Report13, Kind::Keyboard),
            (0x391D, 0x1A05, Family::Compx, Kind::Mouse),
            (0x373B, 0x11CB, Family::AtkSerial, Kind::Headset),
            (0x373B, 0x11CA, Family::AtkSerial, Kind::Headset),
            (0x373B, 0x1204, Family::AtkSerial, Kind::Headset),
        ];
        for (vid, pid, family, kind) in expected {
            let device = known(vid, pid).expect("owner row is in the catalogue");
            assert_eq!(
                (device.family, device.kind),
                (family, kind),
                "{vid:04X}:{pid:04X}"
            );
        }
        // The ATK HUB Compx and BITMOUSE PIDs are imported too.
        assert_eq!(known(0x373B, 0x11FE).map(|d| d.family), Some(Family::Compx));
        assert_eq!(known(0x3554, 0xF58A).map(|d| d.family), Some(Family::Compx));
        assert_eq!(
            known(0x373B, 0x1155).map(|d| d.family),
            Some(Family::AtkBitmouse)
        );
    }

    #[test]
    fn verified_and_cable_flags() {
        assert_eq!(
            ids(|d| d.verified),
            [
                (0x248A, 0x5D2E),
                (0x249A, 0x5C2F),
                (0x373B, 0x1204),
                (0x373B, 0x1216),
                (0x373B, 0x1299),
                (0x3837, 0x6011)
            ]
        );
        let cable = ids(|d| d.cable);
        // 1299 and 130E are BITMOUSE mice on their cable (no HUB receiver flag).
        let cables = [
            (0x3837, 0x6010),
            (0x373B, 0x11CA),
            (0x373B, 0x1299),
            (0x373B, 0x130E),
            (0x248A, 0x5D2E),
        ];
        for id in cables {
            assert!(cable.contains(&id), "{id:04X?}");
        }
        let receivers = [
            (0x3837, 0x6011),
            (0x373B, 0x1107),
            (0x373B, 0x1216),
            (0x3554, 0xFA09),
            (0x391D, 0x1A05),
            (0x373B, 0x11CB),
            (0x373B, 0x1204),
            (0x249A, 0x5C2F),
        ];
        for id in receivers {
            assert!(!cable.contains(&id), "{id:04X?}");
        }
        // HUB receivers are receivers; other HUB PIDs are the mouse on its cable.
        assert!(!known(0x373B, 0x1155).unwrap().cable);
        assert!(known(0x373B, 0x1014).unwrap().cable);
    }

    #[test]
    fn needs_capture_flags() {
        // The flag stays as the mechanism for a future device without a known command.
        assert!(ids(|d| d.needs_capture).is_empty());
        assert!(KNOWN.iter().all(|d| !(d.needs_capture && d.verified)));
    }

    #[test]
    fn fa09_is_report13_and_unverified() {
        let fa09 = known(0x3554, 0xFA09).expect("FA09 is in the catalogue");
        assert_eq!(fa09.name, "AULA F75");
        assert_eq!(fa09.family, Family::Report13);
        assert!(!fa09.verified);
    }

    #[test]
    fn x1_se_dongle_is_bitmouse_and_dragonfly_receiver_is_compx() {
        let dongle = known(0x373B, 0x1216).expect("X1 SE dongle is in the catalogue");
        assert_eq!(dongle.family, Family::AtkBitmouse);
        assert_eq!(dongle.name, "ATK X1 SE");
        assert!(dongle.verified);
        let receiver = known(0x391D, 0x1A05).expect("Dragonfly receiver is in the catalogue");
        assert_eq!(receiver.family, Family::Compx);
        assert!(!receiver.cable && !receiver.verified);
    }

    #[test]
    fn turbo_plus_rongyuan_and_driveall_dongles_are_unverified_receivers() {
        let rows = [
            (0x391D, 0x1A21, Family::VgnKc2, Kind::Mouse),
            (0x3151, 0x5038, Family::RongYuanF7, Kind::Keyboard),
            (0x3151, 0x5006, Family::RongYuanF7, Kind::Keyboard),
            (0x0C45, 0xFEF9, Family::Driveall, Kind::Keyboard),
        ];
        for (vid, pid, family, kind) in rows {
            let device = known(vid, pid).expect("row is in the catalogue");
            assert_eq!((device.family, device.kind), (family, kind));
            assert!(!device.verified && !device.cable && !device.needs_capture);
        }
        // 3151:5038 reports itself as a MonsGeek dongle; the NUT75 dongle is 0C45:FEF9.
        let monsgeek = known(0x3151, 0x5038).expect("row is in the catalogue");
        assert_eq!(monsgeek.name, "MonsGeek 2.4G dongle");
    }

    /// The Delux M500 receiver and the mouse on its cable both speak Xctech.
    #[test]
    fn delux_m500_rows_are_xctech_mice() {
        for (vid, pid) in [(0x249A, 0x5C2F), (0x248A, 0x5D2E)] {
            let device = known(vid, pid).expect("row is in the catalogue");
            assert_eq!((device.family, device.kind), (Family::Xctech, Kind::Mouse));
            assert!(device.verified && !device.needs_capture);
        }
    }

    #[test]
    fn n9_ultra_is_serial_only() {
        let n9 = known(0x373B, 0x1204).expect("N9 Ultra is in the catalogue");
        assert_eq!(n9.family, Family::AtkSerial);
        assert_eq!(n9.family.link(), Link::Serial);
    }

    #[test]
    fn unknown_is_none() {
        assert!(known(0x046D, 0xC52B).is_none());
        assert!(known(0x373B, 0x9999).is_none());
        assert!(known(0x3554, 0x0001).is_none());
        // HUB families without a battery protocol here (HOLTEK, EVISION) are not imported.
        assert!(known(0x373B, 0x10F0).is_none());
        assert!(known(0x373B, 0x108C).is_none());
    }

    #[test]
    fn recovery_ids_are_never_listed() {
        assert!(KNOWN.iter().all(|d| !is_recovery(d.vid, d.pid)));
        assert!(known(0x3554, 0xF2FF).is_none());
        assert!(known(0x3554, 0xF400).is_none());
    }

    #[test]
    fn pid_in_both_hub_lists_is_listed_once() {
        let rows = KNOWN
            .iter()
            .filter(|d| (d.vid, d.pid) == (0x373B, 0x12AF))
            .count();
        assert_eq!(rows, 1);
    }

    #[test]
    fn known_table_has_no_duplicates() {
        let ids: HashSet<_> = KNOWN.iter().map(|d| (d.vid, d.pid)).collect();
        assert_eq!(ids.len(), KNOWN.len());
    }
}
