//! Driveall (driveall.cn web driver; Weikav keyboards on Sonix 2.4 GHz dongles): output report 0
//! carrying `AA 10 30 00 00 00 01 00` (GET_DEVICE_INFO, 48 content bytes, last packet) on
//! FF60:0061, answered by an input report `55 10 …` whose payload starts at byte 8. No checksum
//! and no COMMUNICATION_START: the web driver's first call after connecting is GET_DEVICE_INFO.

use crate::{Decoded, padded, reading};

/// Report id 0 plus the 64-byte output report.
pub(crate) const FRAME_LEN: usize = 65;
/// Data bytes of the output and input reports (report id 0 is not counted).
pub(crate) const REPORT_LEN: usize = 64;
const REQUEST_HEADER: u8 = 0xAA;
const REPLY_HEADER: u8 = 0x55;
/// GET_DEVICE_INFO, read-only. The only command this family ever sends: the same channel takes
/// factory reset 0x0F, SET_* commands, profile 0x90 and OTA/bootloader 0x81-0x88.
const CMD_GET_DEVICE_INFO: u8 = 0x10;
/// Content size the web driver asks for (`contentSize:48`).
const CONTENT_SIZE: u8 = 0x30;
/// Header byte 6: 1 on the last (here only) packet of a request.
const LAST_PACKET: u8 = 0x01;
/// Report id 0, then the 8-byte header `AA cmd size addr_lo addr_hi 0 last 0`, rest zero.
pub(crate) const QUERY: [u8; FRAME_LEN] = padded(&[
    0x00,
    REQUEST_HEADER,
    CMD_GET_DEVICE_INFO,
    CONTENT_SIZE,
    0x00,
    0x00,
    0x00,
    LAST_PACKET,
    0x00,
]);
const _: () = assert!(QUERY[0] == 0 && QUERY[1] == REQUEST_HEADER);
const _: () = assert!(QUERY[2] == CMD_GET_DEVICE_INFO);

/// Raw indexes of the read reply (hidapi drops report id 0): payload byte 17 is batteryLevel
/// and payload byte 18 chargeStatus, after the 8-byte header.
const BATTERY_AT: usize = 8 + 17;
const CHARGE_AT: usize = 8 + 18;

/// Only `55 10 …` is the reply. Dongle notifications (`55 FC 06` keyboard asleep, `55 FC 04`
/// disconnected) and anything else are not, so the query ends with no reading. chargeStatus
/// values are undocumented in the web driver: nonzero is read as charging, unverified.
pub(crate) fn decode(bytes: &[u8]) -> Decoded {
    if bytes.get(..2) != Some(&[REPLY_HEADER, CMD_GET_DEVICE_INFO][..]) {
        return Decoded::NotTheReply;
    }
    let (Some(&percent), Some(&charge)) = (bytes.get(BATTERY_AT), bytes.get(CHARGE_AT)) else {
        return Decoded::Invalid("short reply");
    };
    reading(percent, charge != 0, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Family, Reading, battery_query, decode as decode_family, online_query};

    fn decode(bytes: &[u8]) -> Decoded {
        decode_family(Family::Driveall, bytes)
    }

    /// A 64-byte `55 10` reply with `battery` and `charge` in the payload.
    fn reply(battery: u8, charge: u8) -> [u8; REPORT_LEN] {
        let mut bytes: [u8; REPORT_LEN] = padded(&[REPLY_HEADER, CMD_GET_DEVICE_INFO, 0x30]);
        bytes[BATTERY_AT] = battery;
        bytes[CHARGE_AT] = charge;
        bytes
    }

    #[test]
    fn driveall_query_is_only_get_device_info() {
        let frame = battery_query(Family::Driveall);
        let mut expected = [0u8; FRAME_LEN];
        expected[..9].copy_from_slice(&[0x00, 0xAA, 0x10, 0x30, 0x00, 0x00, 0x00, 0x01, 0x00]);
        assert_eq!(frame, &expected[..]);
        // The command byte is GET_DEVICE_INFO; no other frame of this family exists.
        assert_eq!(frame[2], 0x10);
        assert_eq!(online_query(Family::Driveall), None);
    }

    #[test]
    fn driveall_reply_percent_and_charge() {
        let level = |percent, charging| {
            Decoded::Reading(Reading {
                percent,
                charging,
                asleep: false,
            })
        };
        assert_eq!(decode(&reply(87, 0)), level(87, false));
        assert_eq!(decode(&reply(100, 1)), level(100, true));
    }

    #[test]
    fn driveall_zero_and_out_of_range() {
        assert_eq!(decode(&reply(0, 0)), Decoded::Invalid("percent 0: no data"));
        assert_eq!(
            decode(&reply(101, 0)),
            Decoded::Invalid("percent out of range")
        );
    }

    #[test]
    fn driveall_notifications_and_others_are_not_the_reply() {
        // Keyboard asleep and disconnected notifications from the dongle.
        assert_eq!(
            decode(&padded::<64>(&[0x55, 0xFC, 0x06])),
            Decoded::NotTheReply
        );
        assert_eq!(
            decode(&padded::<64>(&[0x55, 0xFC, 0x04])),
            Decoded::NotTheReply
        );
        // Another command's reply, the request itself and an empty read.
        assert_eq!(
            decode(&padded::<64>(&[0x55, 0x11, 0x38])),
            Decoded::NotTheReply
        );
        assert_eq!(decode(&QUERY), Decoded::NotTheReply);
        assert_eq!(decode(&QUERY[1..]), Decoded::NotTheReply);
        assert_eq!(decode(&[]), Decoded::NotTheReply);
    }

    #[test]
    fn driveall_short_reply() {
        assert_eq!(
            decode(&reply(50, 0)[..CHARGE_AT]),
            Decoded::Invalid("short reply")
        );
    }
}
