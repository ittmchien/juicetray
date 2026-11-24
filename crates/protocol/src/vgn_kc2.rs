//! VGN `vgn-kc2` (Dragonfly 3 Turbo+ dongle): output report `0xB3` cmd 0x06 GetDeviceInfo on
//! FFC1:0001, answered by input report `0xB4` echoing the command. No checksum.

use crate::{Decoded, padded, reading};

pub(crate) const OUTPUT_ID: u8 = 0xB3;
pub(crate) const INPUT_ID: u8 = 0xB4;
/// Both reports are 64 bytes with the id.
pub(crate) const REPORT_LEN: usize = 64;
const CMD_DEVICE_INFO: u8 = 0x06;
/// Raw index of the battery byte (data index 19 plus the report id).
const BATTERY_AT: usize = 20;

pub(crate) const QUERY: [u8; REPORT_LEN] = padded(&[OUTPUT_ID, CMD_DEVICE_INFO]);

/// Only `B4 06 …` is the reply; 0xB6, 0xE2 pushes and other commands are ignored. Bit 7 of the
/// battery byte is masked off as VGN HUB does; it may be charging but nothing confirms it.
pub(crate) fn decode(bytes: &[u8]) -> Decoded {
    if bytes.get(..2) != Some(&[INPUT_ID, CMD_DEVICE_INFO][..]) {
        return Decoded::NotTheReply;
    }
    let Some(&battery) = bytes.get(BATTERY_AT) else {
        return Decoded::Invalid("short reply");
    };
    reading(battery & 0x7F, false, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Family, Reading, battery_query, decode as decode_family};

    fn decode(bytes: &[u8]) -> Decoded {
        decode_family(Family::VgnKc2, bytes)
    }

    /// A 64-byte `B4 06` reply with `battery` at raw[20].
    fn reply(battery: u8) -> [u8; REPORT_LEN] {
        let mut bytes: [u8; REPORT_LEN] = padded(&[INPUT_ID, CMD_DEVICE_INFO, 0x01, 0x00, 0x30]);
        bytes[BATTERY_AT] = battery;
        bytes
    }

    #[test]
    fn vgn_kc2_query_bytes() {
        let mut expected = [0u8; 64];
        expected[..2].copy_from_slice(&[0xB3, 0x06]);
        assert_eq!(battery_query(Family::VgnKc2), &expected[..]);
    }

    #[test]
    fn vgn_kc2_reply_percent() {
        let level = |percent| {
            Decoded::Reading(Reading {
                percent,
                charging: false,
                asleep: false,
            })
        };
        assert_eq!(decode(&reply(0x4B)), level(75));
        assert_eq!(decode(&reply(100)), level(100));
        // Bit 7 is masked; charging stays unknown (false).
        assert_eq!(decode(&reply(0x80 | 42)), level(42));
    }

    #[test]
    fn vgn_kc2_zero_and_out_of_range() {
        assert_eq!(decode(&reply(0)), Decoded::Invalid("percent 0: no data"));
        assert_eq!(decode(&reply(0x80)), Decoded::Invalid("percent 0: no data"));
        assert_eq!(
            decode(&reply(101)),
            Decoded::Invalid("percent out of range")
        );
    }

    #[test]
    fn vgn_kc2_other_reports_are_not_the_reply() {
        // An E2 push (battery at raw[5]), a B6 awake reply and another B4 command.
        assert_eq!(
            decode(&padded::<21>(&[0xB6, 0xE2, 0x01, 0x00, 0x55])),
            Decoded::NotTheReply
        );
        assert_eq!(
            decode(&padded::<21>(&[0xB6, 0x03, 0, 0, 0, 0, 1])),
            Decoded::NotTheReply
        );
        assert_eq!(
            decode(&padded::<64>(&[0xB4, 0x04, 0x01])),
            Decoded::NotTheReply
        );
        // The request itself (an echo) is not the reply either.
        assert_eq!(decode(&QUERY), Decoded::NotTheReply);
        assert_eq!(decode(&[]), Decoded::NotTheReply);
    }

    #[test]
    fn vgn_kc2_short_reply() {
        assert_eq!(decode(&reply(50)[..20]), Decoded::Invalid("short reply"));
    }
}
