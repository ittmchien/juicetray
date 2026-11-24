//! RongYuan 2.4 GHz dongle status: SetFeature `00 F7 00…` (report id 0, 64 data bytes) on
//! FFFF:0002, then GetFeature on the same collection. The dongle answers from its own cache.

use crate::{Decoded, padded, reading};

/// Report id 0 plus the 64-byte feature report.
pub(crate) const FRAME_LEN: usize = 65;
/// The only command this family ever sends; every other first byte on this channel (F6, F8,
/// 7A, 7F, 11, setters) changes dongle or keyboard state.
const CMD_STATUS: u8 = 0xF7;
/// Bare form, as BatteryTray and BatteryHub send it on real receivers (no byte-7 checksum).
pub(crate) const QUERY: [u8; FRAME_LEN] = padded(&[0x00, CMD_STATUS]);
const _: () = assert!(QUERY[0] == 0 && QUERY[1] == CMD_STATUS);

/// Wire indexes (report id 0 at [0]).
const KEYBOARD_PERCENT_AT: usize = 2;
/// 0 = connected in the BatteryTray layout; another source reads this byte as charging. It is
/// treated as offline until a hardware capture settles it.
const KEYBOARD_OFFLINE_AT: usize = 4;

/// Decodes the keyboard slot of a GetFeature reply. An all-zero reply ("not filled yet") and a
/// zero level both decode as no data; charging is unknown (false).
pub(crate) fn decode(bytes: &[u8]) -> Decoded {
    if bytes.first() != Some(&0) {
        return Decoded::NotTheReply;
    }
    let (Some(&percent), Some(&offline)) = (
        bytes.get(KEYBOARD_PERCENT_AT),
        bytes.get(KEYBOARD_OFFLINE_AT),
    ) else {
        return Decoded::Invalid("short reply");
    };
    if offline != 0 {
        return Decoded::Invalid("keyboard offline");
    }
    reading(percent, false, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Family, Reading, battery_query, decode as decode_family, online_query};

    fn decode(bytes: &[u8]) -> Decoded {
        decode_family(Family::RongYuanF7, bytes)
    }

    /// A 65-byte GetFeature reply: id 0, then `[canRead, kb %, mouse %, kb offline, ...]`.
    fn reply(data: &[u8]) -> [u8; FRAME_LEN] {
        let mut bytes = [0u8; FRAME_LEN];
        bytes[1..=data.len()].copy_from_slice(data);
        bytes
    }

    #[test]
    fn rongyuan_query_is_only_f7() {
        let frame = battery_query(Family::RongYuanF7);
        assert_eq!(frame.len(), 65);
        assert_eq!(&frame[..2], &[0x00, 0xF7]);
        assert!(frame[2..].iter().all(|&byte| byte == 0));
        // No second frame exists for this channel.
        assert_eq!(online_query(Family::RongYuanF7), None);
    }

    #[test]
    fn rongyuan_keyboard_percent() {
        assert_eq!(
            decode(&reply(&[0x01, 0x57, 0x00, 0x00, 0x01, 0x01])),
            Decoded::Reading(Reading {
                percent: 87,
                charging: false,
                asleep: false,
            })
        );
    }

    #[test]
    fn rongyuan_offline_or_charging_byte_is_not_a_reading() {
        assert_eq!(
            decode(&reply(&[0x01, 0x57, 0x00, 0x01, 0x01])),
            Decoded::Invalid("keyboard offline")
        );
    }

    #[test]
    fn rongyuan_no_data() {
        assert_eq!(decode(&reply(&[])), Decoded::Invalid("percent 0: no data"));
        assert_eq!(
            decode(&reply(&[0x01, 0x00, 0x00, 0x00, 0x01])),
            Decoded::Invalid("percent 0: no data")
        );
        assert_eq!(
            decode(&reply(&[0x01, 0xC8])),
            Decoded::Invalid("percent out of range")
        );
    }

    #[test]
    fn rongyuan_other_ids_and_short_replies() {
        // A report 05 push from the other interface is not this reply.
        assert_eq!(
            decode(&[0x05, 0x88, 0x00, 0x00, 0x50, 0x01]),
            Decoded::NotTheReply
        );
        assert_eq!(decode(&[]), Decoded::NotTheReply);
        assert_eq!(decode(&[0x00, 0x01, 0x50]), Decoded::Invalid("short reply"));
    }
}
