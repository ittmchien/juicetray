//! Receiver and cable devices (USB HID or serial) for the refresh worker: one inventory and one
//! door per read, at most one query per present known device, never past the query floor.
//! Every decision before the first I/O is the pure `plan`; `view` turns the outcome into a row.

use std::path::Path as FsPath;
use std::time::SystemTime;

use door::{Door, HidCollection, Inventory, SerialEntry};
use protocol::{Decoded, Family, KNOWN, KnownDevice, Link, PickError, Reading};
use ui_model::{DeviceKind, DeviceView, Level, Path};

use crate::query::{
    ClaimError, TargetError, claim_query, floor_check, open_hid_target, open_serial_target,
    pick_hid, pick_port,
};
use crate::query_log::{self, QueryLog};

/// Queries one read may send across all devices.
const READ_BUDGET: u8 = 8;

const ASLEEP_TEXT: &str = "asleep or off";

/// What happened to one device in this read; everything but `Reading` and `Waiting` is a
/// failed or refused attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    NoChannel,
    Unconfirmed,
    Waiting,
    Busy,
    Ambiguous,
    NotRecorded,
    BudgetUsed,
    NoReply,
    Invalid(&'static str),
    Reading(Reading),
}

/// The pre-I/O verdict: show an outcome without touching the device, or open and query it.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Show(Outcome),
    Query,
}

/// Rows for every known device that is plugged in, in `KNOWN` order. `allow` lists the
/// unconfirmed devices the owner lets the worker query.
pub fn read_receivers(allow: &[(u16, u16)], now: SystemTime) -> Vec<DeviceView> {
    let ids: Vec<(u16, u16)> = KNOWN
        .iter()
        .map(|device| (device.vid, device.pid))
        .collect();
    let inventory = door::inventory(&ids);
    for error in &inventory.errors {
        eprintln!("receivers: {error}");
    }
    let log_path = query_log::path();
    let log = log_path
        .as_deref()
        .map(QueryLog::load_from)
        .unwrap_or_default();
    let mut door = Door::new(READ_BUDGET);

    KNOWN
        .iter()
        .filter(|device| is_present(&inventory, device))
        .map(|device| {
            let (vid, pid) = (device.vid, device.pid);
            let allowed = device.confirmed || allow.contains(&(vid, pid));
            let pick = pick_channel(&inventory.hid, &inventory.serial, &inventory.errors, device)
                .map_err(|error| target_outcome(&error));
            let floor = floor_check(&log, device.family, vid, pid, now);
            let outcome = match plan(allowed, pick, floor, door.writes_left() > 0) {
                Step::Show(outcome) => outcome,
                Step::Query => query(&inventory, device, &mut door, log_path.as_deref()),
            };
            view(device, outcome, log.reading(vid, pid), now)
        })
        .collect()
}

/// Any HID collection or serial port with the device's VID:PID.
fn is_present(inventory: &Inventory, device: &KnownDevice) -> bool {
    let id = (device.vid, device.pid);
    inventory.hid.iter().any(|hid| (hid.vid, hid.pid) == id)
        || inventory
            .serial
            .iter()
            .any(|port| (port.vid, port.pid) == id)
}

/// The family's target picked from the listing only (no I/O); mouse and keyboard collections
/// are never candidates because enumeration does not open them.
fn pick_channel(
    hid: &[HidCollection],
    serial: &[SerialEntry],
    errors: &[String],
    device: &KnownDevice,
) -> Result<(), TargetError> {
    let (vid, pid, family) = (device.vid, device.pid, device.family);
    match family.link() {
        Link::Hid => pick_hid(hid, errors, family, vid, pid).map(|_| ()),
        Link::Serial => pick_port(serial, vid, pid)
            .map(|_| ())
            .map_err(|error| match error {
                PickError::NoMatch => TargetError::NoChannel(Vec::new()),
                PickError::Ambiguous(_) => TargetError::Ambiguous(Vec::new()),
            }),
    }
}

/// A device that is present but lacks the family's channel shows as having none.
fn target_outcome(error: &TargetError) -> Outcome {
    match error {
        TargetError::NotConnected(_) | TargetError::NoChannel(_) => Outcome::NoChannel,
        TargetError::Held { .. } => Outcome::Busy,
        TargetError::Ambiguous(_) => Outcome::Ambiguous,
    }
}

/// Safety order: no channel, then confirmation, then the floor, then the pick's own refusal,
/// then the budget. A future stamp (`ClockBack`) goes on to `claim_query`, which replaces it
/// and refuses, so nothing is sent then either.
fn plan(
    allowed: bool,
    pick: Result<(), Outcome>,
    floor: Result<(), ClaimError>,
    has_budget: bool,
) -> Step {
    if pick == Err(Outcome::NoChannel) {
        return Step::Show(Outcome::NoChannel);
    }
    if !allowed {
        return Step::Show(Outcome::Unconfirmed);
    }
    if matches!(floor, Err(ClaimError::Wait(_))) {
        return Step::Show(Outcome::Waiting);
    }
    if let Err(outcome) = pick {
        return Step::Show(outcome);
    }
    if !has_budget {
        return Step::Show(Outcome::BudgetUsed);
    }
    Step::Query
}

/// Opens the target, claims the query time (saved before the write), sends the one query and
/// records a valid reading. No retries; full errors go to the log only.
fn query(
    inventory: &Inventory,
    device: &KnownDevice,
    door: &mut Door,
    log_path: Option<&FsPath>,
) -> Outcome {
    let (vid, pid, family) = (device.vid, device.pid, device.family);
    let opened = match family.link() {
        Link::Hid => open_hid_target(inventory, family, vid, pid),
        Link::Serial => open_serial_target(inventory, vid, pid),
    };
    let mut target = match opened {
        Ok((target, _)) => target,
        Err(error) => {
            eprintln!("receiver {vid:04X}:{pid:04X}: {error:?}");
            return target_outcome(&error);
        }
    };
    let claimed = match log_path {
        Some(path) => claim_query(path, family, vid, pid, SystemTime::now()),
        None => Err(ClaimError::Record("no data directory".to_owned())),
    };
    match claimed {
        Ok(()) => {}
        Err(ClaimError::Record(cause)) => {
            eprintln!("receiver {vid:04X}:{pid:04X}: could not record the query time ({cause})");
            return Outcome::NotRecorded;
        }
        Err(ClaimError::Wait(_) | ClaimError::ClockBack) => return Outcome::Waiting,
    }

    let exchange = match door.send(&mut target) {
        Ok(exchange) => exchange,
        Err(error) => {
            eprintln!("receiver {vid:04X}:{pid:04X}: {error:#}");
            return Outcome::Busy;
        }
    };
    let outcome = match exchange.reply {
        None => Outcome::NoReply,
        Some((_, Decoded::Reading(reading))) => Outcome::Reading(reading),
        Some((_, Decoded::Invalid(why))) => Outcome::Invalid(why),
        // The door never keeps a non-reply as the reply.
        Some((_, Decoded::NotTheReply)) => Outcome::Invalid("not the reply"),
    };
    if let (Outcome::Reading(reading), Some(path)) = (outcome, log_path) {
        let recorded = QueryLog::update(path, |log| {
            log.record_reading(vid, pid, reading, SystemTime::now());
            Ok::<_, anyhow::Error>(())
        });
        if let Err(error) = recorded {
            eprintln!(
                "receiver {vid:04X}:{pid:04X}: could not record the reading ({})",
                error.root_cause()
            );
        }
    }
    outcome
}

/// The menu text for an outcome.
fn reason(device: &KnownDevice, outcome: Outcome) -> String {
    match outcome {
        Outcome::NoChannel => "no battery channel (needs a capture)".to_owned(),
        Outcome::Unconfirmed => format!(
            "unconfirmed: run juicetray probe {:04X}:{:04X}",
            device.vid, device.pid
        ),
        Outcome::Waiting => "waiting for the next query".to_owned(),
        Outcome::Busy => "busy: close the vendor app".to_owned(),
        Outcome::Ambiguous => "several matches: unplug the extra receiver".to_owned(),
        Outcome::NotRecorded => "could not record the query time".to_owned(),
        Outcome::BudgetUsed => "query budget used".to_owned(),
        Outcome::NoReply => "no reply (asleep? move it)".to_owned(),
        Outcome::Invalid(why) => why.to_owned(),
        Outcome::Reading(_) => ASLEEP_TEXT.to_owned(),
    }
}

fn kind(family: Family) -> DeviceKind {
    match family {
        Family::Compx | Family::Report13 | Family::AtkBitmouse => DeviceKind::Mouse,
        Family::Mchose | Family::AtkSerial => DeviceKind::Headset,
    }
}

/// A fresh reading is shown as is. Otherwise the stored reading keeps its percent and time,
/// dimmed with the reason when this attempt failed; with none stored the reason is the level.
fn view(
    device: &KnownDevice,
    outcome: Outcome,
    stored: Option<(Reading, SystemTime)>,
    now: SystemTime,
) -> DeviceView {
    let (shown, updated_at, failed) = match (outcome, stored) {
        (Outcome::Reading(reading), _) => (Some(reading), now, false),
        (Outcome::Waiting, Some((reading, read_at))) => (Some(reading), read_at, false),
        (_, Some((reading, read_at))) => (Some(reading), read_at, true),
        (_, None) => (None, now, false),
    };
    let row = DeviceView {
        id: format!("usb-{:04x}-{:04x}", device.vid, device.pid),
        name: device.name.to_owned(),
        kind: kind(device.family),
        path: if device.cable {
            Path::Cable
        } else {
            Path::Dongle
        },
        level: Level::NoData(reason(device, outcome)),
        charging: false,
        connected: true,
        updated_at,
        stale: false,
        note: None,
    };
    let Some(reading) = shown else {
        return row;
    };
    let note = if failed {
        Some(reason(device, outcome))
    } else {
        reading.asleep.then(|| ASLEEP_TEXT.to_owned())
    };
    DeviceView {
        level: Level::Percent(reading.percent),
        charging: reading.charging,
        stale: failed || reading.asleep,
        note,
        ..row
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use door::Access;
    use protocol::{HidShape, known};

    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    const NOW: u64 = 1_800_000_000;

    fn device(vid: u16, pid: u16) -> &'static KnownDevice {
        known(vid, pid).unwrap()
    }

    fn mchose() -> &'static KnownDevice {
        device(0x3837, 0x6011)
    }

    fn reading(percent: u8, charging: bool, asleep: bool) -> Reading {
        Reading {
            percent,
            charging,
            asleep,
        }
    }

    /// Every attempt that ends without a fresh reading or a floor wait.
    const FAILURES: [Outcome; 8] = [
        Outcome::NoChannel,
        Outcome::Unconfirmed,
        Outcome::Busy,
        Outcome::Ambiguous,
        Outcome::NotRecorded,
        Outcome::BudgetUsed,
        Outcome::NoReply,
        Outcome::Invalid("on cable: level not trustworthy"),
    ];

    #[test]
    fn unconfirmed_never_reaches_the_door() {
        for floor in [Ok(()), Err(ClaimError::ClockBack)] {
            assert_eq!(
                plan(false, Ok(()), floor, true),
                Step::Show(Outcome::Unconfirmed)
            );
        }
        assert_eq!(
            plan(false, Err(Outcome::Busy), Ok(()), true),
            Step::Show(Outcome::Unconfirmed)
        );
        assert_eq!(plan(true, Ok(()), Ok(()), true), Step::Query);
    }

    #[test]
    fn no_channel_comes_before_confirmation() {
        for allowed in [false, true] {
            assert_eq!(
                plan(allowed, Err(Outcome::NoChannel), Ok(()), true),
                Step::Show(Outcome::NoChannel)
            );
        }
    }

    #[test]
    fn floor_comes_before_the_pick_and_the_budget() {
        let wait = || Err(ClaimError::Wait(Duration::from_secs(4)));
        assert_eq!(
            plan(true, Ok(()), wait(), true),
            Step::Show(Outcome::Waiting)
        );
        assert_eq!(
            plan(true, Err(Outcome::Busy), wait(), true),
            Step::Show(Outcome::Waiting)
        );
        assert_eq!(
            plan(true, Ok(()), wait(), false),
            Step::Show(Outcome::Waiting)
        );
    }

    #[test]
    fn pick_refusals_and_budget_skip_the_query() {
        for refused in [Outcome::Busy, Outcome::Ambiguous] {
            assert_eq!(plan(true, Err(refused), Ok(()), true), Step::Show(refused));
        }
        assert_eq!(
            plan(true, Ok(()), Ok(()), false),
            Step::Show(Outcome::BudgetUsed)
        );
        // A future stamp is left to `claim_query`, which replaces it and refuses.
        assert_eq!(
            plan(true, Ok(()), Err(ClaimError::ClockBack), true),
            Step::Query
        );
    }

    #[test]
    fn fresh_reading_row() {
        let row = view(
            mchose(),
            Outcome::Reading(reading(92, true, false)),
            Some((reading(40, false, false), at(NOW - 600))),
            at(NOW),
        );
        assert_eq!(
            row,
            DeviceView {
                id: "usb-3837-6011".into(),
                name: "MCHOSE V9 Pro 2 (dongle)".into(),
                kind: DeviceKind::Headset,
                path: Path::Dongle,
                level: Level::Percent(92),
                charging: true,
                connected: true,
                updated_at: at(NOW),
                stale: false,
                note: None,
            }
        );
    }

    #[test]
    fn asleep_reading_is_dimmed() {
        let row = view(
            mchose(),
            Outcome::Reading(reading(55, false, true)),
            None,
            at(NOW),
        );
        assert_eq!(row.level, Level::Percent(55));
        assert!(row.stale);
        assert_eq!(row.note.as_deref(), Some("asleep or off"));
    }

    #[test]
    fn within_floor_shows_the_stored_reading_with_its_age() {
        let row = view(
            mchose(),
            Outcome::Waiting,
            Some((reading(70, true, false), at(NOW - 120))),
            at(NOW),
        );
        assert_eq!(row.level, Level::Percent(70));
        assert!(row.charging);
        assert_eq!(row.updated_at, at(NOW - 120));
        assert!(!row.stale);
        assert_eq!(row.note, None);

        let asleep = view(
            mchose(),
            Outcome::Waiting,
            Some((reading(70, false, true), at(NOW - 120))),
            at(NOW),
        );
        assert!(asleep.stale);
        assert_eq!(asleep.note.as_deref(), Some("asleep or off"));

        let empty = view(mchose(), Outcome::Waiting, None, at(NOW));
        assert_eq!(
            empty.level,
            Level::NoData("waiting for the next query".into())
        );
        assert!(!empty.stale);
        assert_eq!(empty.note, None);
    }

    #[test]
    fn every_failure_keeps_the_stored_percent_dimmed() {
        let stored = (reading(61, true, false), at(NOW - 300));
        for outcome in FAILURES {
            let row = view(mchose(), outcome, Some(stored), at(NOW));
            assert_eq!(row.level, Level::Percent(61), "{outcome:?}");
            assert!(row.charging, "{outcome:?}");
            assert_eq!(row.updated_at, at(NOW - 300), "{outcome:?}");
            assert!(row.stale, "{outcome:?}");
            assert_eq!(row.note, Some(reason(mchose(), outcome)), "{outcome:?}");
            assert!(row.connected);
        }
    }

    #[test]
    fn failure_without_stored_reading_is_no_data() {
        for outcome in FAILURES {
            let row = view(mchose(), outcome, None, at(NOW));
            assert_eq!(
                row.level,
                Level::NoData(reason(mchose(), outcome)),
                "{outcome:?}"
            );
            assert_eq!(row.note, None, "{outcome:?}");
            assert!(!row.stale, "{outcome:?}");
        }
    }

    #[test]
    fn reason_texts() {
        let vgn = device(0x3554, 0xFA09);
        let text = |outcome| reason(vgn, outcome);
        assert_eq!(
            text(Outcome::Unconfirmed),
            "unconfirmed: run juicetray probe 3554:FA09"
        );
        assert_eq!(
            text(Outcome::NoChannel),
            "no battery channel (needs a capture)"
        );
        assert_eq!(text(Outcome::Busy), "busy: close the vendor app");
        assert_eq!(text(Outcome::NoReply), "no reply (asleep? move it)");
        assert_eq!(text(Outcome::BudgetUsed), "query budget used");
        assert_eq!(
            text(Outcome::Invalid("unknown power state")),
            "unknown power state"
        );
    }

    #[test]
    fn ids_kinds_and_paths() {
        let cases = [
            (
                0x3554,
                0xFA09,
                "usb-3554-fa09",
                DeviceKind::Mouse,
                Path::Dongle,
            ),
            (
                0x3837,
                0x6010,
                "usb-3837-6010",
                DeviceKind::Headset,
                Path::Cable,
            ),
            (
                0x373B,
                0x1204,
                "usb-373b-1204",
                DeviceKind::Headset,
                Path::Dongle,
            ),
            (
                0x373B,
                0x1216,
                "usb-373b-1216",
                DeviceKind::Mouse,
                Path::Dongle,
            ),
            (
                0x391D,
                0x1A05,
                "usb-391d-1a05",
                DeviceKind::Mouse,
                Path::Cable,
            ),
        ];
        for (vid, pid, id, kind, path) in cases {
            let row = view(device(vid, pid), Outcome::Waiting, None, at(NOW));
            assert_eq!((row.id.as_str(), row.kind, row.path), (id, kind, path));
        }
    }

    #[test]
    fn target_errors_map_to_outcomes() {
        assert_eq!(
            target_outcome(&TargetError::NotConnected(Vec::new())),
            Outcome::NoChannel
        );
        assert_eq!(
            target_outcome(&TargetError::NoChannel(Vec::new())),
            Outcome::NoChannel
        );
        assert_eq!(
            target_outcome(&TargetError::Held {
                open_error: Some("denied".into()),
                lines: Vec::new(),
            }),
            Outcome::Busy
        );
        assert_eq!(
            target_outcome(&TargetError::Ambiguous(Vec::new())),
            Outcome::Ambiguous
        );
    }

    fn port(vid: u16, pid: u16) -> SerialEntry {
        SerialEntry {
            port_name: "COM3".into(),
            vid,
            pid,
            manufacturer: None,
            product: None,
            serial_number: None,
        }
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
                interface: Some(0),
                usage_page: 0x0001,
                usage: 0x0002,
                reports: Vec::new(),
            },
            access,
        }
    }

    #[test]
    fn pick_channel_without_io() {
        let n9 = device(0x373B, 0x1204);
        assert_eq!(pick_channel(&[], &[port(0x373B, 0x1204)], &[], n9), Ok(()));
        assert_eq!(
            pick_channel(&[], &[], &[], n9),
            Err(TargetError::NoChannel(Vec::new()))
        );
        let two = [port(0x373B, 0x1204), port(0x373B, 0x1204)];
        assert_eq!(
            pick_channel(&[], &two, &[], n9),
            Err(TargetError::Ambiguous(Vec::new()))
        );

        // A mouse collection is never opened by enumeration, so it never becomes a channel.
        let vgn = device(0x3554, 0xFA09);
        let mouse = [collection(0x3554, 0xFA09, Access::Skipped("system mouse"))];
        let picked = pick_channel(&mouse, &[], &[], vgn).map_err(|error| target_outcome(&error));
        assert_eq!(picked, Err(Outcome::NoChannel));
        assert_eq!(
            plan(true, picked, Ok(()), true),
            Step::Show(Outcome::NoChannel)
        );
    }
}
