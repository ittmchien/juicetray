//! HID report-descriptor parsing: the size of every report of every top-level collection.
//! Item format: USB HID 1.11 section 6.2.2. Pure; never panics, whatever the input.

const MAIN: u8 = 0;
const GLOBAL: u8 = 1;
const LOCAL: u8 = 2;
const LONG_ITEM: u8 = 0xFE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    Input,
    Output,
    Feature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportSize {
    pub kind: ReportKind,
    pub id: u8,
    /// Report length in bytes, including the id byte when `id != 0`.
    pub bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopCollection {
    pub usage_page: u16,
    pub usage: u16,
    pub reports: Vec<ReportSize>,
}

/// Global item state; Push saves a copy, Pop restores it.
#[derive(Clone, Copy, Default)]
struct Globals {
    usage_page: u16,
    field_bits: u32,
    field_count: u32,
    report_id: u8,
}

/// A top collection while its reports are still bit counts.
struct OpenTop {
    usage_page: u16,
    usage: u16,
    bits: Vec<(ReportKind, u8, u64)>,
}

/// The first Usage item since the last main item: an explicit page (4-byte form) and the usage.
type FirstUsage = (Option<u16>, u16);

/// Parses a descriptor into its top-level collections with per-report sizes.
pub fn parse_descriptor(bytes: &[u8]) -> Result<Vec<TopCollection>, String> {
    let mut tops: Vec<OpenTop> = Vec::new();
    let mut globals = Globals::default();
    let mut stack: Vec<Globals> = Vec::new();
    let mut first_usage: Option<FirstUsage> = None;
    let mut depth = 0usize;
    let mut pos = 0usize;

    while let Some(&prefix) = bytes.get(pos) {
        pos += 1;

        // A long item carries its data length in the next byte; its content is not used.
        if prefix == LONG_ITEM {
            let data_len = usize::from(*bytes.get(pos).ok_or("truncated long item")?);
            pos += 2 + data_len;
            if pos > bytes.len() {
                return Err("truncated long item".into());
            }
            continue;
        }

        let size = [0, 1, 2, 4][usize::from(prefix & 3)];
        let kind = (prefix >> 2) & 3;
        let tag = prefix >> 4;
        let data_bytes = bytes.get(pos..pos + size).ok_or("truncated item")?;
        pos += size;
        // Item data is unsigned little-endian.
        let data = data_bytes
            .iter()
            .rev()
            .fold(0u32, |acc, &byte| (acc << 8) | u32::from(byte));

        match (kind, tag) {
            (GLOBAL, 0x0) => {
                globals.usage_page = u16::try_from(data).map_err(|_| "usage page out of range")?;
            }
            (GLOBAL, 0x7) => globals.field_bits = data,
            (GLOBAL, 0x8) => {
                globals.report_id = u8::try_from(data).map_err(|_| "report id out of range")?;
            }
            (GLOBAL, 0x9) => globals.field_count = data,
            (GLOBAL, 0xA) => stack.push(globals),
            (GLOBAL, 0xB) => globals = stack.pop().ok_or("pop without push")?,
            // A 4-byte usage carries its page in the high 16 bits.
            (LOCAL, 0x0) if first_usage.is_none() => {
                first_usage = Some(if size == 4 {
                    (Some((data >> 16) as u16), data as u16)
                } else {
                    (None, data as u16)
                });
            }
            (MAIN, 0xA) => {
                if depth == 0 {
                    let (page, usage) = first_usage.unwrap_or((None, 0));
                    tops.push(OpenTop {
                        usage_page: page.unwrap_or(globals.usage_page),
                        usage,
                        bits: Vec::new(),
                    });
                }
                depth += 1;
            }
            (MAIN, 0xC) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or("end collection without collection")?;
            }
            (MAIN, 0x8 | 0x9 | 0xB) => {
                // Reports outside any collection belong to no top and are ignored.
                if let Some(top) = tops.last_mut().filter(|_| depth > 0) {
                    let report_kind = match tag {
                        0x8 => ReportKind::Input,
                        0x9 => ReportKind::Output,
                        _ => ReportKind::Feature,
                    };
                    add_bits(top, report_kind, &globals)?;
                }
            }
            _ => {}
        }

        // Local items last until the next main item.
        if kind == MAIN {
            first_usage = None;
        }
    }

    // A descriptor cut on an item boundary would parse with smaller sizes; refuse it.
    if depth != 0 {
        return Err("unclosed collection".into());
    }
    tops.into_iter().map(finish).collect()
}

/// Adds one field run (`size x count` bits) to the report `(kind, id)` of `top`.
fn add_bits(top: &mut OpenTop, kind: ReportKind, globals: &Globals) -> Result<(), String> {
    // u32 x u32 always fits in u64.
    let bits = u64::from(globals.field_bits) * u64::from(globals.field_count);
    let id = globals.report_id;
    match top
        .bits
        .iter_mut()
        .find(|(k, report_id, _)| *k == kind && *report_id == id)
    {
        Some(entry) => entry.2 = entry.2.checked_add(bits).ok_or("report size overflow")?,
        None => top.bits.push((kind, id, bits)),
    }
    Ok(())
}

/// Turns bit counts into byte lengths: `ceil(bits / 8)`, plus the id byte when `id != 0`.
fn finish(top: OpenTop) -> Result<TopCollection, String> {
    let mut reports = Vec::with_capacity(top.bits.len());
    for (kind, id, bits) in top.bits {
        let body = usize::try_from(bits.div_ceil(8)).map_err(|_| "report too large")?;
        let bytes = body
            .checked_add(usize::from(id != 0))
            .ok_or("report too large")?;
        reports.push(ReportSize { kind, id, bytes });
    }
    Ok(TopCollection {
        usage_page: top.usage_page,
        usage: top.usage,
        reports,
    })
}

/// Reports of the top collection with this page/usage; else, when the descriptor has exactly
/// one top collection, its reports; else none.
pub fn reports_of(tops: &[TopCollection], usage_page: u16, usage: u16) -> Vec<ReportSize> {
    tops.iter()
        .find(|top| top.usage_page == usage_page && top.usage == usage)
        .or(match tops {
            [only] => Some(only),
            _ => None,
        })
        .map(|top| top.reports.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPX: &str =
        "06 02 ff 09 02 a1 01 85 08 09 01 15 00 26 ff 00 75 08 95 10 81 02 09 01 95 10 91 02 c0";
    const MCHOSE: &str =
        "06 07 ff 0a 22 02 a1 01 85 55 09 01 75 08 95 3f 81 02 09 01 95 3f 91 02 c0";
    const NO_ID: &str = "06 00 ff 09 01 a1 01 75 08 95 40 81 02 c0";
    // Boot-style mouse: a nested physical collection, 3 button bits + 5 padding bits.
    const MOUSE: &str = "05 01 09 02 a1 01 09 01 a1 00 05 09 19 01 29 03 15 00 25 01 95 03 75 01 81 02 95 01 75 05 81 03 c0 c0";

    fn hex(text: &str) -> Vec<u8> {
        text.split_whitespace()
            .map(|byte| u8::from_str_radix(byte, 16).expect("valid hex byte"))
            .collect()
    }

    fn parse(text: &str) -> Vec<TopCollection> {
        parse_descriptor(&hex(text)).expect("descriptor parses")
    }

    fn report(kind: ReportKind, id: u8, bytes: usize) -> ReportSize {
        ReportSize { kind, id, bytes }
    }

    fn compx_top() -> TopCollection {
        TopCollection {
            usage_page: 0xFF02,
            usage: 0x0002,
            reports: vec![
                report(ReportKind::Input, 0x08, 17),
                report(ReportKind::Output, 0x08, 17),
            ],
        }
    }

    #[test]
    fn compx_descriptor_is_29_bytes() {
        assert_eq!(hex(COMPX).len(), 29);
    }

    #[test]
    fn parse_compx_like() {
        assert_eq!(parse(COMPX), vec![compx_top()]);
    }

    #[test]
    fn parse_mchose_like() {
        assert_eq!(
            parse(MCHOSE),
            vec![TopCollection {
                usage_page: 0xFF07,
                usage: 0x0222,
                reports: vec![
                    report(ReportKind::Input, 0x55, 64),
                    report(ReportKind::Output, 0x55, 64),
                ],
            }]
        );
    }

    #[test]
    fn parse_two_top_collections() {
        let tops = parse(&format!("{MOUSE} {COMPX}"));
        assert_eq!(tops.len(), 2);
        assert_eq!((tops[0].usage_page, tops[0].usage), (0x0001, 0x0002));
        assert_eq!(tops[0].reports, vec![report(ReportKind::Input, 0, 1)]);
        assert_eq!(tops[1], compx_top());
    }

    #[test]
    fn parse_no_report_id() {
        assert_eq!(
            parse(NO_ID),
            vec![TopCollection {
                usage_page: 0xFF00,
                usage: 0x0001,
                reports: vec![report(ReportKind::Input, 0, 64)],
            }]
        );
    }

    #[test]
    fn parse_feature_report_and_split_fields() {
        // Two input runs of one report add up; a Feature report is kept apart.
        let tops = parse("06 00 ff 09 01 a1 01 75 08 95 02 81 02 95 03 81 02 95 04 b1 02 c0");
        assert_eq!(
            tops[0].reports,
            vec![
                report(ReportKind::Input, 0, 5),
                report(ReportKind::Feature, 0, 4),
            ]
        );
    }

    #[test]
    fn parse_push_pop() {
        // Size 8 is saved, 16 is set, Pop restores 8: 4 x 8 bits = 4 bytes.
        let tops = parse("06 00 ff 09 01 a1 01 75 08 a4 75 10 b4 95 04 81 02 c0");
        assert_eq!(tops[0].reports, vec![report(ReportKind::Input, 0, 4)]);
    }

    #[test]
    fn parse_extended_usage() {
        let tops = parse("0b 02 00 02 ff a1 01 75 08 95 01 81 02 c0");
        assert_eq!((tops[0].usage_page, tops[0].usage), (0xFF02, 0x0002));
    }

    #[test]
    fn parse_long_item_skipped() {
        let tops = parse(&format!("fe 02 00 aa bb {NO_ID}"));
        assert_eq!(tops[0].reports, vec![report(ReportKind::Input, 0, 64)]);
    }

    #[test]
    fn parse_ignores_reports_outside_a_collection() {
        let tops = parse(&format!("75 08 95 04 81 02 {NO_ID}"));
        assert_eq!(tops.len(), 1);
        assert_eq!(tops[0].reports, vec![report(ReportKind::Input, 0, 64)]);
    }

    #[test]
    fn parse_errors() {
        for bad in [
            "06 02",
            // Unclosed collection, and a usage page wider than 16 bits.
            "06 00 ff 09 01 a1 01 75 08 95 01 81 02",
            "07 00 00 01 00 09 01 a1 01 c0",
            "c0",
            "b4",
            "fe",
            "fe 05 00 01",
            "06 00 ff 09 01 a1 01 ff",
        ] {
            assert!(parse_descriptor(&hex(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn parse_report_id_out_of_range_is_error() {
        assert!(parse_descriptor(&hex("86 00 01")).is_err());
    }

    #[test]
    fn parse_size_overflow_is_error() {
        // Two inputs of (2^32 - 1)^2 bits overflow u64.
        let descriptor = "06 00 ff 09 01 a1 01 77 ff ff ff ff 97 ff ff ff ff 81 02 81 02 c0";
        assert!(parse_descriptor(&hex(descriptor)).is_err());
    }

    #[test]
    fn parse_never_panics_on_truncation_or_garbage() {
        let full = hex(&format!("{MOUSE} {COMPX}"));
        for end in 0..=full.len() {
            let _ = parse_descriptor(&full[..end]);
        }
        for first in 0..=u8::MAX {
            let _ = parse_descriptor(&[first, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
            let _ = parse_descriptor(&[0xA1, 0x01, first, 0xFF, 0xFF, 0xFF, 0xFF, 0x81, 0x02]);
        }
    }

    #[test]
    fn reports_of_picks_matching_top() {
        let tops = parse(&format!("{MOUSE} {COMPX}"));
        assert_eq!(reports_of(&tops, 0xFF02, 0x0002), compx_top().reports);
        assert_eq!(
            reports_of(&tops, 0x0001, 0x0002),
            vec![report(ReportKind::Input, 0, 1)]
        );
        // No match among several tops: nothing.
        assert!(reports_of(&tops, 0xFF99, 0x0001).is_empty());
    }

    #[test]
    fn reports_of_falls_back_to_the_only_top() {
        let tops = parse(COMPX);
        assert_eq!(reports_of(&tops, 0xFF99, 0x0001), compx_top().reports);
        assert!(reports_of(&[], 0xFF02, 0x0002).is_empty());
    }
}
