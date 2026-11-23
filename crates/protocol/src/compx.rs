//! Compx "report 8" (VGN, ATK X1 SE): 17-byte HID report, id `0x08`, command `0x04`.

use crate::{Decoded, padded, reading};

const REPORT_ID: u8 = 0x08;
const CMD_BATTERY: u8 = 0x04;
const REPORT_LEN: usize = 17;

// The last byte is the checksum of the 16 before it.
pub(crate) const QUERY: [u8; REPORT_LEN] = {
    let mut frame = padded(&[REPORT_ID, CMD_BATTERY]);
    let (body, _) = frame.split_at(REPORT_LEN - 1);
    frame[REPORT_LEN - 1] = checksum(body);
    frame
};

/// `0x55` minus the sum of the 16 bytes before the checksum, modulo 256.
pub const fn checksum(first16: &[u8]) -> u8 {
    let mut checksum = 0x55u8;
    let mut i = 0;
    while i < first16.len() {
        checksum = checksum.wrapping_sub(first16[i]);
        i += 1;
    }
    checksum
}

pub(crate) fn decode(bytes: &[u8]) -> Decoded {
    let Some(report) = bytes.first_chunk::<REPORT_LEN>() else {
        return Decoded::NotTheReply;
    };
    if report[0] != REPORT_ID || report[1] != CMD_BATTERY {
        return Decoded::NotTheReply;
    }
    let (body, tail) = report.split_at(REPORT_LEN - 1);
    if checksum(body) != tail[0] {
        return Decoded::Invalid("bad checksum");
    }
    reading(report[6], report[7] != 0, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Family, Reading, battery_query, decode as decode_family};

    const REPLY: [u8; 17] = [
        0x08, 0x04, 0, 0, 0, 0, 0x54, 0x01, 0x0f, 0xa0, 0, 0, 0, 0, 0, 0, 0x45,
    ];

    fn decode(bytes: &[u8]) -> Decoded {
        decode_family(Family::Compx, bytes)
    }

    #[test]
    fn compx_query_bytes() {
        let mut expected = [0u8; 17];
        expected[0] = 0x08;
        expected[1] = 0x04;
        expected[16] = 0x49;
        assert_eq!(battery_query(Family::Compx), &expected[..]);
        assert_eq!(battery_query(Family::Compx).len(), 17);
    }

    #[test]
    fn compx_checksum_formula() {
        let q = battery_query(Family::Compx);
        assert_eq!(checksum(&q[..16]), q[16]);
        assert_eq!(q[16], 0x49);
    }

    #[test]
    fn compx_reply_decodes() {
        assert_eq!(
            decode(&REPLY),
            Decoded::Reading(Reading {
                percent: 84,
                charging: true,
                asleep: false
            })
        );
        let mut idle = REPLY;
        idle[7] = 0x00;
        idle[16] = 0x46;
        assert_eq!(
            decode(&idle),
            Decoded::Reading(Reading {
                percent: 84,
                charging: false,
                asleep: false
            })
        );
    }

    #[test]
    fn compx_bad_checksum() {
        let mut reply = REPLY;
        reply[16] = 0x44;
        assert_eq!(decode(&reply), Decoded::Invalid("bad checksum"));
    }

    #[test]
    fn compx_percent_101() {
        let mut reply = REPLY;
        reply[6] = 0x65;
        reply[16] = 0x34;
        assert_eq!(decode(&reply), Decoded::Invalid("percent out of range"));
    }

    #[test]
    fn compx_other_report_or_cmd() {
        let mut other_report = REPLY;
        other_report[0] = 0x09;
        assert_eq!(decode(&other_report), Decoded::NotTheReply);
        let mut other_cmd = REPLY;
        other_cmd[1] = 0x03;
        assert_eq!(decode(&other_cmd), Decoded::NotTheReply);
    }

    #[test]
    fn compx_echoed_query_is_not_a_reading() {
        assert_eq!(
            decode(battery_query(Family::Compx)),
            Decoded::Invalid("percent 0: no data")
        );
    }

    #[test]
    fn compx_short() {
        assert_eq!(decode(&REPLY[..16]), Decoded::NotTheReply);
    }
}
