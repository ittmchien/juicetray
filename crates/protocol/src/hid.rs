//! HID collection rules: which collections may be opened, and which one a family's query goes to.

use crate::{Family, ReportKind, ReportSize};

/// First usage page of the vendor-defined range.
const VENDOR_PAGE_MIN: u16 = 0xFF00;

const COMPX_REPORT_ID: u8 = 0x08;
const COMPX_REPORT_LEN: usize = 17;
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

/// One verdict per `(path, usage page, usage)` entry: `Ok` only for a vendor collection that is
/// not a system mouse/keyboard and whose path no mouse/keyboard collection shares.
pub fn open_permission(entries: &[(&str, u16, u16)]) -> Vec<Result<(), &'static str>> {
    // macOS lists every usage pair of an interface under one path, so opening one opens all.
    let shared_with_system = |path: &str| {
        entries
            .iter()
            .any(|&(other, page, usage)| other == path && system_reason(page, usage).is_some())
    };
    entries
        .iter()
        .map(|&(path, usage_page, usage)| {
            if let Some(reason) = system_reason(usage_page, usage) {
                return Err(reason);
            }
            if usage_page < VENDOR_PAGE_MIN {
                return Err("not a vendor collection");
            }
            if shared_with_system(path) {
                return Err("shares its interface with a mouse or keyboard");
            }
            Ok(())
        })
        .collect()
}

/// Index of the collection `family`'s battery query goes to. `AtkSerial` is never HID.
pub fn pick_target(family: Family, shapes: &[HidShape]) -> Result<usize, PickError> {
    match family {
        Family::Compx => pick_compx(shapes),
        Family::Mchose => pick_mchose(shapes),
        Family::AtkSerial => Err(PickError::NoMatch),
    }
}

fn has_report(shape: &HidShape, kind: ReportKind, id: u8, bytes: usize) -> bool {
    shape
        .reports
        .iter()
        .any(|report| report.kind == kind && report.id == id && report.bytes == bytes)
}

fn matching(shapes: &[HidShape], accepts: impl Fn(&HidShape) -> bool) -> Vec<usize> {
    shapes
        .iter()
        .enumerate()
        .filter(|(_, shape)| accepts(shape))
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

/// Vendor collection with a 17-byte id-8 report in both directions; several candidates are
/// narrowed to `FF02/0002`.
fn pick_compx(shapes: &[HidShape]) -> Result<usize, PickError> {
    let candidates = matching(shapes, |shape| {
        shape.usage_page >= VENDOR_PAGE_MIN
            && has_report(shape, ReportKind::Input, COMPX_REPORT_ID, COMPX_REPORT_LEN)
            && has_report(shape, ReportKind::Output, COMPX_REPORT_ID, COMPX_REPORT_LEN)
    });
    if candidates.len() <= 1 {
        return exactly_one(candidates);
    }
    let preferred: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|&index| shapes[index].usage_page == 0xFF02 && shapes[index].usage == 0x0002)
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

    #[test]
    fn open_permission_distinct_paths() {
        let verdicts = open_permission(&[
            ("A", 0xFF02, 2),
            ("B", 0x01, 2),
            ("C", 0x01, 6),
            ("D", 0x0C, 1),
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
        let verdicts = open_permission(&[("P", 0xFF00, 1), ("P", 0x01, 6)]);
        assert_eq!(verdicts, vec![Err(SHARED), Err("system keyboard")]);
    }

    #[test]
    fn open_permission_shared_with_mouse_and_empty() {
        let verdicts = open_permission(&[("P", 0xFF00, 1), ("P", 0x01, 2), ("Q", 0xFF00, 1)]);
        assert_eq!(verdicts, vec![Err(SHARED), Err("system mouse"), Ok(())]);
        assert!(open_permission(&[]).is_empty());
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
    fn pick_serial_family_never_hid() {
        let shapes = [shape(0xFF02, 0x0002, both(0x08, 17))];
        assert_eq!(
            pick_target(Family::AtkSerial, &shapes),
            Err(PickError::NoMatch)
        );
    }
}
