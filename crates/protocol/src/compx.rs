//! Compx "report 8" (VGN, ATK X1 SE): 17-byte HID report, id `0x08`, command `0x04`.

use crate::{Decoded, Online, padded, reading};

const REPORT_ID: u8 = 0x08;
const CMD_BATTERY: u8 = 0x04;
/// Read-only getter in both vendor apps (spec: "optionally 0x03 = online status").
const CMD_ONLINE: u8 = 0x03;
const REPORT_LEN: usize = 17;

/// A query frame for `command`; the last byte is the checksum of the 16 before it.
const fn query_frame(command: u8) -> [u8; REPORT_LEN] {
    let mut frame = padded(&[REPORT_ID, command]);
    let (body, _) = frame.split_at(REPORT_LEN - 1);
    frame[REPORT_LEN - 1] = checksum(body);
    frame
}

pub(crate) const QUERY: [u8; REPORT_LEN] = query_frame(CMD_BATTERY);
pub(crate) const ONLINE_QUERY: [u8; REPORT_LEN] = query_frame(CMD_ONLINE);

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

/// The online-status reply (id first): status byte `[2]` 1 = unsupported, else payload byte 5
/// (`[6]`) 1 = online. The checksum is verified before either is trusted.
pub(crate) fn decode_online(bytes: &[u8]) -> Decoded {
    let Some(report) = bytes.first_chunk::<REPORT_LEN>() else {
        return Decoded::NotTheReply;
    };
    if report[0] != REPORT_ID || report[1] != CMD_ONLINE {
        return Decoded::NotTheReply;
    }
    let (body, tail) = report.split_at(REPORT_LEN - 1);
    if checksum(body) != tail[0] {
        return Decoded::Invalid("bad checksum");
    }
    Decoded::Online(match (report[2], report[6]) {
        (1, _) => Online::Unsupported,
        (_, 1) => Online::Yes,
        _ => Online::No,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Family, Reading, battery_query, decode as decode_family,
        decode_online as decode_online_family, online_query,
    };

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

    /// An online-status reply with a valid checksum for `status` and `value`.
    fn online_reply(status: u8, value: u8) -> [u8; 17] {
        let mut reply = [0u8; 17];
        reply[0] = 0x08;
        reply[1] = 0x03;
        reply[2] = status;
        reply[6] = value;
        reply[16] = checksum(&reply[..16]);
        reply
    }

    #[test]
    fn compx_online_query_bytes() {
        let mut expected = [0u8; 17];
        expected[0] = 0x08;
        expected[1] = 0x03;
        expected[16] = 0x4A;
        assert_eq!(online_query(Family::Compx), Some(&expected[..]));
        let q = online_query(Family::Compx).unwrap();
        assert_eq!(checksum(&q[..16]), q[16]);
    }

    #[test]
    fn online_query_is_compx_only() {
        for family in [
            Family::Report13,
            Family::Mchose,
            Family::AtkSerial,
            Family::AtkBitmouse,
        ] {
            assert_eq!(online_query(family), None, "{family:?}");
            assert_eq!(
                decode_online_family(family, &online_reply(0, 1)),
                Decoded::NotTheReply,
                "{family:?}"
            );
        }
    }

    #[test]
    fn compx_online_reply_decodes() {
        let decode = |bytes: &[u8]| decode_online_family(Family::Compx, bytes);
        assert_eq!(decode(&online_reply(0, 1)), Decoded::Online(Online::Yes));
        assert_eq!(decode(&online_reply(0, 0)), Decoded::Online(Online::No));
        assert_eq!(decode(&online_reply(0, 2)), Decoded::Online(Online::No));
        assert_eq!(
            decode(&online_reply(1, 1)),
            Decoded::Online(Online::Unsupported)
        );

        let mut bad = online_reply(0, 1);
        bad[16] ^= 0xFF;
        assert_eq!(decode(&bad), Decoded::Invalid("bad checksum"));
        // A battery reply or a short report answers some other command.
        assert_eq!(decode(&REPLY), Decoded::NotTheReply);
        assert_eq!(decode(&online_reply(0, 1)[..16]), Decoded::NotTheReply);
        let mut other_report = online_reply(0, 1);
        other_report[0] = 0x09;
        assert_eq!(decode(&other_report), Decoded::NotTheReply);
    }
}
