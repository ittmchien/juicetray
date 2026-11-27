//! HID collection rules: which collections may be opened, and which one a family's query goes to.

use crate::{
    Family, GenericReport, GenericSpec, ReportKind, ReportSize, bitmouse, driveall, known,
    rongyuan, vgn_kc2,
};

/// First usage page of the vendor-defined range.
const VENDOR_PAGE_MIN: u16 = 0xFF00;
/// Firmware-update (DFU) channel on VGN / GTAI 2.4 GHz chips; never opened or picked.
pub const FIRMWARE_UPDATE_PAGE: u16 = 0x008C;
const VGN_KC2_PAGE: (u16, u16) = (0xFFC1, 0x0001);
const RONGYUAN_PAGE: (u16, u16) = (0xFFFF, 0x0002);
/// The Driveall query channel; the dongle's FF70:0071 and system collections are never targets.
const DRIVEALL_PAGE: (u16, u16) = (0xFF60, 0x0061);
/// The Xctech query channel: generic-desktop usage 0 (Undefined), never a system mouse/keyboard;
/// opened only on Xctech catalogue ids.
const XCTECH_PAGE: (u16, u16) = (0x0001, 0x0000);

const COMPX_REPORT_ID: u8 = 0x08;
const COMPX_REPORT_LEN: usize = 17;
const REPORT13_REPORT_ID: u8 = 0x13;
const REPORT13_REPORT_LEN: usize = 20;
/// Usage page and usage preferred when several vendor candidates qualify.
const VENDOR_PREFERRED: (u16, u16) = (0xFF02, 0x0002);
const MCHOSE_REPORT_ID: u8 = 0x55;
const MCHOSE_REPORT_LEN: usize = 64;
const MCHOSE_INTERFACE: u8 = 3;

/// What a probe knows about one HID collection of a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidShape {
    /// USB interface number; `None` when the backend does not report it.
    pub interface: Option<u8>,
    pub usage_page: u16,
    pub usage: u16,
    pub reports: Vec<ReportSize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickError {
    NoMatch,
    /// Indices of the equally good candidates.
    Ambiguous(Vec<usize>),
}

/// Why a collection is a system mouse or keyboard, if it is one.
fn system_reason(usage_page: u16, usage: u16) -> Option<&'static str> {
    match (usage_page, usage) {
        (0x0001, 0x0002) => Some("system mouse"),
        (0x0001, 0x0006) => Some("system keyboard"),
        _ => None,
    }
}

/// Whether `(usage_page, usage)` is the Xctech channel of an Xctech catalogue id; on any other id
/// the generic-desktop usage 0 stays closed like every non-vendor collection.
fn is_xctech_channel((vid, pid): (u16, u16), usage_page: u16, usage: u16) -> bool {
    (usage_page, usage) == XCTECH_PAGE
        && known(vid, pid).is_some_and(|device| device.family == Family::Xctech)
}

/// One verdict per `(path, (vid, pid), usage page, usage)` entry: `Ok` only for a vendor
/// collection (or an Xctech model's 0001:0000 channel) that is not a system mouse/keyboard and
/// whose path no mouse/keyboard or firmware-update collection shares.
pub fn open_permission(entries: &[(&str, (u16, u16), u16, u16)]) -> Vec<Result<(), &'static str>> {
    // macOS lists every usage pair of an interface under one path, so opening one opens all.
    let shared_with = |path: &str, deny: &dyn Fn(u16, u16) -> bool| {
        entries
            .iter()
            .any(|&(other, _, page, usage)| other == path && deny(page, usage))
    };
    entries
        .iter()
        .map(|&(path, id, usage_page, usage)| {
            if usage_page == FIRMWARE_UPDATE_PAGE {
                return Err("firmware-update channel");
            }
            if let Some(reason) = system_reason(usage_page, usage) {
                return Err(reason);
            }
            if usage_page < VENDOR_PAGE_MIN && !is_xctech_channel(id, usage_page, usage) {
                return Err("not a vendor collection");
            }
            if shared_with(path, &|page, usage| system_reason(page, usage).is_some()) {
                return Err("shares its interface with a mouse or keyboard");
            }
            if shared_with(path, &|page, _| page == FIRMWARE_UPDATE_PAGE) {
                return Err("shares its interface with a firmware-update channel");
            }
            Ok(())
        })
        .collect()
}

/// Index of the collection `family`'s battery query goes to. `AtkSerial` is never HID.
pub fn pick_target(family: Family, shapes: &[HidShape]) -> Result<usize, PickError> {
    match family {
        Family::Compx => pick_vendor_report(shapes, COMPX_REPORT_ID, COMPX_REPORT_LEN),
        Family::Report13 => pick_vendor_report(shapes, REPORT13_REPORT_ID, REPORT13_REPORT_LEN),
        Family::Mchose => pick_mchose(shapes),
        Family::AtkBitmouse => pick_bitmouse(shapes),
        Family::VgnKc2 => pick_vgn_kc2(shapes),
        Family::RongYuanF7 => pick_rongyuan(shapes),
        Family::Driveall => pick_driveall(shapes),
        Family::Xctech => pick_xctech(shapes),
        Family::Generic(spec) => pick_generic(spec, shapes),
        Family::AtkSerial => Err(PickError::NoMatch),
    }
}

fn has_report(shape: &HidShape, kind: ReportKind, id: u8, bytes: usize) -> bool {
    shape
        .reports
        .iter()
        .any(|report| report.kind == kind && report.id == id && report.bytes == bytes)
}

/// Every picker goes through here, so a firmware-update collection is never a target.
fn matching(shapes: &[HidShape], accepts: impl Fn(&HidShape) -> bool) -> Vec<usize> {
    shapes
        .iter()
        .enumerate()
        .filter(|(_, shape)| shape.usage_page != FIRMWARE_UPDATE_PAGE && accepts(shape))
        .map(|(index, _)| index)
        .collect()
}

fn exactly_one(found: Vec<usize>) -> Result<usize, PickError> {
    match found.len() {
        0 => Err(PickError::NoMatch),
        1 => Ok(found[0]),
        _ => Err(PickError::Ambiguous(found)),
    }
}

/// Vendor collection with an `id` report of `bytes` (id included) in both directions; several
/// candidates are narrowed to `FF02/0002`.
fn pick_vendor_report(shapes: &[HidShape], id: u8, bytes: usize) -> Result<usize, PickError> {
    let candidates = matching(shapes, |shape| {
        shape.usage_page >= VENDOR_PAGE_MIN
            && has_report(shape, ReportKind::Input, id, bytes)
            && has_report(shape, ReportKind::Output, id, bytes)
    });
    if candidates.len() <= 1 {
        return exactly_one(candidates);
    }
    let preferred: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|&index| (shapes[index].usage_page, shapes[index].usage) == VENDOR_PREFERRED)
        .collect();
    match preferred.len() {
        1 => Ok(preferred[0]),
        0 => Err(PickError::Ambiguous(candidates)),
        _ => Err(PickError::Ambiguous(preferred)),
    }
}

/// `FF07/0222` on interface 3 (or an unknown interface) with 64-byte id-0x55 reports both ways.
fn pick_mchose(shapes: &[HidShape]) -> Result<usize, PickError> {
    exactly_one(matching(shapes, |shape| {
        shape.usage_page == 0xFF07
            && shape.usage == 0x0222
            && matches!(shape.interface, Some(MCHOSE_INTERFACE) | None)
            && has_report(
                shape,
                ReportKind::Input,
                MCHOSE_REPORT_ID,
                MCHOSE_REPORT_LEN,
            )
            && has_report(
                shape,
                ReportKind::Output,
                MCHOSE_REPORT_ID,
                MCHOSE_REPORT_LEN,
            )
    }))
}

/// `FF05/0001` with a 64-byte id-8 output report and an id-8 input report of any usable size.
fn pick_bitmouse(shapes: &[HidShape]) -> Result<usize, PickError> {
    exactly_one(matching(shapes, |shape| {
        shape.usage_page == 0xFF05
            && shape.usage == 0x0001
            && has_report(
                shape,
                ReportKind::Output,
                bitmouse::REPORT_ID,
                bitmouse::REPORT_LEN,
            )
            && shape.reports.iter().any(|report| {
                report.kind == ReportKind::Input
                    && report.id == bitmouse::REPORT_ID
                    && report.bytes >= bitmouse::MIN_REPLY_LEN
            })
    }))
}

/// `FFC1/0001` with the 64-byte `0xB3` output and `0xB4` input reports.
fn pick_vgn_kc2(shapes: &[HidShape]) -> Result<usize, PickError> {
    exactly_one(matching(shapes, |shape| {
        (shape.usage_page, shape.usage) == VGN_KC2_PAGE
            && has_report(
                shape,
                ReportKind::Output,
                vgn_kc2::OUTPUT_ID,
                vgn_kc2::REPORT_LEN,
            )
            && has_report(
                shape,
                ReportKind::Input,
                vgn_kc2::INPUT_ID,
                vgn_kc2::REPORT_LEN,
            )
    }))
}

/// `FFFF/0002` with the 64-byte feature report 0 (65 bytes on the wire with the id).
fn pick_rongyuan(shapes: &[HidShape]) -> Result<usize, PickError> {
    exactly_one(matching(shapes, |shape| {
        (shape.usage_page, shape.usage) == RONGYUAN_PAGE
            && has_report(shape, ReportKind::Feature, 0, rongyuan::FRAME_LEN - 1)
    }))
}

/// `FF60/0061` with 64-byte input and output report 0 (no report id on the wire's data).
fn pick_driveall(shapes: &[HidShape]) -> Result<usize, PickError> {
    exactly_one(matching(shapes, |shape| {
        (shape.usage_page, shape.usage) == DRIVEALL_PAGE
            && has_report(shape, ReportKind::Output, 0, driveall::REPORT_LEN)
            && has_report(shape, ReportKind::Input, 0, driveall::REPORT_LEN)
    }))
}

/// `0001/0000` by page and usage only: the descriptor hidapi rebuilds from preparsed data lists no
/// reports (`05 01 09 00 a1 01 c0`); the open rule limits it to Xctech catalogue ids.
fn pick_xctech(shapes: &[HidShape]) -> Result<usize, PickError> {
    exactly_one(matching(shapes, |shape| {
        (shape.usage_page, shape.usage) == XCTECH_PAGE
    }))
}

/// The row's page:usage on a vendor page with the request's report (id and size as scan prints
/// them); an output row also needs an input report with the reply's id.
fn pick_generic(spec: &GenericSpec, shapes: &[HidShape]) -> Result<usize, PickError> {
    let (Some(&request_id), Some(&reply_id)) = (spec.request.first(), spec.reply_prefix.first())
    else {
        return Err(PickError::NoMatch);
    };
    let (kind, needs_input) = match spec.report {
        GenericReport::Output => (ReportKind::Output, true),
        GenericReport::Feature => (ReportKind::Feature, false),
    };
    exactly_one(matching(shapes, |shape| {
        (shape.usage_page, shape.usage) == (spec.usage_page, spec.usage)
            && shape.usage_page >= VENDOR_PAGE_MIN
            && has_report(shape, kind, request_id, usize::from(spec.report_bytes))
            && (!needs_input
                || shape
                    .reports
                    .iter()
                    .any(|report| report.kind == ReportKind::Input && report.id == reply_id))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(kind: ReportKind, id: u8, bytes: usize) -> ReportSize {
        ReportSize { kind, id, bytes }
    }

    fn both(id: u8, bytes: usize) -> Vec<ReportSize> {
        vec![
            report(ReportKind::Input, id, bytes),
            report(ReportKind::Output, id, bytes),
        ]
    }

    fn shape(usage_page: u16, usage: u16, reports: Vec<ReportSize>) -> HidShape {
        HidShape {
            interface: None,
            usage_page,
            usage,
            reports,
        }
    }

    const SHARED: &str = "shares its interface with a mouse or keyboard";
    /// An id outside the catalogue, and the Delux M500 receiver and cable (Xctech rows).
    const OTHER: (u16, u16) = (0x046D, 0xC52B);
    const M500: (u16, u16) = (0x249A, 0x5C2F);
    const M500_CABLE: (u16, u16) = (0x248A, 0x5D2E);

    #[test]
    fn open_permission_distinct_paths() {
        let verdicts = open_permission(&[
            ("A", OTHER, 0xFF02, 2),
            ("B", OTHER, 0x01, 2),
            ("C", OTHER, 0x01, 6),
            ("D", OTHER, 0x0C, 1),
        ]);
        assert_eq!(
            verdicts,
            vec![
                Ok(()),
                Err("system mouse"),
                Err("system keyboard"),
                Err("not a vendor collection"),
            ]
        );
    }

    #[test]
    fn open_permission_shared_path() {
        let verdicts = open_permission(&[("P", OTHER, 0xFF00, 1), ("P", OTHER, 0x01, 6)]);
        assert_eq!(verdicts, vec![Err(SHARED), Err("system keyboard")]);
    }

    #[test]
    fn open_permission_shared_with_mouse_and_empty() {
        let verdicts = open_permission(&[
            ("P", OTHER, 0xFF00, 1),
            ("P", OTHER, 0x01, 2),
            ("Q", OTHER, 0xFF00, 1),
        ]);
        assert_eq!(verdicts, vec![Err(SHARED), Err("system mouse"), Ok(())]);
        assert!(open_permission(&[]).is_empty());
    }

    /// The M500 receiver's interfaces: mouse, keyboard + consumer, and its own 0001:0000.
    #[test]
    fn open_permission_opens_0001_0000_only_on_xctech_catalogue_ids() {
        let verdicts = open_permission(&[
            ("M0", M500, 0x01, 2),
            ("M1", M500, 0x01, 6),
            ("M1", M500, 0x0C, 1),
            ("M2", M500, 0x01, 0),
            ("W2", M500_CABLE, 0x01, 0),
            ("L", OTHER, 0x01, 0),
        ]);
        assert_eq!(
            verdicts,
            vec![
                Err("system mouse"),
                Err("system keyboard"),
                Err("not a vendor collection"),
                Ok(()),
                Ok(()),
                Err("not a vendor collection"),
            ]
        );
        // Behind a path shared with a mouse (macOS), the Xctech channel stays closed.
        let verdicts = open_permission(&[("P", M500, 0x01, 0), ("P", M500, 0x01, 2)]);
        assert_eq!(verdicts, vec![Err(SHARED), Err("system mouse")]);
    }

    #[test]
    fn pick_compx_single() {
        let shapes = [
            shape(0x0001, 0x0002, vec![report(ReportKind::Input, 0, 4)]),
            shape(0xFF01, 0x0001, both(0x08, 17)),
        ];
        assert_eq!(pick_target(Family::Compx, &shapes), Ok(1));
    }

    #[test]
    fn pick_compx_prefers_ff02() {
        let shapes = [
            shape(0xFF00, 0x0001, both(0x08, 17)),
            shape(0xFF02, 0x0002, both(0x08, 17)),
        ];
        assert_eq!(pick_target(Family::Compx, &shapes), Ok(1));
    }

    #[test]
    fn pick_compx_ambiguous() {
        let shapes = [
            shape(0xFF00, 0x0001, both(0x08, 17)),
            shape(0xFF01, 0x0001, both(0x08, 17)),
        ];
        assert_eq!(
            pick_target(Family::Compx, &shapes),
            Err(PickError::Ambiguous(vec![0, 1]))
        );
    }

    #[test]
    fn pick_compx_two_preferred_are_ambiguous() {
        let shapes = [
            shape(0xFF02, 0x0002, both(0x08, 17)),
            shape(0xFF00, 0x0001, both(0x08, 17)),
            shape(0xFF02, 0x0002, both(0x08, 17)),
        ];
        assert_eq!(
            pick_target(Family::Compx, &shapes),
            Err(PickError::Ambiguous(vec![0, 2]))
        );
    }

    #[test]
    fn pick_compx_needs_both_directions() {
        let shapes = [shape(
            0xFF02,
            0x0002,
            vec![report(ReportKind::Input, 0x08, 17)],
        )];
        assert_eq!(pick_target(Family::Compx, &shapes), Err(PickError::NoMatch));
    }

    #[test]
    fn pick_compx_wrong_size() {
        let shapes = [shape(0xFF02, 0x0002, both(0x08, 65))];
        assert_eq!(pick_target(Family::Compx, &shapes), Err(PickError::NoMatch));
    }

    #[test]
    fn pick_compx_needs_a_vendor_page_and_report_id_8() {
        let shapes = [
            shape(0x0001, 0x0002, both(0x08, 17)),
            shape(0xFF02, 0x0002, both(0x09, 17)),
        ];
        assert_eq!(pick_target(Family::Compx, &shapes), Err(PickError::NoMatch));
        assert_eq!(pick_target(Family::Compx, &[]), Err(PickError::NoMatch));
    }

    #[test]
    fn pick_report13_prefers_ff02() {
        let shapes = [
            shape(0xFF00, 0x0001, both(0x13, 20)),
            shape(0xFF02, 0x0002, both(0x13, 20)),
        ];
        assert_eq!(pick_target(Family::Report13, &shapes), Ok(1));
    }

    #[test]
    fn pick_report13_refuses_the_compx_collection() {
        let shapes = [shape(0xFF02, 0x0002, both(0x08, 17))];
        assert_eq!(
            pick_target(Family::Report13, &shapes),
            Err(PickError::NoMatch)
        );
        assert_eq!(pick_target(Family::Compx, &shapes), Ok(0));
    }

    #[test]
    fn pick_mchose_interface() {
        let on = |interface| HidShape {
            interface,
            ..shape(0xFF07, 0x0222, both(0x55, 64))
        };
        assert_eq!(pick_target(Family::Mchose, &[on(Some(3))]), Ok(0));
        assert_eq!(
            pick_target(Family::Mchose, &[on(Some(2))]),
            Err(PickError::NoMatch)
        );
        assert_eq!(pick_target(Family::Mchose, &[on(None)]), Ok(0));
    }

    #[test]
    fn pick_mchose_rejects_other_shapes() {
        let shapes = [
            shape(0xFF07, 0x0221, both(0x55, 64)),
            shape(0xFF07, 0x0222, both(0x55, 63)),
            shape(0xFF06, 0x0222, both(0x55, 64)),
        ];
        assert_eq!(
            pick_target(Family::Mchose, &shapes),
            Err(PickError::NoMatch)
        );
    }

    #[test]
    fn pick_mchose_twin_is_ambiguous() {
        let twin = shape(0xFF07, 0x0222, both(0x55, 64));
        assert_eq!(
            pick_target(Family::Mchose, &[twin.clone(), twin]),
            Err(PickError::Ambiguous(vec![0, 1]))
        );
    }

    #[test]
    fn pick_bitmouse_finds_ff05_among_x1_se_collections() {
        let out = |id, bytes| report(ReportKind::Output, id, bytes);
        let input = |id, bytes| report(ReportKind::Input, id, bytes);
        let shapes = [
            shape(0xFFB3, 0x0001, vec![input(0, 24), out(0, 64)]),
            shape(0xFF05, 0x0001, vec![input(8, 127), out(8, 64)]),
            shape(0xFFB1, 0x0001, vec![out(9, 13)]),
        ];
        assert_eq!(pick_target(Family::AtkBitmouse, &shapes), Ok(1));
        assert_eq!(
            pick_target(Family::AtkBitmouse, &shapes[..1]),
            Err(PickError::NoMatch)
        );
        assert_eq!(
            pick_target(Family::AtkBitmouse, &[shapes[1].clone(), shapes[1].clone()]),
            Err(PickError::Ambiguous(vec![0, 1]))
        );
    }

    #[test]
    fn pick_bitmouse_needs_both_reports() {
        let only_input = shape(0xFF05, 0x0001, vec![report(ReportKind::Input, 8, 127)]);
        let short_output = shape(0xFF05, 0x0001, both(8, 17));
        assert_eq!(
            pick_target(Family::AtkBitmouse, &[only_input, short_output]),
            Err(PickError::NoMatch)
        );
    }

    /// The Dragonfly 3 Turbo+ if1: B3/B5 out, B4/B6 in.
    fn vgn_kc2_shape() -> HidShape {
        shape(
            0xFFC1,
            0x0001,
            vec![
                report(ReportKind::Output, 0xB3, 64),
                report(ReportKind::Output, 0xB5, 21),
                report(ReportKind::Input, 0xB4, 64),
                report(ReportKind::Input, 0xB6, 21),
            ],
        )
    }

    /// The MonsGeek 3151:5038 dongle's if2 as parsed: feature report 0 of 64 bytes.
    fn rongyuan_shape() -> HidShape {
        shape(0xFFFF, 0x0002, vec![report(ReportKind::Feature, 0, 64)])
    }

    #[test]
    fn open_permission_never_opens_the_firmware_update_channel() {
        let verdicts = open_permission(&[
            ("A", OTHER, 0x008C, 1),
            ("B", OTHER, 0xFFC1, 1),
            ("P", OTHER, 0x008C, 2),
        ]);
        assert_eq!(
            verdicts,
            vec![
                Err("firmware-update channel"),
                Ok(()),
                Err("firmware-update channel")
            ]
        );
        // A vendor collection behind the same path (macOS) is refused too.
        let verdicts = open_permission(&[("P", OTHER, 0xFFC1, 1), ("P", OTHER, 0x008C, 1)]);
        assert_eq!(
            verdicts,
            vec![
                Err("shares its interface with a firmware-update channel"),
                Err("firmware-update channel"),
            ]
        );
    }

    #[test]
    fn no_family_ever_picks_the_firmware_update_channel() {
        // A 0x008C collection carrying every family's reports is still never a target.
        let mut reports = vgn_kc2_shape().reports;
        reports.extend(rongyuan_shape().reports);
        reports.extend(both(0x08, 17));
        reports.extend(both(0x13, 20));
        reports.extend(both(0x55, 64));
        reports.extend(both(0, 64));
        reports.extend(both(0, 32));
        reports.push(report(ReportKind::Output, 0x08, 64));
        reports.push(report(ReportKind::Input, 0x08, 127));
        let dfu = HidShape {
            interface: Some(3),
            ..shape(0x008C, 0x0001, reports)
        };
        // A Generic spec aimed at 008C itself (build.rs refuses such a row) still gets nothing.
        static ON_DFU: GenericSpec = GenericSpec {
            usage_page: 0x008C,
            usage: 0x0001,
            report: GenericReport::Output,
            report_bytes: 64,
            request: &[0x00, 0x01],
            reply_prefix: &[0x00, 0x01],
            level_byte: 3,
            charging: None,
            scale: 100,
        };
        for family in [
            Family::Compx,
            Family::Report13,
            Family::Mchose,
            Family::AtkSerial,
            Family::AtkBitmouse,
            Family::VgnKc2,
            Family::RongYuanF7,
            Family::Driveall,
            Family::Xctech,
            Family::Generic(&crate::generic::tests::OUTPUT),
            Family::Generic(&ON_DFU),
        ] {
            assert_eq!(
                pick_target(family, std::slice::from_ref(&dfu)),
                Err(PickError::NoMatch),
                "{family:?}"
            );
        }
        // The Turbo+ lists if1 and the if2 DFU channel: if1 is picked.
        assert_eq!(pick_target(Family::VgnKc2, &[dfu, vgn_kc2_shape()]), Ok(1));
    }

    #[test]
    fn pick_vgn_kc2_needs_ffc1_and_both_64_byte_reports() {
        assert_eq!(pick_target(Family::VgnKc2, &[vgn_kc2_shape()]), Ok(0));
        let wrong_page = HidShape {
            usage_page: 0xFFC0,
            ..vgn_kc2_shape()
        };
        let no_input = shape(0xFFC1, 0x0001, vec![report(ReportKind::Output, 0xB3, 64)]);
        let short_input = shape(
            0xFFC1,
            0x0001,
            vec![
                report(ReportKind::Output, 0xB3, 64),
                report(ReportKind::Input, 0xB4, 21),
            ],
        );
        assert_eq!(
            pick_target(Family::VgnKc2, &[wrong_page, no_input, short_input]),
            Err(PickError::NoMatch)
        );
        assert_eq!(
            pick_target(Family::VgnKc2, &[vgn_kc2_shape(), vgn_kc2_shape()]),
            Err(PickError::Ambiguous(vec![0, 1]))
        );
        // Other families never take the kc2 collection.
        assert_eq!(
            pick_target(Family::Compx, &[vgn_kc2_shape()]),
            Err(PickError::NoMatch)
        );
    }

    #[test]
    fn pick_rongyuan_needs_ffff_0002_feature_64() {
        // The MonsGeek 3151:5038 dongle: if1 FFFF:0001 input 05 (32 B) and if2 FFFF:0002 feature 64.
        let push = shape(0xFFFF, 0x0001, vec![report(ReportKind::Input, 0x05, 32)]);
        assert_eq!(
            pick_target(Family::RongYuanF7, &[push.clone(), rongyuan_shape()]),
            Ok(1)
        );
        let wrong_size = shape(0xFFFF, 0x0002, vec![report(ReportKind::Feature, 0, 7)]);
        let wrong_usage = HidShape {
            usage: 0x0001,
            ..rongyuan_shape()
        };
        let numbered = shape(0xFFFF, 0x0002, vec![report(ReportKind::Feature, 1, 65)]);
        assert_eq!(
            pick_target(
                Family::RongYuanF7,
                &[push, wrong_size, wrong_usage, numbered]
            ),
            Err(PickError::NoMatch)
        );
        assert_eq!(
            pick_target(Family::VgnKc2, &[rongyuan_shape()]),
            Err(PickError::NoMatch)
        );
    }

    /// The NUT75 2.4G dongle's if3 as parsed: FF60:0061, input and output report 0 of 64 bytes.
    fn driveall_shape() -> HidShape {
        HidShape {
            interface: Some(3),
            ..shape(0xFF60, 0x0061, both(0, 64))
        }
    }

    #[test]
    fn pick_driveall_needs_ff60_0061_and_both_64_byte_reports() {
        let if4 = HidShape {
            interface: Some(4),
            ..shape(0xFF70, 0x0071, both(0, 64))
        };
        assert_eq!(
            pick_target(Family::Driveall, &[if4.clone(), driveall_shape()]),
            Ok(1)
        );
        let wrong_usage = HidShape {
            usage: 0x0062,
            ..driveall_shape()
        };
        let qmk_32 = shape(0xFF60, 0x0061, both(0, 32));
        let numbered = shape(0xFF60, 0x0061, both(0x01, 65));
        let no_input = shape(0xFF60, 0x0061, vec![report(ReportKind::Output, 0, 64)]);
        assert_eq!(
            pick_target(
                Family::Driveall,
                &[if4, wrong_usage, qmk_32, numbered, no_input]
            ),
            Err(PickError::NoMatch)
        );
        assert_eq!(
            pick_target(Family::Driveall, &[driveall_shape(), driveall_shape()]),
            Err(PickError::Ambiguous(vec![0, 1]))
        );
        // Other families never take the Driveall collection.
        for family in [Family::Compx, Family::Mchose, Family::VgnKc2] {
            assert_eq!(
                pick_target(family, &[driveall_shape()]),
                Err(PickError::NoMatch),
                "{family:?}"
            );
        }
    }

    /// The M500's MI_02 as Juicetray reads it on Windows: 0001:0000 with no reports listed.
    fn xctech_shape() -> HidShape {
        HidShape {
            interface: Some(2),
            ..shape(0x0001, 0x0000, Vec::new())
        }
    }

    #[test]
    fn pick_xctech_takes_mi_02_among_the_m500_collections() {
        let input = |id, bytes| vec![report(ReportKind::Input, id, bytes)];
        let shapes = [
            shape(0x0001, 0x0002, input(0, 8)),
            shape(0x0001, 0x0006, input(1, 8)),
            shape(0x000C, 0x0001, input(3, 3)),
            shape(0xFFEF, 0x0000, both(5, 64)),
            xctech_shape(),
        ];
        assert_eq!(pick_target(Family::Xctech, &shapes), Ok(4));
        // A descriptor that does list the reports picks the same collection.
        let listed = shape(0x0001, 0x0000, both(0, 32));
        assert_eq!(pick_target(Family::Xctech, &[listed]), Ok(0));
        let other_usage = shape(0x0001, 0x0001, input(0, 32));
        assert_eq!(
            pick_target(Family::Xctech, &[other_usage]),
            Err(PickError::NoMatch)
        );
        assert_eq!(
            pick_target(Family::Xctech, &[xctech_shape(), xctech_shape()]),
            Err(PickError::Ambiguous(vec![0, 1]))
        );
        // Other families never take the Xctech collection.
        for family in [Family::Compx, Family::Driveall, Family::VgnKc2] {
            assert_eq!(
                pick_target(family, &[xctech_shape()]),
                Err(PickError::NoMatch),
                "{family:?}"
            );
        }
    }

    #[test]
    fn pick_generic_needs_the_row_page_and_report() {
        let output = Family::Generic(&crate::generic::tests::OUTPUT);
        let if4 = shape(0xFF70, 0x0071, both(0, 64));
        assert_eq!(pick_target(output, &[if4.clone(), driveall_shape()]), Ok(1));
        // Wrong size, wrong report id, or no input report for the reply: no target.
        let small = shape(0xFF60, 0x0061, both(0, 32));
        let numbered = shape(0xFF60, 0x0061, both(0x01, 65));
        let no_input = shape(0xFF60, 0x0061, vec![report(ReportKind::Output, 0, 64)]);
        assert_eq!(
            pick_target(output, &[if4, small, numbered, no_input]),
            Err(PickError::NoMatch)
        );
        assert_eq!(
            pick_target(output, &[driveall_shape(), driveall_shape()]),
            Err(PickError::Ambiguous(vec![0, 1]))
        );
        // A feature row needs its feature report; system and non-vendor pages never qualify.
        const FEATURE: GenericSpec = GenericSpec {
            usage_page: 0xFFFF,
            usage: 0x0002,
            report: GenericReport::Feature,
            report_bytes: 64,
            request: &[0x00, 0xF7],
            reply_prefix: &[0x00],
            level_byte: 2,
            charging: None,
            scale: 100,
        };
        const KEYBOARD: GenericSpec = GenericSpec {
            usage_page: 0x0001,
            usage: 0x0006,
            ..FEATURE
        };
        let feature = Family::Generic(&FEATURE);
        assert_eq!(pick_target(feature, &[rongyuan_shape()]), Ok(0));
        assert_eq!(
            pick_target(feature, &[driveall_shape()]),
            Err(PickError::NoMatch)
        );
        let keyboard = shape(0x0001, 0x0006, vec![report(ReportKind::Feature, 0, 64)]);
        assert_eq!(
            pick_target(Family::Generic(&KEYBOARD), &[keyboard]),
            Err(PickError::NoMatch)
        );
    }

    #[test]
    fn pick_serial_family_never_hid() {
        let shapes = [shape(0xFF02, 0x0002, both(0x08, 17))];
        assert_eq!(
            pick_target(Family::AtkSerial, &shapes),
            Err(PickError::NoMatch)
        );
    }
}
