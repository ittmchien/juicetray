//! Query steps shared by `juicetray probe` and the refresh worker.

use std::path::Path;
use std::time::{Duration, SystemTime};

use door::{Access, HidCollection, Inventory, SerialEntry, Target};
use protocol::{Family, HidShape, PickError, floor_wait, pick_target};

use crate::query_log::QueryLog;
use crate::scan::{mask_serial, redact_path};

pub(crate) const USAGE: &str = "usage: juicetray probe <VID:PID>, e.g. juicetray probe 3837:6011";

/// Exactly `hhhh:hhhh` hex, any case.
pub(crate) fn parse_vid_pid(arg: &str) -> Result<(u16, u16), String> {
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
pub(crate) fn refusal(vid: u16, pid: u16, why: &str) -> String {
    let close = if why.contains('\n') { "\n" } else { "; " };
    format!("probe {vid:04X}:{pid:04X}: {why}{close}nothing was sent")
}

/// `wait <n> s`, rounded up so it never reads `wait 0 s`.
pub(crate) fn wait_text(wait: Duration) -> String {
    let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
    format!("wait {} s", secs.max(1))
}

/// Refuses within the family's floor. A stamp in the future (clock moved back) is replaced by
/// `now` and saved, so the device is free again one floor later instead of locked out.
pub(crate) fn floor_check(
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
pub(crate) fn claim_query(
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

/// Names the OS error only: the outer contexts carry the data folder path (R10).
pub(crate) fn record_failed(vid: u16, pid: u16, error: &anyhow::Error) -> String {
    let why = format!("could not record the query time ({})", error.root_cause());
    refusal(vid, pid, &why)
}

/// The one USB serial port of `vid:pid`; macOS `/dev/tty.*` twins of `/dev/cu.*` are ignored.
pub(crate) fn pick_port(ports: &[SerialEntry], vid: u16, pid: u16) -> Result<usize, PickError> {
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
pub(crate) fn listing_errors(inventory: &Inventory) -> String {
    inventory
        .errors
        .iter()
        .map(|error| format!("\n  {error}"))
        .collect()
}

pub(crate) fn usage_pair(shape: &HidShape) -> String {
    format!("{:04X}:{:04X}", shape.usage_page, shape.usage)
}

/// Picks among the collections enumeration opened; the picked index is only ever used on
/// that same `opened` list.
pub(crate) fn open_hid_target(
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

pub(crate) fn open_serial_target(
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;

    use super::*;
    use crate::query_log::QueryLog;

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
