//! `juicetray probe <VID:PID>`: one battery query to one known device through the door. The
//! query floor is checked before any device is listed or opened, and the query time is saved
//! before the write, so a failed or unanswered query still counts.

use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use door::{
    Access, Door, Exchange, HID_REPLY_TIMEOUT, HidCollection, Inventory, SERIAL_REPLY_TIMEOUT,
    SerialEntry, Target,
};
use protocol::{
    Decoded, Family, HidShape, KNOWN, KnownDevice, Link, PickError, battery_query, floor_wait,
    known, pick_target,
};

use crate::query_log::{self, QueryLog};
use crate::scan::{bytes_line, mask_serial, redact_path};

const USAGE: &str = "usage: juicetray probe <VID:PID>, e.g. juicetray probe 3837:6011";

/// Prints the exchange, or the reason nothing (more) was sent; exit 0 only for a valid reading.
pub fn run_probe(arg: Option<String>) -> ExitCode {
    match probe(arg.as_deref()) {
        Ok((text, is_reading)) => {
            println!("{text}");
            if is_reading {
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

/// The whole probe flow in its fixed safety order: parse, known device, floor check (no device
/// touched), list, pick, open, claim the query time, then the one write.
fn probe(arg: Option<&str>) -> Result<(String, bool), String> {
    let (vid, pid) = parse_vid_pid(arg.unwrap_or_default())?;
    let device = known(vid, pid).ok_or_else(|| unknown_device(vid, pid))?;
    let family = device.family;

    let log_path = query_log::path()
        .ok_or_else(|| refusal(vid, pid, "no data directory to record the query time"))?;
    let mut log = QueryLog::load_from(&log_path);
    floor_check(&mut log, &log_path, family, vid, pid, SystemTime::now())?;

    let inventory = door::inventory(Some((vid, pid)));
    let (mut target, target_line) = match family.link() {
        Link::Hid => open_hid_target(&inventory, family, vid, pid)?,
        Link::Serial => open_serial_target(&inventory, vid, pid)?,
    };
    claim_query(&mut log, &log_path, family, vid, pid, SystemTime::now())?;

    let exchange = Door::new(1)
        .send(&mut target)
        .map_err(|error| send_failed(device, &format!("{error:#}")))?;
    let is_reading = matches!(exchange.reply, Some((_, Decoded::Reading(_))));
    Ok((exchange_text(device, &target_line, &exchange), is_reading))
}

/// Exactly `hhhh:hhhh` hex, any case.
fn parse_vid_pid(arg: &str) -> Result<(u16, u16), String> {
    let hex = |part: &str| {
        let is_four_hex = part.len() == 4 && part.bytes().all(|byte| byte.is_ascii_hexdigit());
        is_four_hex
            .then(|| u16::from_str_radix(part, 16).ok())
            .flatten()
    };
    let (vid, pid) = arg.split_once(':').ok_or(USAGE)?;
    match (hex(vid), hex(pid)) {
        (Some(vid), Some(pid)) => Ok((vid, pid)),
        _ => Err(USAGE.to_owned()),
    }
}

/// A refusal; detail lines in `why` push the closing sentence onto its own line.
fn refusal(vid: u16, pid: u16, why: &str) -> String {
    let close = if why.contains('\n') { "\n" } else { "; " };
    format!("probe {vid:04X}:{pid:04X}: {why}{close}nothing was sent")
}

fn unknown_device(vid: u16, pid: u16) -> String {
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
    refusal(vid, pid, &format!("is not a known device; known:{rows}"))
}

/// `wait <n> s`, rounded up so it never reads `wait 0 s`.
fn wait_text(wait: Duration) -> String {
    let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
    format!("wait {} s", secs.max(1))
}

/// Refuses within the family's floor. A stamp in the future (clock moved back) is replaced by
/// `now` and saved, so the device is free again one floor later instead of locked out.
fn floor_check(
    log: &mut QueryLog,
    log_path: &Path,
    family: Family,
    vid: u16,
    pid: u16,
    now: SystemTime,
) -> Result<(), String> {
    let last = log.last(vid, pid);
    if last.is_some_and(|last| last > now) {
        log.stamp(vid, pid, now);
        log.save_to(log_path)
            .map_err(|error| record_failed(vid, pid, &error))?;
        let why = format!(
            "clock moved back; recorded now, {}",
            wait_text(family.query_floor())
        );
        return Err(refusal(vid, pid, &why));
    }
    match floor_wait(family, last, now) {
        Some(wait) => Err(refusal(vid, pid, &wait_text(wait))),
        None => Ok(()),
    }
}

/// Re-reads the log file, checks the floor, then saves the stamp; only a saved stamp lets the
/// query go out. The re-read catches another probe that claimed since this one loaded the log.
/// shortcut: re-read narrows but does not close the race between two processes; add a lock
/// file (`create_new`) before milestone 3's refresh worker shares this log.
fn claim_query(
    log: &mut QueryLog,
    log_path: &Path,
    family: Family,
    vid: u16,
    pid: u16,
    now: SystemTime,
) -> Result<(), String> {
    *log = QueryLog::load_from(log_path);
    floor_check(log, log_path, family, vid, pid, now)?;
    log.stamp(vid, pid, now);
    log.save_to(log_path)
        .map_err(|error| record_failed(vid, pid, &error))
}

/// A door error. Door refusals before the write say "nothing was sent"; any other error comes
/// from the write or the read, so the query went out (or may have) and the floor applies; a
/// refused HID write is usually a vendor app holding the device, hence the hint.
fn send_failed(device: &KnownDevice, error: &str) -> String {
    let head = format!("probe {:04X}:{:04X}: {error}", device.vid, device.pid);
    if error.contains("nothing was sent") {
        return head;
    }
    [
        head,
        bytes_line("sent", battery_query(device.family)),
        "the query was (or may have been) sent; the next probe waits for the floor".to_owned(),
        "if a vendor app is open, close it and wait for the floor before the next probe".to_owned(),
    ]
    .join("\n")
}

/// Names the OS error only: the outer contexts carry the data folder path (R10).
fn record_failed(vid: u16, pid: u16, error: &anyhow::Error) -> String {
    let why = format!("could not record the query time ({})", error.root_cause());
    refusal(vid, pid, &why)
}

/// The one USB serial port of `vid:pid`; macOS `/dev/tty.*` twins of `/dev/cu.*` are ignored.
fn pick_port(ports: &[SerialEntry], vid: u16, pid: u16) -> Result<usize, PickError> {
    let found: Vec<usize> = ports
        .iter()
        .enumerate()
        .filter(|(_, port)| {
            (port.vid, port.pid) == (vid, pid) && !port.port_name.starts_with("/dev/tty.")
        })
        .map(|(index, _)| index)
        .collect();
    match found.as_slice() {
        [] => Err(PickError::NoMatch),
        [only] => Ok(*only),
        _ => Err(PickError::Ambiguous(found)),
    }
}

/// Listing failures, one indented line each; empty when the listing worked.
fn listing_errors(inventory: &Inventory) -> String {
    inventory
        .errors
        .iter()
        .map(|error| format!("\n  {error}"))
        .collect()
}

fn usage_pair(shape: &HidShape) -> String {
    format!("{:04X}:{:04X}", shape.usage_page, shape.usage)
}

/// Picks among the collections enumeration opened; the picked index is only ever used on
/// that same `opened` list.
fn open_hid_target(
    inventory: &Inventory,
    family: Family,
    vid: u16,
    pid: u16,
) -> Result<(Target, String), String> {
    let all: Vec<&HidCollection> = inventory
        .hid
        .iter()
        .filter(|collection| (collection.vid, collection.pid) == (vid, pid))
        .collect();
    if all.is_empty() {
        let why = format!("is not connected{}", listing_errors(inventory));
        return Err(refusal(vid, pid, &why));
    }
    let opened: Vec<&HidCollection> = all
        .iter()
        .copied()
        .filter(|collection| matches!(collection.access, Access::Opened { .. }))
        .collect();
    let shapes: Vec<HidShape> = opened
        .iter()
        .map(|collection| collection.shape.clone())
        .collect();

    match pick_target(family, &shapes) {
        Ok(index) => {
            let collection = opened[index];
            let target = inventory
                .open_hid(collection)
                .map_err(|error| refusal(vid, pid, &format!("{error:#}")))?;
            let interface = collection
                .shape
                .interface
                .map_or("?".to_owned(), |n| n.to_string());
            let line = format!(
                "target HID if {interface} {} · path {}",
                usage_pair(&collection.shape),
                redact_path(&collection.path)
            );
            Ok((target, line))
        }
        Err(PickError::NoMatch) => {
            let mut why = format!(
                "no collection matches the {} shape (see juicetray scan)",
                family.name()
            );
            for collection in &all {
                let usage = usage_pair(&collection.shape);
                match &collection.access {
                    Access::Failed(text) => why.push_str(&format!(
                        "\n  {usage} open failed: {text}; close the vendor app"
                    )),
                    Access::Skipped(reason) => {
                        why.push_str(&format!("\n  {usage} not opened ({reason})"))
                    }
                    Access::Opened { .. } => {}
                }
            }
            Err(refusal(vid, pid, &why))
        }
        Err(PickError::Ambiguous(indices)) => {
            let mut why = format!(
                "several collections match the {} shape; unplug the extra receiver",
                family.name()
            );
            for collection in indices.iter().filter_map(|&index| opened.get(index)) {
                why.push_str(&format!(
                    "\n  {} path {}",
                    usage_pair(&collection.shape),
                    redact_path(&collection.path)
                ));
            }
            Err(refusal(vid, pid, &why))
        }
    }
}

fn open_serial_target(
    inventory: &Inventory,
    vid: u16,
    pid: u16,
) -> Result<(Target, String), String> {
    let port_name =
        |entry: &SerialEntry| mask_serial(&entry.port_name, entry.serial_number.as_deref());
    match pick_port(&inventory.serial, vid, pid) {
        Ok(index) => {
            let entry = &inventory.serial[index];
            let target = inventory
                .open_serial(entry)
                .map_err(|error| refusal(vid, pid, &format!("{error:#}")))?;
            Ok((target, format!("target serial {}", port_name(entry))))
        }
        Err(PickError::NoMatch) => {
            let why = format!("is not connected{}", listing_errors(inventory));
            Err(refusal(vid, pid, &why))
        }
        Err(PickError::Ambiguous(indices)) => {
            let mut why = "several serial ports match; unplug the extra receiver".to_owned();
            for entry in indices
                .iter()
                .filter_map(|&index| inventory.serial.get(index))
            {
                why.push_str(&format!("\n  {}", port_name(entry)));
            }
            Err(refusal(vid, pid, &why))
        }
    }
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
            decoded_text(decoded)
        }
        None => match exchange.family.link() {
            Link::Hid => format!(
                "no reply within {} ms (asleep? move it or put it on, then run probe once more \
                 after the floor)",
                HID_REPLY_TIMEOUT.as_millis()
            ),
            Link::Serial => format!("no reply within {} s", SERIAL_REPLY_TIMEOUT.as_secs()),
        },
    };
    lines.push(format!("result: {result}"));
    lines.join("\n")
}

fn decoded_text(decoded: &Decoded) -> String {
    match decoded {
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
    use std::fs;
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;

    use protocol::{Reading, decode};

    use super::*;

    /// A fresh folder under the OS temp dir, unique per test and process.
    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("juicetray-probe-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn whole(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    const NOW: u64 = 1_800_000_000;
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

    fn port(name: &str, vid: u16, pid: u16) -> SerialEntry {
        SerialEntry {
            port_name: name.into(),
            vid,
            pid,
            manufacturer: None,
            product: None,
            serial_number: None,
        }
    }

    #[test]
    fn parse_vid_pid_cases() {
        assert_eq!(parse_vid_pid("3837:6011"), Ok((0x3837, 0x6011)));
        assert_eq!(parse_vid_pid("373b:1204"), Ok((0x373B, 0x1204)));
        for bad in [
            "3837",
            "3837:601",
            "3837:60111",
            "0x3837:0x6011",
            "zzzz:6011",
            "",
            "+837:6011",
        ] {
            let error = parse_vid_pid(bad).expect_err(bad);
            assert!(error.contains("juicetray probe 3837:6011"), "{error}");
        }
    }

    #[test]
    fn probe_refuses_unknown() {
        let text = probe(Some("046D:C52B")).expect_err("unknown device must be refused");
        assert!(text.contains("046D:C52B"), "{text}");
        assert!(text.contains("not a known device"), "{text}");
        assert!(
            text.contains("3837:6011  MCHOSE V9 Pro 2 (dongle) · MCHOSE"),
            "{text}"
        );
        assert!(text.ends_with("nothing was sent"), "{text}");

        let usage = probe(None).expect_err("missing argument");
        assert_eq!(usage, USAGE);
    }

    #[test]
    fn refusal_always_ends_with_nothing_sent() {
        assert_eq!(
            refusal(0x3837, 0x6011, "wait 3 s"),
            "probe 3837:6011: wait 3 s; nothing was sent"
        );
        assert_eq!(
            refusal(0x3837, 0x6011, "head\n  detail"),
            "probe 3837:6011: head\n  detail\nnothing was sent"
        );
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
    fn pick_port_ignores_tty() {
        let ports = [
            port("/dev/cu.usbmodem1", 0x373B, 0x1204),
            port("/dev/tty.usbmodem1", 0x373B, 0x1204),
        ];
        assert_eq!(pick_port(&ports, 0x373B, 0x1204), Ok(0));
    }

    #[test]
    fn pick_port_ambiguous() {
        let ports = [port("COM3", 0x373B, 0x1204), port("COM4", 0x373B, 0x1204)];
        assert_eq!(
            pick_port(&ports, 0x373B, 0x1204),
            Err(PickError::Ambiguous(vec![0, 1]))
        );
    }

    #[test]
    fn pick_port_none() {
        let ports = [port("COM5", 0x046D, 0xC52B), port("COM6", 0x046D, 0xC52B)];
        assert_eq!(pick_port(&ports, 0x373B, 0x1204), Err(PickError::NoMatch));
        assert_eq!(pick_port(&[], 0x373B, 0x1204), Err(PickError::NoMatch));
    }

    #[test]
    fn wait_text_rounds_up() {
        assert_eq!(wait_text(Duration::from_millis(239_200)), "wait 240 s");
        assert_eq!(wait_text(Duration::from_millis(200)), "wait 1 s");
        assert_eq!(wait_text(Duration::from_secs(10)), "wait 10 s");
        assert_eq!(wait_text(Duration::ZERO), "wait 1 s");
    }

    #[test]
    fn claim_within_floor_leaves_log_untouched() {
        let root = temp_root("within");
        let path = root.join("query-log.toml");
        let (vid, pid) = MCHOSE;
        let mut log = QueryLog::default();
        log.stamp(vid, pid, whole(NOW - 60));
        log.save_to(&path).unwrap();
        let before = fs::read(&path).unwrap();

        let result = claim_query(&mut log, &path, Family::Mchose, vid, pid, whole(NOW));
        let after = fs::read(&path).unwrap();
        fs::remove_dir_all(&root).unwrap();

        let text = result.expect_err("within the floor");
        assert!(text.contains("wait 240 s"), "{text}");
        assert!(text.ends_with("nothing was sent"), "{text}");
        assert_eq!(log.last(vid, pid), Some(whole(NOW - 60)));
        assert_eq!(after, before);
    }

    #[test]
    fn claim_save_failure_refuses() {
        let root = temp_root("blocked");
        let blocker = root.join("blocker");
        fs::write(&blocker, "a file, not a folder").unwrap();
        let path = blocker.join("query-log.toml");
        let (vid, pid) = MCHOSE;
        let mut log = QueryLog::default();

        let result = claim_query(&mut log, &path, Family::Mchose, vid, pid, whole(NOW));
        fs::remove_dir_all(&root).unwrap();

        let text = result.expect_err("an unsaved stamp must refuse");
        assert!(text.contains("could not record the query time"), "{text}");
        assert!(text.ends_with("nothing was sent"), "{text}");
        assert!(!text.contains("blocker"), "the data path stays out: {text}");
    }

    #[test]
    fn claim_future_stamp_restamps_now() {
        let root = temp_root("future");
        let path = root.join("query-log.toml");
        let (vid, pid) = MCHOSE;
        let mut log = QueryLog::default();
        log.stamp(vid, pid, whole(NOW + 3600));
        log.save_to(&path).unwrap();

        let result = claim_query(&mut log, &path, Family::Mchose, vid, pid, whole(NOW));
        let saved = QueryLog::load_from(&path);
        fs::remove_dir_all(&root).unwrap();

        let text = result.expect_err("a future stamp refuses");
        assert!(text.contains("clock moved back"), "{text}");
        assert!(text.contains("wait 300 s"), "{text}");
        assert!(text.ends_with("nothing was sent"), "{text}");
        assert_eq!(log.last(vid, pid), Some(whole(NOW)));
        assert_eq!(saved.last(vid, pid), Some(whole(NOW)));
    }

    #[test]
    fn claim_ok_stamps_and_saves() {
        let root = temp_root("ok");
        let path = root.join("query-log.toml");
        let (vid, pid) = MCHOSE;
        let mut log = QueryLog::default();
        let now = whole(NOW) + Duration::from_millis(400);

        let result = claim_query(&mut log, &path, Family::Mchose, vid, pid, now);
        let saved = QueryLog::load_from(&path);
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(result, Ok(()));
        assert_eq!(saved.last(vid, pid), Some(whole(NOW)));
    }

    #[test]
    fn claim_rereads_log() {
        let root = temp_root("reread");
        let path = root.join("query-log.toml");
        let (vid, pid) = MCHOSE;
        // The caller loaded an empty log; another probe has since stamped the file.
        let mut stale = QueryLog::default();
        let mut other = QueryLog::default();
        other.stamp(vid, pid, whole(NOW - 60));
        other.save_to(&path).unwrap();
        let before = fs::read(&path).unwrap();

        let result = claim_query(&mut stale, &path, Family::Mchose, vid, pid, whole(NOW));
        let after = fs::read(&path).unwrap();
        fs::remove_dir_all(&root).unwrap();

        let text = result.expect_err("the other probe's stamp is within the floor");
        assert!(text.contains("wait 240 s"), "{text}");
        assert!(text.ends_with("nothing was sent"), "{text}");
        assert_eq!(after, before);
    }

    #[test]
    fn send_failed_shows_sent_bytes_after_the_write() {
        let text = send_failed(mchose(), "write failed: boom");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "probe 3837:6011: write failed: boom");
        assert_eq!(
            lines[1],
            format!("sent (64 B): {}", hex(battery_query(Family::Mchose)))
        );
        assert!(lines[2].contains("may have been) sent"), "{text}");
        assert!(lines[2].contains("floor"), "{text}");
        assert!(lines[3].contains("vendor app is open"), "{text}");

        let refused = send_failed(mchose(), "write budget used up; nothing was sent");
        assert_eq!(
            refused,
            "probe 3837:6011: write budget used up; nothing was sent"
        );
    }

    #[test]
    fn floor_check_passes_after_the_floor_without_stamping() {
        let root = temp_root("after");
        let path = root.join("query-log.toml");
        let (vid, pid) = (0x3554, 0xFA09);
        let mut log = QueryLog::default();
        log.stamp(vid, pid, whole(NOW - 10));

        let result = floor_check(&mut log, &path, Family::Compx, vid, pid, whole(NOW));
        let written = path.exists();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(result, Ok(()));
        assert_eq!(log.last(vid, pid), Some(whole(NOW - 10)));
        assert!(!written);
    }
}
