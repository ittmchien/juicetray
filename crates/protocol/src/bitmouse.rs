//! ATK "BITMOUSE" (X1 SE dongle 373B:1216): 64-byte HID output report id `0x08`, command `0x07`.
//! Layout from the ATK HUB app (BITMOUSE driver); no capture yet.

use crate::{Decoded, padded, reading};

pub(crate) const REPORT_ID: u8 = 0x08;
pub(crate) const REPORT_LEN: usize = 64;
/// Smallest input report that still holds the percent byte.
pub(crate) const MIN_REPLY_LEN: usize = 8;
const HEADER: u8 = 0x72;
const PUSH_HEADER: u8 = 0x73;
const STATUS_ERROR: u8 = 0xFF;
const PARAM_LEN: u8 = 0x02;
const SN: u8 = 0x00;
const TARGET_MOUSE_VIA_DONGLE: u8 = 0x01;
const CMD_BATTERY: u8 = 0x07;
const CMD_LEN: u8 = 0x01;

// Wire frame: report id, checksum, then the body the checksum covers.
pub(crate) const QUERY: [u8; REPORT_LEN] = {
    let mut frame = padded(&[
        REPORT_ID,
        0,
        HEADER,
        PARAM_LEN,
        SN,
        TARGET_MOUSE_VIA_DONGLE,
        CMD_BATTERY,
        CMD_LEN,
    ]);
    frame[1] = checksum(frame.split_at(2).1);
    frame
};

/// Wrapping 8-bit sum of the bytes after the checksum.
const fn checksum(body: &[u8]) -> u8 {
    let mut sum = 0u8;
    let mut i = 0;
    while i < body.len() {
        sum = sum.wrapping_add(body[i]);
        i += 1;
    }
    sum
}

pub(crate) fn decode(bytes: &[u8]) -> Decoded {
    if bytes.len() < MIN_REPLY_LEN || bytes[0] != REPORT_ID {
        return Decoded::NotTheReply;
    }
    // Our own request echoed back has 0x01 here, so it never matches.
    if bytes[2] == PUSH_HEADER || bytes[5] != CMD_BATTERY {
        return Decoded::NotTheReply;
    }
    if bytes[2] == STATUS_ERROR {
        return Decoded::Invalid("device returned an error");
    }
    // Charging needs a second command that is not sent.
    reading(bytes[7], false, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Family, Reading, battery_query, decode as decode_family};

    // Synthetic: layout from the ATK HUB code, no capture yet.
    fn reply() -> Vec<u8> {
        let mut frame = vec![0x08, 0x00, 0x00, 0x00, 0x01, 0x07, 0x01, 0x54];
        frame.resize(127, 0);
        frame
    }

    fn decode(bytes: &[u8]) -> Decoded {
        decode_family(Family::AtkBitmouse, bytes)
    }

    #[test]
    fn bitmouse_query_bytes() {
        let mut expected = [0u8; 64];
        expected[..8].copy_from_slice(&[0x08, 0x7D, 0x72, 0x02, 0x00, 0x01, 0x07, 0x01]);
        assert_eq!(battery_query(Family::AtkBitmouse), &expected[..]);
        let q = battery_query(Family::AtkBitmouse);
        assert_eq!(checksum(&q[2..]), q[1]);
        assert_eq!(q[1], 0x7D);
    }

    #[test]
    fn bitmouse_reply_decodes() {
        assert_eq!(
            decode(&reply()),
            Decoded::Reading(Reading {
                percent: 84,
                charging: false,
                asleep: false
            })
        );
    }

    #[test]
    fn bitmouse_error_status() {
        let mut frame = reply();
        frame[2] = 0xFF;
        assert_eq!(decode(&frame), Decoded::Invalid("device returned an error"));
    }

    #[test]
    fn bitmouse_unsolicited_push() {
        let mut frame = reply();
        frame[2] = 0x73;
        assert_eq!(decode(&frame), Decoded::NotTheReply);
    }

    #[test]
    fn bitmouse_percent_out_of_range() {
        let mut zero = reply();
        zero[7] = 0;
        assert_eq!(decode(&zero), Decoded::Invalid("percent 0: no data"));
        let mut over = reply();
        over[7] = 101;
        assert_eq!(decode(&over), Decoded::Invalid("percent out of range"));
    }

    #[test]
    fn bitmouse_other_report_or_command() {
        let mut other_report = reply();
        other_report[0] = 0x09;
        assert_eq!(decode(&other_report), Decoded::NotTheReply);
        let mut other_cmd = reply();
        other_cmd[5] = 0x06;
        assert_eq!(decode(&other_cmd), Decoded::NotTheReply);
    }

    #[test]
    fn bitmouse_echoed_query_is_not_the_reply() {
        assert_eq!(
            decode(battery_query(Family::AtkBitmouse)),
            Decoded::NotTheReply
        );
    }

    #[test]
    fn bitmouse_short() {
        assert_eq!(decode(&reply()[..7]), Decoded::NotTheReply);
        assert_eq!(decode(&[]), Decoded::NotTheReply);
    }
}
