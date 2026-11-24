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

/// The families `vid:pid` can be queried as, in `Family` order. HID families need their
/// `pick_target` collection among `collections`; ATK serial needs a port and a catalogue ATK
/// serial PID. A recovery id has none.
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
    let compx_vid =
        COMPX_VIDS.contains(&vid) && !(vid == PULSAR_VID && PULSAR_NOT_COMPX.contains(&pid));
    let atk_serial = vid == ATK_VID
        && serial_present
        && known(vid, pid).is_some_and(|device| device.family == Family::AtkSerial);
    [
        (Family::Compx, compx_vid && has_target(Family::Compx)),
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

    #[test]
    fn shared_391d_vid_goes_by_shape() {
        // 391D also hosts RongYuan receivers (FFFF:0002, feature 65): no Compx collection, no family.
        let rongyuan = HidShape {
            reports: vec![ReportSize {
                kind: ReportKind::Feature,
                id: 0,
                bytes: 65,
            }],
            ..shape(0xFFFF, 0x0002, Vec::new())
        };
        assert!(families_for(0x391D, 0x1A04, &[rongyuan], false).is_empty());
        assert_eq!(
            families_for(0x391D, 0x1A05, &[compx()], false),
            [Family::Compx]
        );
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
}
