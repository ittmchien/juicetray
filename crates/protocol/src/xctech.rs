//! XCTECH receivers (Delux M500 receiver 249A:5C2F, the mouse on its cable 248A:5D2E): output
//! report 0 carrying `10` (device info, the M500 app's only getter) on the 0001:0000 collection
//! of interface 2, answered by a 32-byte input report `10 00 …` that ends in a checksum.

use crate::{Decoded, ON_CABLE_TEXT, padded, reading};

/// Report id 0 plus the 32-byte output report.
pub(crate) const FRAME_LEN: usize = 33;
/// Data bytes of the output and input reports (report id 0 is not counted).
pub(crate) const REPORT_LEN: usize = 32;
/// Device info, read-only. The only command this family ever sends: the same channel takes the
/// M500 app's setters 0x02, 0x04, 0x05, 0x07, 0x08, 0x09, 0x0F and 0x11.
const CMD_DEVICE_INFO: u8 = 0x10;
const STATUS_OK: u8 = 0x00;
/// Report id 0, then the command; the all-zero body's checksum is 0, so the frame is complete.
pub(crate) const QUERY: [u8; FRAME_LEN] = padded(&[0x00, CMD_DEVICE_INFO]);

/// Raw indexes of the read reply (hidapi drops report id 0).
const STATUS_AT: usize = 1;
const CHARGING_AT: usize = 12;
const PERCENT_AT: usize = 13;
const LINK_AT: usize = 14;
/// The last byte is the wrapping sum of the bytes from index 4 up to it.
const CHECKSUM_FROM: usize = 4;
const CHECKSUM_AT: usize = REPORT_LEN - 1;
const CHARGING: u8 = 1;
const NOT_LINKED: u8 = 0;

/// Only `10 00 …` is the reply: `C0` link and `C2` DPI notifications and the request's echo are
/// not. Charging means the mouse is on its cable, whose level byte is not the real level; link 0
/// means the mouse is off and the level is the receiver's last one.
pub(crate) fn decode(bytes: &[u8]) -> Decoded {
    let Some(report) = bytes.first_chunk::<REPORT_LEN>() else {
        return Decoded::NotTheReply;
    };
    let is_echo = report[1..].iter().all(|&byte| byte == 0);
    if report[0] != CMD_DEVICE_INFO || report[STATUS_AT] != STATUS_OK || is_echo {
        return Decoded::NotTheReply;
    }
    let sum = report[CHECKSUM_FROM..CHECKSUM_AT]
        .iter()
        .fold(0u8, |sum, &byte| sum.wrapping_add(byte));
    if sum != report[CHECKSUM_AT] {
        return Decoded::Invalid("bad checksum");
    }
    if report[CHARGING_AT] == CHARGING {
        return Decoded::Invalid(ON_CABLE_TEXT);
    }
    reading(report[PERCENT_AT], false, report[LINK_AT] == NOT_LINKED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Family, Reading, battery_query, decode as decode_family, online_query};

    fn decode(bytes: &[u8]) -> Decoded {
        decode_family(Family::Xctech, bytes)
    }

    /// A reply from its first 16 bytes (the rest zero, as captured) and its checksum byte.
    fn capture(head: &[u8], checksum: u8) -> [u8; REPORT_LEN] {
        let mut bytes: [u8; REPORT_LEN] = padded(head);
        bytes[CHECKSUM_AT] = checksum;
        bytes
    }

    /// The receiver with the mouse off: 55 %, link 0 (checksum as captured).
    fn mouse_off() -> [u8; REPORT_LEN] {
        let head = [
            0x10, 0x00, 0x01, 0x0b, 0x4d, 0x35, 0x30, 0x30, 0x00, 0x00, 0x11, 0x01, 0x00, 0x37,
            0x00, 0x00,
        ];
        capture(&head, 0x2b)
    }

    /// The receiver with the mouse in use: 85 %, link 1 (checksum as captured).
    fn mouse_in_use() -> [u8; REPORT_LEN] {
        let head = [
            0x10, 0x00, 0x01, 0x0b, 0x4d, 0x35, 0x30, 0x30, 0x00, 0x00, 0x11, 0x01, 0x00, 0x55,
            0x01, 0x00,
        ];
        capture(&head, 0x4a)
    }

    /// The mouse on its cable (248A:5D2E): charging 1, level byte 100, link 1 (as captured).
    fn on_cable() -> [u8; REPORT_LEN] {
        let head = [
            0x10, 0x00, 0x01, 0x0b, 0x4d, 0x35, 0x30, 0x30, 0x15, 0x03, 0x11, 0x01, 0x01, 0x64,
            0x01, 0x00,
        ];
        capture(&head, 0x72)
    }

    #[test]
    fn xctech_query_is_only_device_info() {
        let mut expected = [0u8; FRAME_LEN];
        expected[1] = 0x10;
        assert_eq!(battery_query(Family::Xctech), &expected[..]);
        assert_eq!(online_query(Family::Xctech), None);
    }

    #[test]
    fn xctech_captured_replies() {
        let level = |percent, asleep| {
            Decoded::Reading(Reading {
                percent,
                charging: false,
                asleep,
            })
        };
        assert_eq!(decode(&mouse_off()), level(55, true));
        assert_eq!(decode(&mouse_in_use()), level(85, false));
        assert_eq!(decode(&on_cable()), Decoded::Invalid(ON_CABLE_TEXT));
    }

    #[test]
    fn xctech_bad_checksum() {
        let mut frame = mouse_in_use();
        frame[CHECKSUM_AT] ^= 0x01;
        assert_eq!(decode(&frame), Decoded::Invalid("bad checksum"));
        let mut level = mouse_in_use();
        level[PERCENT_AT] = 0x45;
        assert_eq!(decode(&level), Decoded::Invalid("bad checksum"));
    }

    #[test]
    fn xctech_notifications_echo_and_errors_are_not_the_reply() {
        // Link-change and DPI-change notifications from the receiver.
        assert_eq!(
            decode(&padded::<REPORT_LEN>(&[0xC0, 0x01, 0x44, 0x00])),
            Decoded::NotTheReply
        );
        assert_eq!(
            decode(&padded::<REPORT_LEN>(&[0xC2, 0x03])),
            Decoded::NotTheReply
        );
        // The request as sent and as read back without its id, a failed status, short reads.
        assert_eq!(decode(&QUERY), Decoded::NotTheReply);
        assert_eq!(decode(&QUERY[1..]), Decoded::NotTheReply);
        let mut failed = mouse_in_use();
        failed[STATUS_AT] = 0x01;
        assert_eq!(decode(&failed), Decoded::NotTheReply);
        assert_eq!(decode(&mouse_in_use()[..CHECKSUM_AT]), Decoded::NotTheReply);
        assert_eq!(decode(&[]), Decoded::NotTheReply);
    }
}
