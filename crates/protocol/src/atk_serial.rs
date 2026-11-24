//! ATK N9 serial protocol: 64-byte frames, request `[0x02][cmd][sub]`, reply `[len][cmd][sub]…`.

use crate::{Decoded, Online, SERIAL_FRAME_LEN, padded, reading};

const REQUEST_START: u8 = 0x02;
const CMD_BATTERY: u8 = 0x00;
const SUB_BATTERY: u8 = 0x03;
/// ATK HUB's getStatus: the reply says whether the headset is linked to the dongle.
const SUB_STATUS: u8 = 0x04;

pub(crate) const QUERY: [u8; SERIAL_FRAME_LEN] = padded(&[REQUEST_START, CMD_BATTERY, SUB_BATTERY]);
pub(crate) const ONLINE_QUERY: [u8; SERIAL_FRAME_LEN] =
    padded(&[REQUEST_START, CMD_BATTERY, SUB_STATUS]);

/// The getStatus reply: byte `[4]` 1 = headset linked to the dongle, 0 = off (checked both
/// ways on hardware); any other cmd/sub answers some other command.
pub(crate) fn decode_online(bytes: &[u8]) -> Decoded {
    let &[_, CMD_BATTERY, SUB_STATUS, _, linked, ..] = bytes else {
        return Decoded::NotTheReply;
    };
    Decoded::Online(if linked == 1 { Online::Yes } else { Online::No })
}

pub(crate) fn decode(bytes: &[u8]) -> Decoded {
    // The leading length byte is not interpreted.
    let &[_, CMD_BATTERY, SUB_BATTERY, _, level, ..] = bytes else {
        return Decoded::NotTheReply;
    };
    reading(level & 0x7f, level & 0x80 != 0, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Family, Reading, battery_query, decode as decode_family,
        decode_online as decode_online_family, online_query,
    };

    // Synthetic frame: no real capture exists for this family.
    fn frame(cmd_sub: [u8; 2], level: u8) -> [u8; 64] {
        padded(&[0x05, cmd_sub[0], cmd_sub[1], 0x00, level])
    }

    fn decode(bytes: &[u8]) -> Decoded {
        decode_family(Family::AtkSerial, bytes)
    }

    #[test]
    fn atk_serial_query_bytes() {
        let mut expected = [0u8; 64];
        expected[..3].copy_from_slice(&[0x02, 0x00, 0x03]);
        assert_eq!(battery_query(Family::AtkSerial), &expected[..]);
        assert_eq!(battery_query(Family::AtkSerial).len(), 64);
    }

    #[test]
    fn atk_serial_reply() {
        assert_eq!(
            decode(&frame([0x00, 0x03], 0xd2)),
            Decoded::Reading(Reading {
                percent: 82,
                charging: true,
                asleep: false
            })
        );
        assert_eq!(
            decode(&frame([0x00, 0x03], 0x52)),
            Decoded::Reading(Reading {
                percent: 82,
                charging: false,
                asleep: false
            })
        );
    }

    #[test]
    fn atk_serial_other_cmd() {
        assert_eq!(decode(&frame([0x00, 0x04], 0x52)), Decoded::NotTheReply);
    }

    #[test]
    fn atk_echoed_request_is_not_a_reading() {
        let echoed = decode(battery_query(Family::AtkSerial));
        assert!(!matches!(echoed, Decoded::Reading(_)));
        assert_eq!(echoed, Decoded::Invalid("percent 0: no data"));
    }

    #[test]
    fn atk_serial_percent_101() {
        assert_eq!(
            decode(&frame([0x00, 0x03], 101)),
            Decoded::Invalid("percent out of range")
        );
    }

    #[test]
    fn atk_serial_short_is_not_the_reply() {
        assert_eq!(decode(&[0x05, 0x00, 0x03, 0x00]), Decoded::NotTheReply);
    }

    fn online(bytes: &[u8]) -> Decoded {
        decode_online_family(Family::AtkSerial, bytes)
    }

    #[test]
    fn atk_serial_online_query_bytes() {
        let mut expected = [0u8; 64];
        expected[..3].copy_from_slice(&[0x02, 0x00, 0x04]);
        assert_eq!(online_query(Family::AtkSerial), Some(&expected[..]));
    }

    #[test]
    fn atk_serial_online_reply() {
        // Synthetic frames: no real capture exists for this family.
        assert_eq!(
            online(&frame([0x00, 0x04], 1)),
            Decoded::Online(Online::Yes)
        );
        assert_eq!(online(&frame([0x00, 0x04], 0)), Decoded::Online(Online::No));
        assert_eq!(online(&frame([0x00, 0x04], 2)), Decoded::Online(Online::No));
    }

    #[test]
    fn atk_serial_online_other_cmd_or_short() {
        // A battery reply, another command and a short frame answer some other command.
        assert_eq!(online(&frame([0x00, 0x03], 1)), Decoded::NotTheReply);
        assert_eq!(online(&frame([0x01, 0x04], 1)), Decoded::NotTheReply);
        assert_eq!(online(&[0x05, 0x00, 0x04, 0x00]), Decoded::NotTheReply);
        // The battery decoder ignores a status reply.
        assert_eq!(decode(&frame([0x00, 0x04], 1)), Decoded::NotTheReply);
    }
}
