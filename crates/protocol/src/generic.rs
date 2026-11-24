//! Generic: a catalogue row's own battery query instead of family code. build.rs turns the row's
//! fields into a `GenericSpec` (checked there: vendor page only, never 008C, request within the
//! report); the door sends its one fixed frame and this decoder reads the level from the reply.

use crate::{Decoded, reading};

/// How the request goes out and the reply comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericReport {
    /// An output report answered by an input report.
    Output,
    /// SetFeature, then GetFeature of the same report id.
    Feature,
}

/// One Generic catalogue row's query and reply layout. Indexes count the report id as byte 0.
#[derive(Debug, PartialEq, Eq)]
pub struct GenericSpec {
    pub usage_page: u16,
    pub usage: u16,
    pub report: GenericReport,
    /// The report's size as `juicetray scan` prints it (the id byte counted when not 0).
    pub report_bytes: u8,
    /// The only frame ever sent: report id first, zero-padded to the report.
    pub request: &'static [u8],
    /// The bytes a valid reply starts with, report id first.
    pub reply_prefix: &'static [u8],
    pub level_byte: u8,
    /// Byte index and mask; a nonzero masked value reads as charging.
    pub charging: Option<(u8, u8)>,
    /// Raw level of a full battery; percent = raw * 100 / scale.
    pub scale: u8,
}

/// Decodes one reply as read. hidapi drops an unused report id 0 from input reads (GetFeature
/// keeps it), so such a reply is matched and indexed one byte early.
pub(crate) fn decode(spec: &GenericSpec, bytes: &[u8]) -> Decoded {
    let dropped = usize::from(spec.report == GenericReport::Output && spec.reply_prefix[0] == 0);
    if !bytes.starts_with(&spec.reply_prefix[dropped..]) {
        return Decoded::NotTheReply;
    }
    let at = |index: u8| {
        usize::from(index)
            .checked_sub(dropped)
            .and_then(|at| bytes.get(at))
            .copied()
    };
    let Some(raw) = at(spec.level_byte) else {
        return Decoded::Invalid("short reply");
    };
    let charging = match spec.charging {
        None => false,
        Some((index, mask)) => match at(index) {
            Some(byte) => byte & mask != 0,
            None => return Decoded::Invalid("short reply"),
        },
    };
    let percent = u16::from(raw) * 100 / u16::from(spec.scale.max(1));
    match u8::try_from(percent) {
        Ok(percent) => reading(percent, charging, false),
        Err(_) => Decoded::Invalid("percent out of range"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{Family, Reading, battery_query, decode as decode_family, online_query, padded};

    /// A Driveall-shaped output row: report 0, reply `55 10 …` read without its id 0.
    pub(crate) static OUTPUT: GenericSpec = GenericSpec {
        usage_page: 0xFF60,
        usage: 0x0061,
        report: GenericReport::Output,
        report_bytes: 64,
        request: &padded::<65>(&[0x00, 0xAA, 0x10, 0x30, 0x00, 0x00, 0x00, 0x01]),
        reply_prefix: &[0x00, 0x55, 0x10],
        level_byte: 26,
        charging: Some((27, 0xFF)),
        scale: 100,
    };

    /// A feature row on report 5 whose level is 0-10.
    static FEATURE: GenericSpec = GenericSpec {
        usage_page: 0xFF00,
        usage: 0x0001,
        report: GenericReport::Feature,
        report_bytes: 8,
        request: &[0x05, 0xF7, 0, 0, 0, 0, 0, 0],
        reply_prefix: &[0x05, 0x01],
        level_byte: 3,
        charging: None,
        scale: 10,
    };

    fn level(percent: u8, charging: bool) -> Decoded {
        Decoded::Reading(Reading {
            percent,
            charging,
            asleep: false,
        })
    }

    /// An input report of OUTPUT as hidapi reads it (no id byte).
    fn output_reply(raw: u8, charge: u8) -> [u8; 64] {
        let mut bytes: [u8; 64] = padded(&[0x55, 0x10, 0x30]);
        bytes[25] = raw;
        bytes[26] = charge;
        bytes
    }

    #[test]
    fn generic_query_is_the_row_frame_only() {
        let frame = battery_query(Family::Generic(&OUTPUT));
        assert_eq!(frame, OUTPUT.request);
        assert_eq!(frame.len(), 65);
        assert_eq!(online_query(Family::Generic(&OUTPUT)), None);
        assert!(!Family::Generic(&OUTPUT).uses_feature_report());
        assert!(Family::Generic(&FEATURE).uses_feature_report());
    }

    #[test]
    fn generic_output_reply_reads_level_and_charging() {
        let decode = |bytes: &[u8]| decode_family(Family::Generic(&OUTPUT), bytes);
        assert_eq!(decode(&output_reply(87, 0)), level(87, false));
        assert_eq!(decode(&output_reply(100, 2)), level(100, true));
    }

    #[test]
    fn generic_feature_reply_scales_the_level() {
        let decode = |bytes: &[u8]| decode_family(Family::Generic(&FEATURE), bytes);
        // GetFeature keeps the id at [0]: raw 7 of 10 is 70 %.
        assert_eq!(decode(&[0x05, 0x01, 0x00, 7, 0, 0, 0, 0]), level(70, false));
        assert_eq!(decode(&[0x05, 0x01, 0x00, 10]), level(100, false));
    }

    #[test]
    fn generic_invalid_levels_are_never_shown() {
        let decode = |bytes: &[u8]| decode_family(Family::Generic(&OUTPUT), bytes);
        assert_eq!(
            decode(&output_reply(0, 0)),
            Decoded::Invalid("percent 0: no data")
        );
        assert_eq!(
            decode(&output_reply(101, 0)),
            Decoded::Invalid("percent out of range")
        );
        assert_eq!(
            decode(&output_reply(255, 0)),
            Decoded::Invalid("percent out of range")
        );
        // Over the scale is out of range too, not clamped.
        let feature = |bytes: &[u8]| decode_family(Family::Generic(&FEATURE), bytes);
        assert_eq!(
            feature(&[0x05, 0x01, 0x00, 11]),
            Decoded::Invalid("percent out of range")
        );
        assert_eq!(
            decode(&output_reply(50, 0)[..25]),
            Decoded::Invalid("short reply")
        );
        assert_eq!(
            decode(&output_reply(50, 0)[..26]),
            Decoded::Invalid("short reply")
        );
    }

    #[test]
    fn generic_prefix_mismatch_is_not_the_reply() {
        let decode = |bytes: &[u8]| decode_family(Family::Generic(&OUTPUT), bytes);
        assert_eq!(
            decode(&padded::<64>(&[0x55, 0xFC, 0x06])),
            Decoded::NotTheReply
        );
        assert_eq!(
            decode(&padded::<64>(&[0x55, 0x11, 0x30])),
            Decoded::NotTheReply
        );
        // The request itself, with or without its id, and an empty read.
        assert_eq!(decode(OUTPUT.request), Decoded::NotTheReply);
        assert_eq!(decode(&OUTPUT.request[1..]), Decoded::NotTheReply);
        assert_eq!(decode(&[]), Decoded::NotTheReply);
        // A reply carrying an id 0 byte is not matched as an id-0 input read.
        assert_eq!(decode(&[0x00, 0x55, 0x10, 0x30]), Decoded::NotTheReply);
        let feature = |bytes: &[u8]| decode_family(Family::Generic(&FEATURE), bytes);
        assert_eq!(feature(&[0x06, 0x01, 0x00, 7]), Decoded::NotTheReply);
        assert_eq!(feature(&[0x05, 0x02, 0x00, 7]), Decoded::NotTheReply);
    }
}
