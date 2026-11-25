//! ATK "BITMOUSE" (X1 SE dongle 373B:1216, X1 SE NK mouse 373B:1299): 64-byte HID output report
//! id `0x08`, command `0x07`. Layout from the ATK HUB app; the cable (target 0) reply is captured.

use crate::{Decoded, DeviceType, padded, reading};

pub(crate) const REPORT_ID: u8 = 0x08;
pub(crate) const REPORT_LEN: usize = 64;
/// Smallest input report that still holds the percent byte.
pub(crate) const MIN_REPLY_LEN: usize = 8;
const HEADER: u8 = 0x72;
const PUSH_HEADER: u8 = 0x73;
const STATUS_ERROR: u8 = 0xFF;
const PARAM_LEN: u8 = 0x02;
const SN: u8 = 0x00;
/// HUB `usbMouse`: the mouse itself on its USB cable.
const TARGET_MOUSE_ON_CABLE: u8 = 0x00;
const TARGET_MOUSE_VIA_DONGLE: u8 = 0x01;
const CMD_BATTERY: u8 = 0x07;
/// HUB GetDeviceType: read-only; receivers answer it themselves, cable mice too.
const CMD_DEVICE_TYPE: u8 = 0x1A;
const CMD_LEN: u8 = 0x01;
/// HUB device types by code (enum `ki`): name, and whether it is the mouse on its cable.
const DEVICE_TYPES: [(&str, bool); 8] = [
    ("dongle1K", false),
    ("dongle4K", false),
    ("wired1K", true),
    ("wired8K", true),
    ("dongle2K", false),
    ("dongle8K", false),
    ("wired2K", true),
    ("wired4K", true),
];

/// The battery query sent to a receiver; the receiver forwards it to its mouse.
pub(crate) const QUERY: [u8; REPORT_LEN] = frame(TARGET_MOUSE_VIA_DONGLE, CMD_BATTERY);
/// The battery query sent to a mouse on its cable.
pub(crate) const QUERY_CABLE: [u8; REPORT_LEN] = frame(TARGET_MOUSE_ON_CABLE, CMD_BATTERY);
/// The type query; HUB sends it with target 0 to receivers and cable mice alike.
pub(crate) const TYPE_QUERY: [u8; REPORT_LEN] = frame(TARGET_MOUSE_ON_CABLE, CMD_DEVICE_TYPE);

// Wire frame: report id, checksum, then the body the checksum covers.
const fn frame(target: u8, command: u8) -> [u8; REPORT_LEN] {
    let mut frame = padded(&[
        REPORT_ID, 0, HEADER, PARAM_LEN, SN, target, command, CMD_LEN,
    ]);
    frame[1] = checksum(frame.split_at(2).1);
    frame
}

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

/// Header check shared by every reply: `Err` says why `bytes` is not the answer to `command`.
fn check_reply(bytes: &[u8], command: u8) -> Result<(), Decoded> {
    if bytes.len() < MIN_REPLY_LEN || bytes[0] != REPORT_ID {
        return Err(Decoded::NotTheReply);
    }
    // An error frame may lack the command echo, so the status is read before the command.
    if bytes[2] == STATUS_ERROR {
        return Err(Decoded::Invalid("device returned an error"));
    }
    // Our own request echoed back has its target (0x00 or 0x01) here, so it never matches.
    if bytes[2] == PUSH_HEADER || bytes[5] != command {
        return Err(Decoded::NotTheReply);
    }
    Ok(())
}

/// An error frame carrying the type command: the type query's late error, not a battery answer.
pub(crate) fn is_type_error(bytes: &[u8]) -> bool {
    bytes.len() >= MIN_REPLY_LEN
        && bytes[0] == REPORT_ID
        && bytes[2] == STATUS_ERROR
        && bytes[5] == CMD_DEVICE_TYPE
}

pub(crate) fn decode(bytes: &[u8]) -> Decoded {
    if let Err(not_a_reading) = check_reply(bytes, CMD_BATTERY) {
        return not_a_reading;
    }
    // Charging needs a second command that is not sent.
    reading(bytes[7], false, false)
}

/// The answer to `TYPE_QUERY`; `None` for any other report, an error or an unknown type code.
pub(crate) fn decode_type(bytes: &[u8]) -> Option<DeviceType> {
    check_reply(bytes, CMD_DEVICE_TYPE).ok()?;
    let &(name, cable) = DEVICE_TYPES.get(usize::from(bytes[7]))?;
    Some(DeviceType { name, cable })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Family, Reading, battery_query, battery_query_for, decode as decode_family,
        decode_type as decode_type_family, type_query,
    };

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
        assert_eq!(battery_query_for(Family::AtkBitmouse, false), &expected[..]);
    }

    /// The cable frame differs only in the target byte and its checksum (sent on real hardware).
    #[test]
    fn bitmouse_cable_query_bytes() {
        let mut expected = [0u8; 64];
        expected[..8].copy_from_slice(&[0x08, 0x7C, 0x72, 0x02, 0x00, 0x00, 0x07, 0x01]);
        assert_eq!(battery_query_for(Family::AtkBitmouse, true), &expected[..]);
        assert_eq!(checksum(&QUERY_CABLE[2..]), QUERY_CABLE[1]);
        assert_eq!(decode(&QUERY_CABLE), Decoded::NotTheReply);
    }

    #[test]
    fn bitmouse_type_query_bytes() {
        let mut expected = [0u8; 64];
        expected[..8].copy_from_slice(&[0x08, 0x8F, 0x72, 0x02, 0x00, 0x00, 0x1A, 0x01]);
        assert_eq!(type_query(Family::AtkBitmouse), Some(&expected[..]));
        assert_eq!(type_query(Family::Compx), None);
        // The echoed request is neither a type answer nor a battery reply.
        assert_eq!(decode_type(&TYPE_QUERY), None);
        assert_eq!(decode(&TYPE_QUERY), Decoded::NotTheReply);
    }

    /// Real captures: 373B:1216 (X1 SE receiver) and 373B:1299 (X1 SE NK mouse on its cable).
    #[test]
    fn bitmouse_type_reply_captures_decode() {
        let reply = |code: u8| {
            let mut frame = vec![0x08, 0x72, 0x00, 0x3B, 0x00, 0x1A, 0x01, code, 0x00];
            frame.resize(127, 0);
            frame
        };
        let receiver = DeviceType {
            name: "dongle1K",
            cable: false,
        };
        let cable = DeviceType {
            name: "wired1K",
            cable: true,
        };
        assert_eq!(
            decode_type_family(Family::AtkBitmouse, &reply(0)),
            Some(receiver)
        );
        assert_eq!(
            decode_type_family(Family::AtkBitmouse, &reply(2)),
            Some(cable)
        );
        assert_eq!(decode_type_family(Family::Compx, &reply(2)), None);
        // Wired codes are 2, 3, 6, 7; an unknown code or an error is no answer.
        let is_wired = |code: u8| decode_type(&reply(code)).unwrap().cable;
        let wired: Vec<u8> = (0..8).filter(|&code| is_wired(code)).collect();
        assert_eq!(wired, [2, 3, 6, 7]);
        assert_eq!(decode_type(&reply(8)), None);
        let mut error = reply(2);
        error[2] = 0xFF;
        assert_eq!(decode_type(&error), None);
        // A type answer is not a battery reply.
        assert_eq!(decode(&reply(2)), Decoded::NotTheReply);
        // Only an error frame carrying the type command is a type error.
        assert!(is_type_error(&error));
        assert!(!is_type_error(&reply(2)));
        error[5] = CMD_BATTERY;
        assert!(!is_type_error(&error));
    }

    /// Real capture: X1 SE NK mouse 373B:1299 on its cable, reply to the target-0 query.
    #[test]
    fn bitmouse_cable_reply_capture_decodes() {
        let mut frame = vec![0x08, 0x72, 0x00, 0x3B, 0x00, 0x07, 0x01, 0x3C, 0x0C, 0x0F];
        frame.resize(127, 0);
        assert_eq!(
            decode(&frame),
            Decoded::Reading(Reading {
                percent: 60,
                charging: false,
                asleep: false
            })
        );
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
    fn bitmouse_error_status_without_command_echo() {
        let mut frame = reply();
        frame[2] = 0xFF;
        frame[5] = 0x00;
        assert_eq!(decode(&frame), Decoded::Invalid("device returned an error"));
        // Another report id is still not the reply.
        frame[0] = 0x09;
        assert_eq!(decode(&frame), Decoded::NotTheReply);
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
