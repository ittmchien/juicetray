//! Query steps shared by `juicetray probe` and the refresh worker.

use std::path::Path;
use std::time::{Duration, SystemTime};

use door::{Access, HidCollection, Inventory, SerialEntry, Target};
use protocol::{Family, HidShape, Link, PickError, floor_wait, pick_target};

use crate::query_log::QueryLog;
use crate::scan::{interface_label, mask_serial, redact_path};

pub(crate) const USAGE: &str =
    "usage: juicetray probe [--online] <VID:PID>, e.g. juicetray probe 3837:6011";

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

/// A probe refusal; detail lines in `why` push the closing sentence onto its own line.
pub(crate) fn refusal(vid: u16, pid: u16, why: &str) -> String {
    refusal_for("probe", vid, pid, why)
}

/// `refusal` for another subcommand (`listen` shares the device checks).
pub(crate) fn refusal_for(command: &str, vid: u16, pid: u16, why: &str) -> String {
    let close = if why.contains('\n') { "\n" } else { "; " };
    format!("{command} {vid:04X}:{pid:04X}: {why}{close}nothing was sent")
}

/// `wait <n> s`, rounded up so it never reads `wait 0 s`.
pub(crate) fn wait_text(wait: Duration) -> String {
    let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
    format!("wait {} s", secs.max(1))
}

/// Why a query may not go out; `claim_refusal` turns it into the user-facing text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClaimError {
    /// Within the family's floor; the time left.
    Wait(Duration),
    /// The stored stamp is in the future (clock moved back).
    ClockBack,
    /// The query time could not be saved; the OS error only (the outer contexts carry paths).
    Record(String),
}

impl From<anyhow::Error> for ClaimError {
    fn from(error: anyhow::Error) -> Self {
        ClaimError::Record(error.root_cause().to_string())
    }
}

/// The refusal text for a failed claim.
pub(crate) fn claim_refusal(vid: u16, pid: u16, family: Family, error: ClaimError) -> String {
    let why = match error {
        ClaimError::Wait(wait) => wait_text(wait),
        ClaimError::ClockBack => format!(
            "clock moved back; recorded now, {}",
            wait_text(family.query_floor())
        ),
        ClaimError::Record(cause) => format!("could not record the query time ({cause})"),
    };
    refusal(vid, pid, &why)
}

/// Read-only: refuses within the family's floor, and on a stamp in the future (clock moved
/// back), which `claim_query` replaces by `now`.
pub(crate) fn floor_check(
    log: &QueryLog,
    family: Family,
    vid: u16,
    pid: u16,
    now: SystemTime,
) -> Result<(), ClaimError> {
    let last = log.last(vid, pid);
    if last.is_some_and(|last| last > now) {
        return Err(ClaimError::ClockBack);
    }
    match floor_wait(family, last, now) {
        Some(wait) => Err(ClaimError::Wait(wait)),
        None => Ok(()),
    }
}

/// Under the log lock: re-reads the log file, checks the floor, then saves the stamp; only a
/// saved stamp lets the query go out. A future stamp is replaced by `now` and saved, so the
/// device is free again one floor later instead of locked out, and the claim still refuses.
pub(crate) fn claim_query(
    log_path: &Path,
    family: Family,
    vid: u16,
    pid: u16,
    now: SystemTime,
) -> Result<(), ClaimError> {
    QueryLog::update(log_path, |log| {
        match floor_check(log, family, vid, pid, now) {
            Ok(()) => {
                log.stamp(vid, pid, now);
                Ok(())
            }
            Err(ClaimError::ClockBack) => {
                log.stamp(vid, pid, now);
                log.save_to(log_path)?;
                Err(ClaimError::ClockBack)
            }
            Err(refused) => Err(refused),
        }
    })
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

/// Detail lines, each on its own indented line; empty for no lines.
fn indented(lines: &[String]) -> String {
    lines.iter().map(|line| format!("\n  {line}")).collect()
}

fn usage_pair(shape: &HidShape) -> String {
    format!("{:04X}:{:04X}", shape.usage_page, shape.usage)
}

/// Why no target could be picked or opened; `target_refusal` turns it into the user-facing
/// text. Detail strings are already redacted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TargetError {
    /// No such device in the listing; the listing failures, if any.
    NotConnected(Vec<String>),
    /// Something else holds the device: a collection failed to open during enumeration
    /// (`lines`), or opening the pick failed (`open_error`, the OS error without paths).
    Held {
        open_error: Option<String>,
        lines: Vec<String>,
    },
    /// Collections exist but none carries the family's battery channel; one line per
    /// collection that was not opened.
    NoChannel(Vec<String>),
    /// Several candidates fit; one redacted line each.
    Ambiguous(Vec<String>),
}

/// The probe refusal text for a target that could not be picked or opened.
pub(crate) fn target_refusal(vid: u16, pid: u16, family: Family, error: TargetError) -> String {
    refusal(vid, pid, &target_why(family, error))
}

/// The reason text of a target error, without the command prefix.
pub(crate) fn target_why(family: Family, error: TargetError) -> String {
    match error {
        TargetError::NotConnected(listing) => format!("is not connected{}", indented(&listing)),
        TargetError::Held {
            open_error: Some(cause),
            ..
        } => cause,
        TargetError::Held { lines, .. } | TargetError::NoChannel(lines) => format!(
            "no collection matches the {} shape (see juicetray scan){}",
            family.name(),
            indented(&lines)
        ),
        TargetError::Ambiguous(lines) => {
            let head = match family.link() {
                Link::Hid => format!(
                    "several collections match the {} shape; unplug the extra receiver",
                    family.name()
                ),
                Link::Serial => "several serial ports match; unplug the extra receiver".to_owned(),
            };
            format!("{head}{}", indented(&lines))
        }
    }
}

/// Picks among the collections enumeration opened; the picked index is only ever used on
/// that same `opened` list. No I/O: the refresh worker judges a device by it before opening.
pub(crate) fn pick_hid<'a>(
    hid: &'a [HidCollection],
    errors: &[String],
    family: Family,
    vid: u16,
    pid: u16,
) -> Result<&'a HidCollection, TargetError> {
    let all: Vec<&HidCollection> = hid
        .iter()
        .filter(|collection| (collection.vid, collection.pid) == (vid, pid))
        .collect();
    if all.is_empty() {
        return Err(TargetError::NotConnected(errors.to_vec()));
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
        Ok(index) => Ok(opened[index]),
        Err(PickError::NoMatch) => {
            let mut lines = Vec::new();
            let mut held = false;
            for collection in &all {
                let usage = usage_pair(&collection.shape);
                match &collection.access {
                    Access::Failed(text) => {
                        held = true;
                        lines.push(format!("{usage} open failed: {text}; close the vendor app"));
                    }
                    Access::Skipped(reason) => {
                        lines.push(format!("{usage} not opened ({reason})"));
                    }
                    Access::Opened { .. } => {}
                }
            }
            Err(if held {
                TargetError::Held {
                    open_error: None,
                    lines,
                }
            } else {
                TargetError::NoChannel(lines)
            })
        }
        Err(PickError::Ambiguous(indices)) => Err(TargetError::Ambiguous(
            indices
                .iter()
                .filter_map(|&index| opened.get(index))
                .map(|collection| {
                    format!(
                        "{} path {}",
                        usage_pair(&collection.shape),
                        redact_path(&collection.path)
                    )
                })
                .collect(),
        )),
    }
}

/// A failed open means the OS refused the pick (usually a vendor app holds it).
fn open_failed(error: &anyhow::Error) -> TargetError {
    TargetError::Held {
        open_error: Some(format!("{error:#}")),
        lines: Vec::new(),
    }
}

pub(crate) fn open_hid_target(
    inventory: &Inventory,
    family: Family,
    vid: u16,
    pid: u16,
) -> Result<(Target, String), TargetError> {
    let collection = pick_hid(&inventory.hid, &inventory.errors, family, vid, pid)?;
    let target = inventory
        .open_hid(collection)
        .map_err(|error| open_failed(&error))?;
    // The report's target line names the collection like `scan` does; the path is redacted.
    let line = format!(
        "target HID {} · path {}",
        interface_label(&collection.shape),
        redact_path(&collection.path)
    );
    Ok((target, line))
}

pub(crate) fn open_serial_target(
    inventory: &Inventory,
    vid: u16,
    pid: u16,
) -> Result<(Target, String), TargetError> {
    let port_name =
        |entry: &SerialEntry| mask_serial(&entry.port_name, entry.serial_number.as_deref());
    match pick_port(&inventory.serial, vid, pid) {
        Ok(index) => {
            let entry = &inventory.serial[index];
            let target = inventory
                .open_serial(entry)
                .map_err(|error| open_failed(&error))?;
            Ok((target, format!("target serial {}", port_name(entry))))
        }
        Err(PickError::NoMatch) => Err(TargetError::NotConnected(inventory.errors.clone())),
        Err(PickError::Ambiguous(indices)) => Err(TargetError::Ambiguous(
            indices
                .iter()
                .filter_map(|&index| inventory.serial.get(index))
                .map(port_name)
                .collect(),
        )),
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

    fn collection(vid: u16, pid: u16, access: Access) -> HidCollection {
        HidCollection {
            vid,
            pid,
            bus: "Usb".into(),
            manufacturer: None,
            product: None,
            has_serial: false,
            path: "P".into(),
            shape: HidShape {
                interface: Some(1),
                usage_page: 0xFF02,
                usage: 0x0002,
                reports: Vec::new(),
            },
            access,
        }
    }

    fn opened() -> Access {
        Access::Opened {
            descriptor: Vec::new(),
            parse_error: None,
        }
    }

    const COMPX: (u16, u16) = (0x3554, 0xFA09);

    #[test]
    fn pick_hid_held_when_a_collection_failed() {
        let hid = [collection(COMPX.0, COMPX.1, Access::Failed("busy".into()))];

        let error = pick_hid(&hid, &[], Family::Compx, COMPX.0, COMPX.1).expect_err("held");

        assert_eq!(
            error,
            TargetError::Held {
                open_error: None,
                lines: vec!["FF02:0002 open failed: busy; close the vendor app".into()],
            }
        );
        let text = target_refusal(COMPX.0, COMPX.1, Family::Compx, error);
        assert_eq!(
            text,
            "probe 3554:FA09: no collection matches the Compx report 8 shape (see juicetray scan)\n  FF02:0002 open failed: busy; close the vendor app\nnothing was sent"
        );
    }

    #[test]
    fn pick_hid_no_channel_when_none_fits() {
        let hid = [
            collection(COMPX.0, COMPX.1, opened()),
            collection(COMPX.0, COMPX.1, Access::Skipped("system mouse")),
        ];

        let error = pick_hid(&hid, &[], Family::Compx, COMPX.0, COMPX.1).expect_err("no channel");

        assert_eq!(
            error,
            TargetError::NoChannel(vec!["FF02:0002 not opened (system mouse)".into()])
        );
    }

    #[test]
    fn pick_hid_not_connected_when_the_id_is_absent() {
        let hid = [collection(0x046D, 0xC52B, opened())];
        let listing = ["serial ports could not be listed: boom".to_owned()];

        let error = pick_hid(&hid, &listing, Family::Compx, COMPX.0, COMPX.1).expect_err("absent");

        assert_eq!(error, TargetError::NotConnected(listing.to_vec()));
        assert_eq!(
            target_refusal(COMPX.0, COMPX.1, Family::Compx, error),
            "probe 3554:FA09: is not connected\n  serial ports could not be listed: boom\nnothing was sent"
        );
    }

    #[test]
    fn target_refusal_open_error_and_ambiguous() {
        let open = TargetError::Held {
            open_error: Some("could not open".into()),
            lines: Vec::new(),
        };
        assert_eq!(
            target_refusal(0x373B, 0x1204, Family::AtkSerial, open),
            "probe 373B:1204: could not open; nothing was sent"
        );
        let several = TargetError::Ambiguous(vec!["COM3".into(), "COM4".into()]);
        assert_eq!(
            target_refusal(0x373B, 0x1204, Family::AtkSerial, several),
            "probe 373B:1204: several serial ports match; unplug the extra receiver\n  COM3\n  COM4\nnothing was sent"
        );
    }

    #[test]
    fn wait_text_rounds_up() {
        assert_eq!(wait_text(Duration::from_millis(239_200)), "wait 240 s");
        assert_eq!(wait_text(Duration::from_millis(200)), "wait 1 s");
        assert_eq!(wait_text(Duration::from_secs(10)), "wait 10 s");
        assert_eq!(wait_text(Duration::ZERO), "wait 1 s");
    }

    fn claim_text(
        path: &Path,
        family: Family,
        (vid, pid): (u16, u16),
        now: SystemTime,
    ) -> Result<(), String> {
        claim_query(path, family, vid, pid, now)
            .map_err(|error| claim_refusal(vid, pid, family, error))
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

        let result = claim_text(&path, Family::Mchose, MCHOSE, whole(NOW));
        let after = fs::read(&path).unwrap();
        fs::remove_dir_all(&root).unwrap();

        let text = result.expect_err("within the floor");
        assert!(text.contains("wait 240 s"), "{text}");
        assert!(text.ends_with("nothing was sent"), "{text}");
        assert_eq!(after, before);
    }

    #[test]
    fn claim_save_failure_refuses() {
        let root = temp_root("blocked");
        let blocker = root.join("blocker");
        fs::write(&blocker, "a file, not a folder").unwrap();
        let path = blocker.join("query-log.toml");

        let result = claim_text(&path, Family::Mchose, MCHOSE, whole(NOW));
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

        let result = claim_text(&path, Family::Mchose, MCHOSE, whole(NOW));
        let saved = QueryLog::load_from(&path);
        fs::remove_dir_all(&root).unwrap();

        let text = result.expect_err("a future stamp refuses");
        assert!(text.contains("clock moved back"), "{text}");
        assert!(text.contains("wait 300 s"), "{text}");
        assert!(text.ends_with("nothing was sent"), "{text}");
        assert_eq!(saved.last(vid, pid), Some(whole(NOW)));
    }

    #[test]
    fn claim_ok_stamps_and_saves() {
        let root = temp_root("ok");
        let path = root.join("query-log.toml");
        let (vid, pid) = MCHOSE;
        let now = whole(NOW) + Duration::from_millis(400);

        let result = claim_query(&path, Family::Mchose, vid, pid, now);
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
        // Another probe has stamped the file after the caller last looked at it.
        let mut other = QueryLog::default();
        other.stamp(vid, pid, whole(NOW - 60));
        other.save_to(&path).unwrap();
        let before = fs::read(&path).unwrap();

        let result = claim_text(&path, Family::Mchose, MCHOSE, whole(NOW));
        let after = fs::read(&path).unwrap();
        fs::remove_dir_all(&root).unwrap();

        let text = result.expect_err("the other probe's stamp is within the floor");
        assert!(text.contains("wait 240 s"), "{text}");
        assert!(text.ends_with("nothing was sent"), "{text}");
        assert_eq!(after, before);
    }

    #[test]
    fn claim_keeps_stored_reading() {
        let root = temp_root("keeps-reading");
        let path = root.join("query-log.toml");
        let (vid, pid) = MCHOSE;
        let reading = protocol::Reading {
            percent: 92,
            charging: false,
            asleep: false,
        };
        let mut log = QueryLog::default();
        log.record_reading(vid, pid, reading, whole(NOW - 600));
        log.save_to(&path).unwrap();

        let result = claim_query(&path, Family::Mchose, vid, pid, whole(NOW));
        let saved = QueryLog::load_from(&path);
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(result, Ok(()));
        assert_eq!(saved.last(vid, pid), Some(whole(NOW)));
        assert_eq!(saved.reading(vid, pid), Some((reading, whole(NOW - 600))));
    }

    #[test]
    fn floor_check_passes_after_the_floor_without_stamping() {
        let (vid, pid) = (0x3554, 0xFA09);
        let mut log = QueryLog::default();
        log.stamp(vid, pid, whole(NOW - 10));

        let result = floor_check(&log, Family::Compx, vid, pid, whole(NOW));

        assert_eq!(result, Ok(()));
        assert_eq!(log.last(vid, pid), Some(whole(NOW - 10)));
    }

    #[test]
    fn floor_check_refuses_without_writing() {
        let root = temp_root("floor-readonly");
        let path = root.join("query-log.toml");
        let (vid, pid) = MCHOSE;
        let mut within = QueryLog::default();
        within.stamp(vid, pid, whole(NOW - 60));
        let mut future = QueryLog::default();
        future.stamp(vid, pid, whole(NOW + 3600));

        let wait = floor_check(&within, Family::Mchose, vid, pid, whole(NOW));
        let clock = floor_check(&future, Family::Mchose, vid, pid, whole(NOW));
        let written = path.exists() || root.join("query-log.lock").exists();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(wait, Err(ClaimError::Wait(Duration::from_secs(240))));
        assert_eq!(clock, Err(ClaimError::ClockBack));
        assert_eq!(future.last(vid, pid), Some(whole(NOW + 3600)));
        assert!(!written, "floor_check never touches the files");
    }
}
