//! `juicetray probe [--online] <VID:PID>`: one query to one known device through the door:
//! battery by default, the Compx online-status getter with `--online`. The query floor is
//! checked before any device is listed or opened, and the query time is saved before the write,
//! so a failed or unanswered query still counts.

use std::process::ExitCode;
use std::time::SystemTime;

use door::{Door, Exchange};
use protocol::{Decoded, KNOWN, KnownDevice, Link, Online, battery_query, known, online_query};

use crate::query::{
    ClaimError, USAGE, claim_query, claim_refusal, floor_check, open_hid_target,
    open_serial_target, parse_vid_pid, refusal, refusal_for, target_refusal,
};
use crate::query_log::{self, QueryLog};
use crate::scan::bytes_line;

/// Prints the exchange, or the reason nothing (more) was sent; exit 0 only for a valid reply.
pub fn run_probe(args: Vec<String>) -> ExitCode {
    match parse_probe_args(&args).and_then(|(arg, online)| probe(arg, online)) {
        Ok((text, is_reply)) => {
            println!("{text}");
            if is_reply {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(text) => {
            eprintln!("{text}");
            ExitCode::FAILURE
        }
    }
}

/// The optional `--online` flag (anywhere) and the single device argument.
fn parse_probe_args(args: &[String]) -> Result<(Option<&str>, bool), String> {
    let online = args.iter().any(|arg| arg == "--online");
    let mut rest = args.iter().filter(|arg| *arg != "--online");
    let device = rest.next().map(String::as_str);
    if rest.next().is_some() {
        return Err(USAGE.to_owned());
    }
    Ok((device, online))
}

/// Refusals before the log or any device is touched: `--online` is Compx only, and a device
/// with no known working battery command is refused for the battery query.
fn early_refusal(device: &KnownDevice, online: bool) -> Result<(), String> {
    let (vid, pid) = (device.vid, device.pid);
    if online && online_query(device.family).is_none() {
        return Err(refusal(vid, pid, "online query is Compx only"));
    }
    if device.needs_capture && !online {
        return Err(refusal(vid, pid, "not supported yet: needs a capture"));
    }
    Ok(())
}

/// The whole probe flow in its fixed safety order: parse, known device, capture / online
/// refusals, floor check (no device touched), list, pick, open, claim the query time, then the
/// one write. A device that needs a capture has no battery command yet, so only `--online`
/// (a different, read-only getter) may reach it.
fn probe(arg: Option<&str>, online: bool) -> Result<(String, bool), String> {
    let (vid, pid) = parse_vid_pid(arg.unwrap_or_default())?;
    let device = known(vid, pid).ok_or_else(|| unknown_device("probe", vid, pid))?;
    let family = device.family;
    early_refusal(device, online)?;

    let log_path = query_log::path()
        .ok_or_else(|| refusal(vid, pid, "no data directory to record the query time"))?;
    let log = QueryLog::load_from(&log_path);
    let refuse = |error: ClaimError| claim_refusal(vid, pid, family, error);
    if let Err(early) = floor_check(&log, family, vid, pid, SystemTime::now()) {
        // A future stamp is replaced by the claim, which refuses with the same clock-back text.
        let error = match early {
            ClaimError::ClockBack => claim_query(&log_path, family, vid, pid, SystemTime::now())
                .err()
                .unwrap_or(early),
            other => other,
        };
        return Err(refuse(error));
    }

    let inventory = door::inventory(&[(vid, pid)]);
    let opened = match family.link() {
        Link::Hid => open_hid_target(&inventory, family, vid, pid),
        Link::Serial => open_serial_target(&inventory, vid, pid),
    };
    let (mut target, target_line) =
        opened.map_err(|error| target_refusal(vid, pid, family, error))?;
    claim_query(&log_path, family, vid, pid, SystemTime::now()).map_err(refuse)?;

    let mut door = Door::new(1);
    let (sent, sent_result) = match online_query(family).filter(|_| online) {
        Some(frame) => (frame, door.send_online(&mut target)),
        None => (battery_query(family), door.send(&mut target)),
    };
    let exchange = sent_result.map_err(|error| send_failed(device, sent, &format!("{error:#}")))?;
    let mut text = exchange_text(device, &target_line, &exchange);
    // An online answer is not a battery reading: nothing is recorded for it.
    if matches!(exchange.reply, Some((_, Decoded::Online(_)))) {
        return Ok((text, true));
    }
    let Some((_, Decoded::Reading(reading))) = &exchange.reply else {
        return Ok((text, false));
    };
    // A failed record is a note only; the reading itself was valid.
    let read_at = SystemTime::now();
    let recorded = QueryLog::update(&log_path, |log| {
        log.record_reading(vid, pid, *reading, read_at);
        Ok::<_, anyhow::Error>(())
    });
    if let Err(error) = recorded {
        text.push_str(&format!(
            "\nnote: could not record the reading ({})",
            error.root_cause()
        ));
    }
    Ok((text, true))
}

/// The known-device table as a refusal for `command` (`probe` or `listen`).
pub(crate) fn unknown_device(command: &str, vid: u16, pid: u16) -> String {
    let rows: String = KNOWN
        .iter()
        .map(|device| {
            format!(
                "\n  {:04X}:{:04X}  {} · {}",
                device.vid,
                device.pid,
                device.name,
                device.family.name()
            )
        })
        .collect();
    refusal_for(
        command,
        vid,
        pid,
        &format!("is not a known device; known:{rows}"),
    )
}

/// A door error. Door refusals before the write say "nothing was sent"; any other error comes
/// from the write or the read, so the query `sent` went out (or may have) and the floor applies;
/// a refused HID write is usually a vendor app holding the device, hence the hint.
fn send_failed(device: &KnownDevice, sent: &[u8], error: &str) -> String {
    let head = format!("probe {:04X}:{:04X}: {error}", device.vid, device.pid);
    if error.contains("nothing was sent") {
        return head;
    }
    [
        head,
        bytes_line("sent", sent),
        "the query was (or may have been) sent; the next probe waits for the floor".to_owned(),
        "if a vendor app is open, close it and wait for the floor before the next probe".to_owned(),
    ]
    .join("\n")
}

/// Header, target, every byte sent and received, then the decoded result.
fn exchange_text(device: &KnownDevice, target_line: &str, exchange: &Exchange) -> String {
    let mut lines = vec![
        format!(
            "probe {:04X}:{:04X} · {} · {}",
            device.vid,
            device.pid,
            device.name,
            device.family.name()
        ),
        target_line.to_owned(),
        bytes_line("sent", exchange.sent),
    ];
    lines.extend(
        exchange
            .unrelated
            .iter()
            .map(|report| bytes_line("other", report)),
    );
    if exchange.dropped > 0 {
        lines.push(format!("+{} more not shown", exchange.dropped));
    }
    let result = match &exchange.reply {
        Some((bytes, decoded)) => {
            lines.push(bytes_line("reply", bytes));
            match decoded {
                Decoded::Online(_) => decoded_text(decoded),
                _ => format!("result: {}", decoded_text(decoded)),
            }
        }
        None => format!(
            "result: {}",
            match exchange.family.link() {
                Link::Hid => format!(
                    "no reply within {} ms (asleep? move it or put it on, then run probe once \
                     more after the floor)",
                    exchange.family.reply_timeout().as_millis()
                ),
                Link::Serial => format!(
                    "no reply within {} s",
                    exchange.family.reply_timeout().as_secs()
                ),
            }
        ),
    };
    lines.push(result);
    lines.join("\n")
}

fn decoded_text(decoded: &Decoded) -> String {
    match decoded {
        Decoded::Online(status) => format!(
            "online: {}",
            match status {
                Online::Yes => "yes",
                Online::No => "no",
                Online::Unsupported => "unsupported",
            }
        ),
        Decoded::Reading(reading) => format!(
            "{}% · {} · {}",
            reading.percent,
            if reading.charging {
                "charging"
            } else {
                "not charging"
            },
            if reading.asleep { "asleep" } else { "awake" }
        ),
        Decoded::Invalid(reason) => format!("invalid reply ({reason})"),
        // The door never keeps a non-reply as the reply.
        Decoded::NotTheReply => "invalid reply (not the reply)".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use protocol::{Family, Reading, decode};

    use super::*;
    use crate::query::USAGE;

    const MCHOSE: (u16, u16) = (0x3837, 0x6011);

    fn padded(head: &[u8], len: usize) -> Vec<u8> {
        let mut bytes = head.to_vec();
        bytes.resize(len, 0);
        bytes
    }

    /// The spec's MCHOSE capture (84 %), report id first.
    fn mchose_capture() -> Vec<u8> {
        padded(
            &[
                0x55, 0xaa, 0x01, 0x01, 0x03, 0x00, 0x01, 0x0b, 0x01, 0x54, 0x5d,
            ],
            64,
        )
    }

    fn mchose() -> &'static KnownDevice {
        known(MCHOSE.0, MCHOSE.1).unwrap()
    }

    fn exchange(family: Family, reply: Option<Vec<u8>>) -> Exchange {
        Exchange {
            family,
            sent: battery_query(family),
            reply: reply.map(|bytes| {
                let decoded = decode(family, &bytes);
                (bytes, decoded)
            }),
            unrelated: Vec::new(),
            dropped: 0,
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn probe_refuses_unknown() {
        let text = probe(Some("046D:C52B"), false).expect_err("unknown device must be refused");
        assert!(text.contains("046D:C52B"), "{text}");
        assert!(text.contains("not a known device"), "{text}");
        assert!(
            text.contains("3837:6011  MCHOSE V9 Pro 2 (dongle) · MCHOSE"),
            "{text}"
        );
        assert!(text.ends_with("nothing was sent"), "{text}");

        let usage = probe(None, false).expect_err("missing argument");
        assert_eq!(usage, USAGE);
    }

    #[test]
    fn probe_refuses_devices_that_need_a_capture() {
        for arg in ["3554:FA09", "391D:1A05"] {
            let text = probe(Some(arg), false).expect_err(arg);
            assert!(
                text.contains("not supported yet: needs a capture"),
                "{text}"
            );
            assert!(text.ends_with("nothing was sent"), "{text}");
        }
    }

    #[test]
    fn probe_text_reading() {
        let text = exchange_text(
            mchose(),
            "target HID if 3 FF07:0222 · path P",
            &exchange(Family::Mchose, Some(mchose_capture())),
        );
        let lines: Vec<&str> = text.lines().collect();

        assert_eq!(
            lines[0],
            "probe 3837:6011 · MCHOSE V9 Pro 2 (dongle) · MCHOSE"
        );
        assert_eq!(lines[1], "target HID if 3 FF07:0222 · path P");
        let query = battery_query(Family::Mchose);
        assert_eq!(lines[2], format!("sent (64 B): {}", hex(query)));
        assert!(lines[2].starts_with("sent (64 B): 55 aa 01 00 01 00 01 0b 00 00 00"));
        assert_eq!(
            lines[3],
            format!("reply (64 B): {}", hex(&mchose_capture()))
        );
        assert_eq!(lines[4], "result: 84% · not charging · awake");
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn probe_text_no_reply_hid() {
        let text = exchange_text(mchose(), "target", &exchange(Family::Mchose, None));
        let result = text.lines().last().unwrap();
        assert!(
            result.starts_with("result: no reply within 500 ms"),
            "{result}"
        );
        assert!(result.contains("asleep?"), "{result}");
        assert!(!text.contains("reply ("), "{text}");
    }

    #[test]
    fn probe_text_no_reply_serial() {
        let n9 = known(0x373B, 0x1204).unwrap();
        let text = exchange_text(n9, "target serial COM3", &exchange(Family::AtkSerial, None));
        assert!(text.contains("sent (64 B): "), "{text}");
        assert_eq!(text.lines().last(), Some("result: no reply within 1 s"));
    }

    #[test]
    fn probe_text_invalid() {
        let mut odd_flags = mchose_capture();
        odd_flags[3] = 0x07;
        let text = exchange_text(
            mchose(),
            "target",
            &exchange(Family::Mchose, Some(odd_flags)),
        );
        assert!(text.contains("reply (64 B): 55 aa 01 07"), "{text}");
        assert_eq!(
            text.lines().last(),
            Some("result: invalid reply (unknown flags)")
        );
        assert_eq!(
            decoded_text(&Decoded::Reading(Reading {
                percent: 50,
                charging: true,
                asleep: true,
            })),
            "50% · charging · asleep"
        );
    }

    #[test]
    fn probe_text_other_reports() {
        let first = padded(&[0x55, 0xaa, 0x02], 64);
        let second = padded(&[0x55, 0xaa, 0x03], 64);
        let exchange = Exchange {
            unrelated: vec![first.clone(), second.clone()],
            dropped: 4,
            ..exchange(Family::Mchose, Some(mchose_capture()))
        };
        let text = exchange_text(mchose(), "target", &exchange);
        let lines: Vec<&str> = text.lines().collect();

        assert_eq!(lines[3], format!("other (64 B): {}", hex(&first)));
        assert_eq!(lines[4], format!("other (64 B): {}", hex(&second)));
        assert_eq!(lines[5], "+4 more not shown");
        assert!(lines[6].starts_with("reply (64 B): "));
    }

    #[test]
    fn send_failed_shows_sent_bytes_after_the_write() {
        let text = send_failed(
            mchose(),
            battery_query(Family::Mchose),
            "write failed: boom",
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "probe 3837:6011: write failed: boom");
        assert_eq!(
            lines[1],
            format!("sent (64 B): {}", hex(battery_query(Family::Mchose)))
        );
        assert!(lines[2].contains("may have been) sent"), "{text}");
        assert!(lines[2].contains("floor"), "{text}");
        assert!(lines[3].contains("vendor app is open"), "{text}");

        let refused = send_failed(
            mchose(),
            battery_query(Family::Mchose),
            "write budget used up; nothing was sent",
        );
        assert_eq!(
            refused,
            "probe 3837:6011: write budget used up; nothing was sent"
        );
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn probe_args_online_flag() {
        let parse = |list: &[&str]| {
            parse_probe_args(&args(list)).map(|(arg, online)| (arg.map(str::to_owned), online))
        };
        assert_eq!(parse(&[]), Ok((None, false)));
        assert_eq!(parse(&["3837:6011"]), Ok((Some("3837:6011".into()), false)));
        assert_eq!(
            parse(&["--online", "391D:1A05"]),
            Ok((Some("391D:1A05".into()), true))
        );
        assert_eq!(
            parse(&["391D:1A05", "--online"]),
            Ok((Some("391D:1A05".into()), true))
        );
        assert_eq!(parse(&["--online"]), Ok((None, true)));
        assert_eq!(parse(&["a", "b"]), Err(USAGE.to_owned()));
        // An unknown flag is taken as the device argument and fails the VID:PID parse.
        let flag = parse(&["--oneline"]).unwrap().0;
        assert_eq!(
            parse_vid_pid(flag.as_deref().unwrap()),
            Err(USAGE.to_owned())
        );
        assert!(USAGE.contains("--online"));
    }

    #[test]
    fn early_refusal_online_rules() {
        let vgn = known(0x3554, 0xFA09).unwrap();
        let cable = known(0x391D, 0x1A05).unwrap();
        let x1 = known(0x373B, 0x1107).unwrap();

        // needs_capture blocks the battery query but not the online one (Compx only).
        assert!(
            early_refusal(cable, false)
                .unwrap_err()
                .contains("needs a capture")
        );
        assert_eq!(early_refusal(cable, true), Ok(()));
        assert_eq!(early_refusal(x1, true), Ok(()));
        assert_eq!(early_refusal(x1, false), Ok(()));

        for online in [false, true] {
            let text = early_refusal(vgn, online).unwrap_err();
            assert!(text.ends_with("nothing was sent"), "{text}");
            let expected = if online {
                "online query is Compx only"
            } else {
                "needs a capture"
            };
            assert!(text.contains(expected), "{text}");
        }
        let mchose = early_refusal(mchose(), true).unwrap_err();
        assert!(mchose.contains("online query is Compx only"), "{mchose}");
    }

    #[test]
    fn probe_text_online() {
        let cable = known(0x391D, 0x1A05).unwrap();
        let frame = online_query(Family::Compx).unwrap();
        let reply = |status: u8, value: u8| {
            let mut bytes = padded(&[0x08, 0x03, status, 0, 0, 0, value], 17);
            bytes[16] = protocol::compx::checksum(&bytes[..16]);
            bytes
        };
        for (bytes, last) in [
            (reply(0, 1), "online: yes"),
            (reply(0, 0), "online: no"),
            (reply(1, 0), "online: unsupported"),
        ] {
            let decoded = protocol::decode_online(Family::Compx, &bytes);
            let exchange = Exchange {
                family: Family::Compx,
                sent: frame,
                reply: Some((bytes.clone(), decoded)),
                unrelated: Vec::new(),
                dropped: 0,
            };
            let text = exchange_text(cable, "target", &exchange);
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines[2], format!("sent (17 B): {}", hex(frame)));
            assert_eq!(lines[3], format!("reply (17 B): {}", hex(&bytes)));
            assert_eq!(lines[4], last);
            assert_eq!(lines.len(), 5);
        }
    }

    #[test]
    fn send_failed_shows_the_frame_that_went_out() {
        let cable = known(0x391D, 0x1A05).unwrap();
        let frame = online_query(Family::Compx).unwrap();
        let text = send_failed(cable, frame, "write failed: boom");
        assert_eq!(
            text.lines().nth(1),
            Some(format!("sent (17 B): {}", hex(frame)).as_str())
        );
    }
}
