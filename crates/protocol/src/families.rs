//! Family detection from what a device exposes: its VID, its HID collection shapes and whether
//! it has a serial port. Nothing is sent; a family is named only when its query has a target.

use crate::{Family, HidShape, is_recovery, known, pick_target};

/// VIDs whose vendor report-0x08 17/17 collection speaks Compx report 8.
const COMPX_VIDS: [u16; 6] = [0x373B, 0x3554, 0x391D, 0x3710, 0x260D, 0x33E4];
/// Pulsar PIDs that speak another protocol on the same shape.
const PULSAR_NOT_COMPX: [u16; 6] = [0x5405, 0x3404, 0x3409, 0x3410, 0x5402, 0x5403];
const PULSAR_VID: u16 = 0x3710;
const ATK_VID: u16 = 0x373B;
const REPORT13_VID: u16 = 0x3554;
const MCHOSE_VID: u16 = 0x3837;
const VGN_VID: u16 = 0x391D;
/// VIDs of the HID families other than Compx; RongYuan F7 goes by catalogue PIDs only.
const OTHER_FAMILY_VIDS: [u16; 4] = [ATK_VID, REPORT13_VID, MCHOSE_VID, VGN_VID];

/// Whether `vid:pid` falls under Compx's VID rule (its VIDs minus Pulsar's other protocol).
fn compx_vid(vid: u16, pid: u16) -> bool {
    COMPX_VIDS.contains(&vid) && !(vid == PULSAR_VID && PULSAR_NOT_COMPX.contains(&pid))
}

/// Whether some family's VID rule admits `vid:pid`, so an id outside the catalogue may speak a
/// known protocol; recovery ids never do. Nothing is opened: shapes still decide the family.
pub fn family_vid(vid: u16, pid: u16) -> bool {
    !is_recovery(vid, pid) && (compx_vid(vid, pid) || OTHER_FAMILY_VIDS.contains(&vid))
}

/// The families `vid:pid` can be queried as, in `Family` order. HID families need their
/// `pick_target` collection among `collections`; ATK serial needs a port and a catalogue ATK
/// serial PID; RongYuan F7 needs a catalogue RongYuan PID, since other protocols share its
/// shape. A recovery id has none.
pub fn families_for(
    vid: u16,
    pid: u16,
    collections: &[HidShape],
    serial_present: bool,
) -> Vec<Family> {
    if is_recovery(vid, pid) {
        return Vec::new();
    }
    let has_target = |family| pick_target(family, collections).is_ok();
    let catalogue_is = |family| known(vid, pid).is_some_and(|device| device.family == family);
    let atk_serial = vid == ATK_VID && serial_present && catalogue_is(Family::AtkSerial);
    [
        (
            Family::Compx,
            compx_vid(vid, pid) && has_target(Family::Compx),
        ),
        (
            Family::Report13,
            vid == REPORT13_VID && has_target(Family::Report13),
        ),
        (
            Family::Mchose,
            vid == MCHOSE_VID && has_target(Family::Mchose),
        ),
        (Family::AtkSerial, atk_serial),
        (
            Family::AtkBitmouse,
            vid == ATK_VID && has_target(Family::AtkBitmouse),
        ),
        (Family::VgnKc2, vid == VGN_VID && has_target(Family::VgnKc2)),
        (
            Family::RongYuanF7,
            catalogue_is(Family::RongYuanF7) && has_target(Family::RongYuanF7),
        ),
    ]
    .into_iter()
    .filter_map(|(family, matches)| matches.then_some(family))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReportKind, ReportSize};

    fn both(id: u8, bytes: usize) -> Vec<ReportSize> {
        [ReportKind::Input, ReportKind::Output]
            .into_iter()
            .map(|kind| ReportSize { kind, id, bytes })
            .collect()
    }

    fn shape(usage_page: u16, usage: u16, reports: Vec<ReportSize>) -> HidShape {
        HidShape {
            interface: None,
            usage_page,
            usage,
            reports,
        }
    }

    fn compx() -> HidShape {
        shape(0xFF02, 0x0002, both(0x08, 17))
    }

    fn report13() -> HidShape {
        shape(0xFF02, 0x0002, both(0x13, 20))
    }

    fn mchose() -> HidShape {
        HidShape {
            interface: Some(3),
            ..shape(0xFF07, 0x0222, both(0x55, 64))
        }
    }

    /// The X1 SE dongle's FF05 collection: 64-byte output, 127-byte input.
    fn bitmouse() -> HidShape {
        shape(
            0xFF05,
            0x0001,
            vec![
                ReportSize {
                    kind: ReportKind::Output,
                    id: 0x08,
                    bytes: 64,
                },
                ReportSize {
                    kind: ReportKind::Input,
                    id: 0x08,
                    bytes: 127,
                },
            ],
        )
    }

    fn system_mouse() -> HidShape {
        shape(0x0001, 0x0002, both(0x00, 4))
    }

    #[test]
    fn compx_on_every_compx_vid() {
        for vid in COMPX_VIDS {
            assert_eq!(
                families_for(vid, 0x1234, &[system_mouse(), compx()], false),
                [Family::Compx],
                "{vid:04X}"
            );
        }
        // Another VID with the same shape is not Compx.
        assert!(families_for(0x046D, 0x1234, &[compx()], false).is_empty());
    }

    #[test]
    fn compx_needs_the_17_byte_vendor_collection() {
        assert!(families_for(0x3554, 0x1234, &[system_mouse()], false).is_empty());
        assert!(families_for(0x3554, 0x1234, &[], false).is_empty());
        let wrong_size = shape(0xFF02, 0x0002, both(0x08, 65));
        assert!(families_for(0x373B, 0x1107, &[wrong_size], false).is_empty());
    }

    #[test]
    fn the_six_non_compx_pulsar_pids_are_not_compx() {
        for pid in PULSAR_NOT_COMPX {
            assert!(
                families_for(PULSAR_VID, pid, &[compx()], false).is_empty(),
                "{pid:04X}"
            );
        }
        assert_eq!(
            families_for(PULSAR_VID, 0x5406, &[compx()], false),
            [Family::Compx]
        );
    }

    /// A RongYuan dongle's FFFF:0002 collection: feature report 0, 64 bytes.
    fn rongyuan() -> HidShape {
        let feature = ReportSize {
            kind: ReportKind::Feature,
            id: 0,
            bytes: 64,
        };
        shape(0xFFFF, 0x0002, vec![feature])
    }

    /// The Dragonfly 3 Turbo+ if1 collection: 64-byte B3 out, B4 in.
    fn vgn_kc2() -> HidShape {
        let report = |kind, id| ReportSize {
            kind,
            id,
            bytes: 64,
        };
        shape(
            0xFFC1,
            0x0001,
            vec![
                report(ReportKind::Output, 0xB3),
                report(ReportKind::Input, 0xB4),
            ],
        )
    }

    /// The Turbo+ if2 firmware-update channel.
    fn dfu() -> HidShape {
        shape(0x008C, 0x0001, both(0xB4, 64))
    }

    #[test]
    fn shared_391d_vid_goes_by_shape() {
        // 391D also hosts RongYuan receivers outside the catalogue: no family for them.
        assert!(families_for(0x391D, 0x1A04, &[rongyuan()], false).is_empty());
        assert_eq!(
            families_for(0x391D, 0x1A05, &[compx()], false),
            [Family::Compx]
        );
        assert_eq!(
            families_for(0x391D, 0x1A21, &[vgn_kc2(), dfu()], false),
            [Family::VgnKc2]
        );
    }

    #[test]
    fn vgn_kc2_only_on_391d_and_never_on_the_dfu_channel() {
        assert!(families_for(0x3554, 0x1A21, &[vgn_kc2()], false).is_empty());
        assert!(families_for(0x391D, 0x1A21, &[dfu()], false).is_empty());
        assert!(families_for(0x391D, 0x1A21, &[system_mouse()], false).is_empty());
    }

    #[test]
    fn rongyuan_needs_a_catalogue_pid_and_its_shape() {
        let push = shape(0xFFFF, 0x0001, both(0x05, 32));
        assert_eq!(
            families_for(0x3151, 0x5038, &[push.clone(), rongyuan()], false),
            [Family::RongYuanF7]
        );
        assert!(families_for(0x3151, 0x5038, &[push], false).is_empty());
        // The same shape on an id outside the catalogue is another protocol until listed.
        assert!(families_for(0x3151, 0x9999, &[rongyuan()], false).is_empty());
        assert!(families_for(0x046D, 0x5038, &[rongyuan()], false).is_empty());
    }

    #[test]
    fn report13_only_on_3554() {
        assert_eq!(
            families_for(0x3554, 0xFA09, &[report13()], false),
            [Family::Report13]
        );
        assert!(families_for(0x373B, 0xFA09, &[report13()], false).is_empty());
        // A 3554 device with both collections can be queried either way.
        assert_eq!(
            families_for(0x3554, 0x1234, &[compx(), report13()], false),
            [Family::Compx, Family::Report13]
        );
    }

    #[test]
    fn mchose_only_on_3837() {
        assert_eq!(
            families_for(0x3837, 0x6011, &[system_mouse(), mchose()], false),
            [Family::Mchose]
        );
        assert!(families_for(0x373B, 0x6011, &[mchose()], false).is_empty());
    }

    #[test]
    fn bitmouse_only_on_373b() {
        assert_eq!(
            families_for(0x373B, 0x1216, &[bitmouse()], false),
            [Family::AtkBitmouse]
        );
        assert!(families_for(0x3554, 0x1216, &[bitmouse()], false).is_empty());
        // ATK serves Compx and BITMOUSE on disjoint pages; both shapes give both families.
        assert_eq!(
            families_for(0x373B, 0x9999, &[compx(), bitmouse()], false),
            [Family::Compx, Family::AtkBitmouse]
        );
    }

    #[test]
    fn atk_serial_needs_a_port_and_a_catalogue_serial_pid() {
        assert_eq!(families_for(0x373B, 0x1204, &[], true), [Family::AtkSerial]);
        assert!(families_for(0x373B, 0x1204, &[], false).is_empty());
        // A port on a PID the catalogue does not list as ATK serial is never ATK serial.
        assert!(families_for(0x373B, 0x9999, &[], true).is_empty());
        assert!(families_for(0x373B, 0x1216, &[], true).is_empty());
        assert!(families_for(0x3837, 0x6011, &[], true).is_empty());
    }

    #[test]
    fn recovery_ids_have_no_family() {
        assert!(families_for(0x3554, 0xF2FF, &[compx()], false).is_empty());
        assert!(families_for(0x3554, 0xF405, &[compx(), report13()], false).is_empty());
    }

    #[test]
    fn ambiguous_collections_name_no_family() {
        let other = shape(0xFF00, 0x0001, both(0x08, 17));
        let third = shape(0xFF01, 0x0001, both(0x08, 17));
        assert!(families_for(0x3554, 0x1234, &[other, third], false).is_empty());
    }

    #[test]
    fn family_vids_admit_unknown_ids_except_recovery_and_other_pulsar_protocols() {
        for (vid, pid) in [
            (0x373B, 0x9999),
            (0x3554, 0x1234),
            (0x3837, 0x1234),
            (0x391D, 0x1234),
            (0x3710, 0x1234),
            (0x260D, 0x1234),
            (0x33E4, 0x1234),
        ] {
            assert!(family_vid(vid, pid), "{vid:04X}:{pid:04X}");
        }
        assert!(!family_vid(0x3710, 0x5405));
        assert!(!family_vid(0x3554, 0xF2FF));
        assert!(!family_vid(0x046D, 0xC52B));
    }
}
