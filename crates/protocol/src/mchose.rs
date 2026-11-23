//! MCHOSE V9 Pro 2: 64-byte HID report, id `0x55`, then a frame starting with `0xAA`.

use crate::{Decoded, padded, reading};

const REPORT_ID: u8 = 0x55;
const REPORT_LEN: usize = 64;
const FRAME_START: u8 = 0xaa;
// Frame bytes after the report id: start, version, flags, length, cmdType (2), cmd.
const CMD_TYPE_REQUEST: [u8; 2] = [0x00, 0x01];
const CMD_GET_BATTERY_SLEEP: u8 = 0x0b;
const FRAME_HEADER_LEN: usize = 7;

pub(crate) const QUERY: [u8; REPORT_LEN] = padded(&[
    REPORT_ID,
    FRAME_START,
    0x01,
    0x00, // flags: no checksum
    0x01, // length: the checksum byte only
    CMD_TYPE_REQUEST[0],
    CMD_TYPE_REQUEST[1],
    CMD_GET_BATTERY_SLEEP,
]);

pub(crate) fn decode(bytes: &[u8]) -> Decoded {
    let Some((&REPORT_ID, frame)) = bytes.split_first() else {
        return Decoded::NotTheReply;
    };
    let Some(header) = frame.get(..FRAME_HEADER_LEN) else {
        return Decoded::NotTheReply;
    };
    // Replies start with 0xAA; flags-0 replies carry no checksum to catch anything else.
    if header[0] != FRAME_START {
        return Decoded::NotTheReply;
    }
    let cmd_match = [
        CMD_TYPE_REQUEST[0],
        CMD_TYPE_REQUEST[1],
        CMD_GET_BATTERY_SLEEP,
    ];
    if header[4..] != cmd_match {
        return Decoded::NotTheReply;
    }
    let (flags, len) = (header[2], usize::from(header[3]));

    // `len` counts the payload plus the trailing checksum byte.
    if len < 3 {
        return Decoded::Invalid("short payload");
    }
    let Some(body) = frame.get(FRAME_HEADER_LEN..FRAME_HEADER_LEN + len) else {
        return Decoded::Invalid("truncated");
    };
    let Some((&checksum, payload)) = body.split_last() else {
        return Decoded::Invalid("short payload");
    };
    let &[sleep_state, level, ..] = payload else {
        return Decoded::Invalid("short payload");
    };

    match flags {
        0 => {}
        1 => {
            let xor = header[2..].iter().chain(payload).fold(0, |acc, b| acc ^ b);
            if xor != checksum {
                return Decoded::Invalid("bad checksum");
            }
        }
        _ => return Decoded::Invalid("unknown flags"),
    }
    reading(level & 0x7f, level & 0x80 != 0, sleep_state == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Family, Reading, battery_query, decode as decode_family};

    /// Zero-pads `head` to a full 64-byte report, as the device returns it.
    fn report(head: &[u8]) -> [u8; 64] {
        padded(head)
    }

    fn decode(bytes: &[u8]) -> Decoded {
        decode_family(Family::Mchose, bytes)
    }

    fn reading_of(percent: u8, charging: bool, asleep: bool) -> Decoded {
        Decoded::Reading(Reading {
            percent,
            charging,
            asleep,
        })
    }

    #[test]
    fn mchose_query_bytes() {
        let mut expected = [0u8; 64];
        expected[..9].copy_from_slice(&[0x55, 0xaa, 0x01, 0x00, 0x01, 0x00, 0x01, 0x0b, 0x00]);
        assert_eq!(battery_query(Family::Mchose), &expected[..]);
        assert_eq!(battery_query(Family::Mchose).len(), 64);
    }

    #[test]
    fn mchose_real_capture() {
        let capture = report(&[
            0x55, 0xaa, 0x01, 0x01, 0x03, 0x00, 0x01, 0x0b, 0x01, 0x54, 0x5d,
        ]);
        assert_eq!(decode(&capture), reading_of(84, false, false));
    }

    #[test]
    fn mchose_bad_checksum() {
        let capture = report(&[
            0x55, 0xaa, 0x01, 0x01, 0x03, 0x00, 0x01, 0x0b, 0x01, 0x54, 0x5c,
        ]);
        assert_eq!(decode(&capture), Decoded::Invalid("bad checksum"));
    }

    #[test]
    fn mchose_asleep() {
        let bytes = [
            0x55, 0xaa, 0x01, 0x00, 0x03, 0x00, 0x01, 0x0b, 0x00, 0x54, 0x00,
        ];
        assert_eq!(decode(&bytes), reading_of(84, false, true));
    }

    #[test]
    fn mchose_charging() {
        let bytes = [
            0x55, 0xaa, 0x01, 0x00, 0x03, 0x00, 0x01, 0x0b, 0x01, 0xd4, 0x00,
        ];
        assert_eq!(decode(&bytes), reading_of(84, true, false));
    }

    #[test]
    fn mchose_notification_ignored() {
        let bytes = [
            0x55, 0xaa, 0x01, 0x00, 0x03, 0x00, 0x04, 0x22, 0x01, 0x54, 0x00,
        ];
        assert_eq!(decode(&bytes), Decoded::NotTheReply);
    }

    #[test]
    fn mchose_truncated() {
        let bytes = [0x55, 0xaa, 0x01, 0x00, 0x03, 0x00, 0x01, 0x0b, 0x01];
        assert_eq!(decode(&bytes), Decoded::Invalid("truncated"));
    }

    #[test]
    fn mchose_short_payload() {
        let bytes = [0x55, 0xaa, 0x01, 0x00, 0x02, 0x00, 0x01, 0x0b, 0x01, 0x54];
        assert_eq!(decode(&bytes), Decoded::Invalid("short payload"));
    }

    #[test]
    fn mchose_level_zero_is_no_data() {
        let awake = [
            0x55, 0xaa, 0x01, 0x00, 0x03, 0x00, 0x01, 0x0b, 0x01, 0x00, 0x00,
        ];
        assert_eq!(decode(&awake), Decoded::Invalid("percent 0: no data"));
        let asleep = [
            0x55, 0xaa, 0x01, 0x00, 0x03, 0x00, 0x01, 0x0b, 0x00, 0x00, 0x00,
        ];
        assert_eq!(decode(&asleep), Decoded::Invalid("percent 0: no data"));
    }

    #[test]
    fn mchose_wrong_start_byte_is_not_the_reply() {
        let bytes = [
            0x55, 0xab, 0x01, 0x00, 0x03, 0x00, 0x01, 0x0b, 0x01, 0x54, 0x00,
        ];
        assert_eq!(decode(&bytes), Decoded::NotTheReply);
    }

    #[test]
    fn mchose_unknown_flags() {
        let bytes = [
            0x55, 0xaa, 0x01, 0x02, 0x03, 0x00, 0x01, 0x0b, 0x01, 0x54, 0x00,
        ];
        assert_eq!(decode(&bytes), Decoded::Invalid("unknown flags"));
    }
}
