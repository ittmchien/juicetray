//! VGN "report 0x13" (receiver 3554:FA09): 20-byte HID report, id `0x13`, command `0x4A`.

use crate::{Decoded, padded, reading};

const REPORT_ID: u8 = 0x13;
const CMD_BATTERY: u8 = 0x4A;
const REPORT_LEN: usize = 20;
const STATE_DISCHARGING: u8 = 0x01;
const STATE_ON_CABLE: u8 = 0x10;
/// Shown in the menu as is; the device reports a level that is not trustworthy while on cable.
pub const ON_CABLE_TEXT: &str = "on cable";

// The last byte is the checksum of the 19 before it.
pub(crate) const QUERY: [u8; REPORT_LEN] = {
    let mut frame = padded(&[REPORT_ID, CMD_BATTERY]);
    let (body, _) = frame.split_at(REPORT_LEN - 1);
    frame[REPORT_LEN - 1] = checksum(body);
    frame
};

/// Wrapping 8-bit sum of the 19 bytes before the checksum (report id included).
const fn checksum(first19: &[u8]) -> u8 {
    let mut sum = 0u8;
    let mut i = 0;
    while i < first19.len() {
        sum = sum.wrapping_add(first19[i]);
        i += 1;
    }
    sum
}

pub(crate) fn decode(bytes: &[u8]) -> Decoded {
    let Some(report) = bytes.first_chunk::<REPORT_LEN>() else {
        return Decoded::NotTheReply;
    };
    // The device may set the high bit of the command byte in its answer.
    if report[0] != REPORT_ID || report[1] & 0x7F != CMD_BATTERY {
        return Decoded::NotTheReply;
    }
    let (body, tail) = report.split_at(REPORT_LEN - 1);
    if checksum(body) != tail[0] {
        return Decoded::Invalid("bad checksum");
    }
    match report[6] {
        STATE_DISCHARGING => reading(report[5], false, false),
        STATE_ON_CABLE => Decoded::Invalid(ON_CABLE_TEXT),
        _ => Decoded::Invalid("unknown power state"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Family, Reading, battery_query, decode as decode_family};

    // Hand-made replies: 90 % discharging, and on cable; the last byte is the byte sum.
    const ON_BATTERY_90: [u8; 20] = [
        0x13, 0x4a, 0x01, 0x00, 0x02, 0x5a, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xbb,
    ];
    const ON_CABLE: [u8; 20] = [
        0x13, 0x4a, 0x01, 0x00, 0x02, 0x63, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xd3,
    ];

    /// Hand-made reply: 84 %, discharging, checksum 0xB2.
    fn reply() -> [u8; 20] {
        sealed([
            0x13, 0x4a, 0, 0, 0, 0x54, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ])
    }

    fn sealed(mut frame: [u8; 20]) -> [u8; 20] {
        frame[19] = checksum(&frame[..19]);
        frame
    }

    fn decode(bytes: &[u8]) -> Decoded {
        decode_family(Family::Report13, bytes)
    }

    fn battery(percent: u8) -> Decoded {
        Decoded::Reading(Reading {
            percent,
            charging: false,
            asleep: false,
        })
    }

    #[test]
    fn report13_query_bytes() {
        let mut expected = [0u8; 20];
        expected[0] = 0x13;
        expected[1] = 0x4A;
        expected[19] = 0x5D;
        assert_eq!(battery_query(Family::Report13), &expected[..]);
    }

    #[test]
    fn report13_captured_replies() {
        assert_eq!(decode(&ON_BATTERY_90), battery(90));
        assert_eq!(decode(&ON_CABLE), Decoded::Invalid(ON_CABLE_TEXT));
    }

    #[test]
    fn report13_hand_made_reply_decodes() {
        assert_eq!(reply()[19], 0xB2);
        assert_eq!(decode(&reply()), battery(84));
    }

    #[test]
    fn report13_unknown_state() {
        let mut frame = reply();
        frame[6] = 0x02;
        assert_eq!(
            decode(&sealed(frame)),
            Decoded::Invalid("unknown power state")
        );
    }

    #[test]
    fn report13_bad_checksum() {
        let mut frame = ON_BATTERY_90;
        frame[19] ^= 0x01;
        assert_eq!(decode(&frame), Decoded::Invalid("bad checksum"));
    }

    #[test]
    fn report13_other_report_or_cmd() {
        let mut other_report = ON_BATTERY_90;
        other_report[0] = 0x14;
        assert_eq!(decode(&sealed(other_report)), Decoded::NotTheReply);
        let mut other_cmd = ON_BATTERY_90;
        other_cmd[1] = 0x4B;
        assert_eq!(decode(&sealed(other_cmd)), Decoded::NotTheReply);
    }

    #[test]
    fn report13_high_bit_of_command_still_matches() {
        let mut frame = ON_BATTERY_90;
        frame[1] = 0xCA;
        assert_eq!(decode(&sealed(frame)), battery(90));
    }

    #[test]
    fn report13_percent_out_of_range() {
        let mut zero = ON_BATTERY_90;
        zero[5] = 0;
        assert_eq!(
            decode(&sealed(zero)),
            Decoded::Invalid("percent 0: no data")
        );
        let mut over = ON_BATTERY_90;
        over[5] = 101;
        assert_eq!(
            decode(&sealed(over)),
            Decoded::Invalid("percent out of range")
        );
    }

    #[test]
    fn report13_short() {
        assert_eq!(decode(&ON_BATTERY_90[..19]), Decoded::NotTheReply);
    }
}
