//! Receiver and cable devices (USB HID or serial) for the refresh worker: presence is checked
//! from the device lists alone; only verified catalogue models, `allow` entries and custom
//! devices become rows and are ever opened, one door per read, at most one query per device,
//! never past the query floor. Every other device is never touched.
//! Every decision before the first I/O is the pure `plan`; `view` turns the outcome into a row.

use std::path::Path as FsPath;
use std::time::SystemTime;

use door::{Door, HidCollection, Inventory, Listing, SerialEntry, Target};
use protocol::{Decoded, Family, KNOWN, Kind, Link, ON_CABLE_TEXT, Online, PickError, Reading};
use ui_model::{DeviceKind, DeviceView, Level, Path};

use crate::device::{Device, lookup};
use crate::log;
use crate::query::{
    ClaimError, TargetError, claim_query, floor_check, open_hid_target, open_serial_target,
    pick_hid, pick_port,
};
use crate::query_log::{self, QueryLog};
use crate::vendor_apps;

/// Queries one read may send across all devices.
const READ_BUDGET: u8 = 8;

const ASLEEP_TEXT: &str = "sleep";

/// What happened to one device in this read; everything but `Reading` and `Waiting` is a
/// failed or refused attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    NoChannel,
    NeedsCapture,
    Waiting,
    Busy,
    Ambiguous,
    NotRecorded,
    BudgetUsed,
    NoReply,
    /// The receiver answered but its headset is not linked: no battery query was sent.
    Offline,
    Invalid(&'static str),
    /// The brand's vendor app (shown name) is open: nothing is listed in depth, opened or sent.
    Paused(&'static str),
    /// A valid reading and the time it was taken.
    Reading(Reading, SystemTime),
}

/// The pre-I/O verdict: show an outcome without touching the device, or open and query it.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Show(Outcome),
    Query,
}

/// Rows for every plugged-in device the query gate lets through: verified catalogue models,
/// `allow` entries and `custom` devices. Presence comes from the device lists alone; only the
/// openable devices are listed in depth (and opened), so every other one is never touched.
pub fn read_receivers(allow: &[(u16, u16)], custom: &[Device], now: SystemTime) -> Vec<DeviceView> {
    let ids: Vec<(u16, u16)> = KNOWN
        .iter()
        .map(|device| (device.vid, device.pid))
        .chain(custom.iter().map(|device| (device.vid, device.pid)))
        .collect();
    let listing = list_devices();
    let devices = queried(&listing.present(&ids), allow, custom);
    // One process check per cycle, before any device is touched.
    let vids: Vec<u16> = devices.iter().map(|device| device.vid).collect();
    let running = vendor_apps::running(&vids);
    let log_path = query_log::path();
    let log = load_log(log_path.as_deref());
    let openable = openable(&devices, &running);
    // Every openable device would only show its stored reading as waiting: no descriptor reads.
    let inventory = if only_waiting(&openable, &log, now) {
        log::trace(format_args!("inventory skipped: all openable devices wait"));
        None
    } else {
        list_in_depth(listing, &openable)
    };
    let mut door = Door::new(READ_BUDGET);

    let outcomes: Vec<(&Device, Outcome)> = devices
        .iter()
        .map(|device| {
            let outcome = attempt(
                device,
                &running,
                inventory.as_ref(),
                (&log, log_path.as_deref()),
                &mut door,
                now,
            );
            (device, outcome)
        })
        .collect();
    record_readings(log_path.as_deref(), &outcomes);
    outcomes
        .into_iter()
        .map(|(device, outcome)| {
            let (vid, pid) = (device.vid, device.pid);
            view(
                device,
                outcome,
                log.reading(vid, pid),
                log.last(vid, pid),
                now,
            )
        })
        .collect()
}

/// The query gate: the present devices that are verified in the catalogue, added to `allow`, or
/// custom, each once (catalogue first, as `lookup` resolves them).
fn queried(present: &[(u16, u16)], allow: &[(u16, u16)], custom: &[Device]) -> Vec<Device> {
    let mut devices: Vec<Device> = Vec::new();
    for &(vid, pid) in present {
        if devices
            .iter()
            .any(|device| (device.vid, device.pid) == (vid, pid))
        {
            continue;
        }
        let Some(device) = lookup(vid, pid, custom) else {
            continue;
        };
        if device.is_queried(allow) {
            devices.push(device);
        }
    }
    devices
}

/// One traced listing of the HID and serial devices (slow while one unplugs); presence and the
/// in-depth inventory both come from it.
fn list_devices() -> Listing {
    let _span = log::span("door::list");
    let listing = door::list();
    for error in listing.errors() {
        eprintln!("receivers: {error}");
    }
    listing
}

/// The gated devices whose own refusals and vendor apps let them through; only these are listed
/// in depth and opened.
fn openable<'a>(devices: &'a [Device], running: &[&str]) -> Vec<&'a Device> {
    devices
        .iter()
        .filter(|device| open_gate(device, running).is_ok())
        .collect()
}

/// The in-depth inventory of `openable` from the same listing. An empty filter would keep every
/// device, so with nothing openable no inventory is made.
fn list_in_depth(listing: Listing, openable: &[&Device]) -> Option<Inventory> {
    if openable.is_empty() {
        return None;
    }
    let ids: Vec<(u16, u16)> = openable
        .iter()
        .map(|device| (device.vid, device.pid))
        .collect();
    let _span = log::span("door::inventory");
    Some(listing.into_inventory(&ids))
}

/// Whether every device in `openable` is inside its query floor with a stored reading, so the
/// cycle shows each as waiting and needs no descriptor. A future stamp is not a wait (the claim
/// handles it), so it forces the full listing.
// shortcut: a channel that vanished inside the floor shows the stored reading, not "unsupported",
// until the floor ends and the next cycle lists in depth.
fn only_waiting(openable: &[&Device], log: &QueryLog, now: SystemTime) -> bool {
    openable.iter().all(|device| {
        let (vid, pid) = (device.vid, device.pid);
        log.reading(vid, pid).is_some()
            && matches!(
                floor_check(log, device.family, vid, pid, now),
                Err(ClaimError::Wait(_))
            )
    })
}

fn load_log(log_path: Option<&FsPath>) -> QueryLog {
    let _span = log::span("query log load");
    log_path.map(QueryLog::load_from).unwrap_or_default()
}

/// One device's outcome: the pure plan first, then the query only when the plan allows it.
fn attempt(
    device: &Device,
    running: &[&str],
    inventory: Option<&Inventory>,
    (log, log_path): (&QueryLog, Option<&FsPath>),
    door: &mut Door,
    now: SystemTime,
) -> Outcome {
    let _span = log::span_fmt(format_args!(
        "device {:04X}:{:04X} attempt",
        device.vid, device.pid
    ));
    let gate_result = open_gate(device, running);
    let pick = match inventory {
        Some(inv) if gate_result.is_ok() => {
            pick_channel(&inv.hid, &inv.serial, &inv.errors, device)
                .map_err(|error| target_outcome(&error))
        }
        _ => Ok(()),
    };
    let floor = floor_check(log, device.family, device.vid, device.pid, now);
    match (
        plan(
            gate_result,
            pick,
            floor,
            door.writes_left() >= writes_needed(device),
        ),
        inventory,
    ) {
        (Step::Show(outcome), _) => outcome,
        (Step::Query, Some(inv)) => query(inv, device, door, log_path),
        // `Query` is planned only for an openable device, which is in the inventory.
        (Step::Query, None) => Outcome::NoChannel,
    }
}

/// Whether a gated device may be opened at all: it must have a known working command.
fn gate(device: &Device) -> Result<(), Outcome> {
    if device.needs_capture {
        return Err(Outcome::NeedsCapture);
    }
    Ok(())
}

/// The gate, then the vendor-app pause: a second client hangs the brand's open app (MCHOSE HUB
/// froze on start beside juicetray), so its devices are left alone until it closes.
fn open_gate(device: &Device, running: &[&str]) -> Result<(), Outcome> {
    gate(device)?;
    match vendor_apps::paused_by(device.vid, running) {
        Some(app) => Err(Outcome::Paused(app)),
        None => Ok(()),
    }
}

/// The family's target picked from the listing only (no I/O); mouse and keyboard collections
/// are never candidates because enumeration does not open them.
fn pick_channel(
    hid: &[HidCollection],
    serial: &[SerialEntry],
    errors: &[String],
    device: &Device,
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

/// Safety order: the gate (capture, open vendor app), then no channel, then the floor,
/// then the pick's own refusal, then the budget. A future stamp (`ClockBack`) goes on to
/// `claim_query`, which replaces it and refuses, so nothing is sent then either.
fn plan(
    gate: Result<(), Outcome>,
    pick: Result<(), Outcome>,
    floor: Result<(), ClaimError>,
    has_budget: bool,
) -> Step {
    if let Err(outcome) = gate {
        return Step::Show(outcome);
    }
    if pick == Err(Outcome::NoChannel) {
        return Step::Show(Outcome::NoChannel);
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

/// Opens the target, claims the query time (saved before the first write), sends the status
/// query where the device has one, then the battery query, and records a valid reading. No
/// retries; full errors go to the log only.
fn query(
    inventory: &Inventory,
    device: &Device,
    door: &mut Door,
    log_path: Option<&FsPath>,
) -> Outcome {
    let (vid, pid, family) = (device.vid, device.pid, device.family);
    let opened = {
        let _span = log::span_fmt(format_args!("device {vid:04X}:{pid:04X} open"));
        match family.link() {
            Link::Hid => open_hid_target(inventory, family, vid, pid),
            Link::Serial => open_serial_target(inventory, vid, pid),
        }
    };
    let mut target = match opened {
        Ok((target, _)) => target,
        Err(error) => {
            eprintln!("receiver {vid:04X}:{pid:04X}: {error:?}");
            log::line(&format!(
                "receiver {vid:04X}:{pid:04X}: open failed: {error:?}"
            ));
            return target_outcome(&error);
        }
    };
    let claimed = {
        let _span = log::span_fmt(format_args!("device {vid:04X}:{pid:04X} claim"));
        match log_path {
            Some(path) => claim_query(path, family, vid, pid, SystemTime::now()),
            None => Err(ClaimError::Record("no data directory".to_owned())),
        }
    };
    match claimed {
        Ok(()) => {}
        Err(ClaimError::Record(cause)) => {
            eprintln!("receiver {vid:04X}:{pid:04X}: could not record the query time ({cause})");
            log::line(&format!(
                "receiver {vid:04X}:{pid:04X}: claim failed, query time not recorded ({cause})"
            ));
            return Outcome::NotRecorded;
        }
        Err(ClaimError::Wait(_) | ClaimError::ClockBack) => return Outcome::Waiting,
    }

    if online_first(device)
        && let Some(outcome) = status_verdict(door, &mut target, device)
    {
        log::trace(format_args!("{} {}", row_id(device), outcome_text(outcome)));
        return outcome;
    }

    let sent = {
        let _span = log::span_fmt(format_args!("device {vid:04X}:{pid:04X} send and decode"));
        // A custom device is outside the catalogue, so the door checks its family on the device;
        // its connection picks the first frame (a BITMOUSE cable mouse takes target 0).
        if device.custom {
            door.send_as(&mut target, family, device.cable)
        } else {
            door.send(&mut target)
        }
    };
    let exchange = match sent {
        Ok(exchange) => exchange,
        Err(error) => {
            eprintln!("receiver {vid:04X}:{pid:04X}: {error:#}");
            log::line(&format!(
                "receiver {vid:04X}:{pid:04X}: send failed: {error:#}"
            ));
            return Outcome::Busy;
        }
    };
    // One time for the shown reading and the stored one; it is never before the claimed stamp.
    let read_at = SystemTime::now();
    let outcome = match exchange.reply {
        None => Outcome::NoReply,
        Some((_, Decoded::Reading(reading))) => Outcome::Reading(reading, read_at),
        Some((_, Decoded::Invalid(why))) => {
            eprintln!("receiver {vid:04X}:{pid:04X}: invalid reply: {why}");
            Outcome::Invalid(why)
        }
        // The door never keeps a non-reply as the reply, and only `send_online` yields `Online`.
        Some((_, Decoded::NotTheReply | Decoded::Online(_))) => Outcome::Invalid("not the reply"),
    };
    // One result line per query (not per step); traced only, so release stays silent.
    log::trace(format_args!("{} {}", row_id(device), outcome_text(outcome)));
    outcome
}

/// The valid readings among one cycle's outcomes, with their device ids.
fn readings_of(outcomes: &[(&Device, Outcome)]) -> Vec<(u16, u16, Reading, SystemTime)> {
    outcomes
        .iter()
        .filter_map(|&(device, outcome)| match outcome {
            Outcome::Reading(reading, at) => Some((device.vid, device.pid, reading, at)),
            _ => None,
        })
        .collect()
}

/// Stores all of a cycle's readings in one locked update of the query log; the query stamps were
/// already saved before each send. Nothing is written when no device gave a reading.
fn record_readings(log_path: Option<&FsPath>, outcomes: &[(&Device, Outcome)]) {
    let readings = readings_of(outcomes);
    let Some(path) = log_path.filter(|_| !readings.is_empty()) else {
        return;
    };
    let _span = log::span("record readings");
    let recorded = QueryLog::update(path, |log| {
        for &(vid, pid, reading, read_at) in &readings {
            log.record_reading(vid, pid, reading, read_at);
        }
        Ok::<_, anyhow::Error>(())
    });
    if let Err(error) = recorded {
        eprintln!(
            "receivers: could not record the readings ({})",
            error.root_cause()
        );
        log::line(&format!("receivers: record failed: {}", error.root_cause()));
    }
}

/// Whether the worker asks the online status before the battery. The ATK serial receiver keeps
/// answering the battery query with the last level after the headset is off; a cable
/// connection has no dongle link to ask about, and the status query is catalogue-only.
fn online_first(device: &Device) -> bool {
    device.family == Family::AtkSerial && !device.cable && !device.custom
}

/// Writes one try needs: the status query first, then the battery query.
fn writes_needed(device: &Device) -> u8 {
    1 + u8::from(online_first(device))
}

/// Sends the status query; `Some` ends the try (headset off, or the send failed and is not
/// retried), `None` goes on to the battery query. A missing or unusable status reply is
/// logged and falls through, so an unverified decoder never hides a working device.
fn status_verdict(door: &mut Door, target: &mut Target, device: &Device) -> Option<Outcome> {
    let (vid, pid) = (device.vid, device.pid);
    let sent = {
        let _span = log::span_fmt(format_args!("device {vid:04X}:{pid:04X} status"));
        door.send_online(target)
    };
    let exchange = match sent {
        Ok(exchange) => exchange,
        Err(error) => {
            eprintln!("receiver {vid:04X}:{pid:04X}: {error:#}");
            log::line(&format!(
                "receiver {vid:04X}:{pid:04X}: status send failed: {error:#}"
            ));
            return Some(Outcome::Busy);
        }
    };
    let decoded = exchange.reply.map(|(_, decoded)| decoded);
    match status_offline(decoded) {
        Some(true) => Some(Outcome::Offline),
        Some(false) => None,
        None => {
            log::line(&format!(
                "receiver {vid:04X}:{pid:04X}: status unusable ({decoded:?}); querying the battery"
            ));
            None
        }
    }
}

/// `Some(true)` = headset not linked, `Some(false)` = linked, `None` = no usable status.
fn status_offline(reply: Option<Decoded>) -> Option<bool> {
    match reply {
        Some(Decoded::Online(Online::No)) => Some(true),
        Some(Decoded::Online(Online::Yes)) => Some(false),
        _ => None,
    }
}

/// The result of a query for the log: the percent of a reading, else the outcome's name.
fn outcome_text(outcome: Outcome) -> String {
    match outcome {
        Outcome::Reading(reading, _) => format!("{}%", reading.percent),
        Outcome::NoReply => "no reply".to_owned(),
        other => format!("{other:?}"),
    }
}

/// The menu text for an outcome.
fn reason(outcome: Outcome) -> String {
    match outcome {
        Outcome::NoChannel | Outcome::NeedsCapture => "unsupported".to_owned(),
        Outcome::Waiting => "…".to_owned(),
        Outcome::Busy => "in use".to_owned(),
        Outcome::Ambiguous => "2 receivers".to_owned(),
        Outcome::NotRecorded | Outcome::BudgetUsed => "error".to_owned(),
        // A device that never answered has nothing to show yet; it is not broken.
        Outcome::NoReply => "no data".to_owned(),
        Outcome::Offline => "offline".to_owned(),
        // The one wording of a pause, shown as the level or after the kept reading.
        Outcome::Paused(app) => format!("paused ({app} open)"),
        // The detailed `why` is logged where the reply is decoded; the menu keeps one word.
        Outcome::Invalid(_) if on_cable(outcome) => "charging".to_owned(),
        Outcome::Invalid(_) => "error".to_owned(),
        Outcome::Reading(..) => ASLEEP_TEXT.to_owned(),
    }
}

fn on_cable(outcome: Outcome) -> bool {
    matches!(outcome, Outcome::Invalid(why) if why == ON_CABLE_TEXT)
}

/// The row id the menu and config use for a receiver or cable device.
fn row_id(device: &Device) -> String {
    format!("usb-{:04x}-{:04x}", device.vid, device.pid)
}

/// The row kind (icon and group) of a catalogue model or custom device.
fn kind(kind: Kind) -> DeviceKind {
    match kind {
        Kind::Mouse => DeviceKind::Mouse,
        Kind::Headset => DeviceKind::Headset,
        Kind::Keyboard => DeviceKind::Keyboard,
        Kind::Gamepad => DeviceKind::Gamepad,
        Kind::Other => DeviceKind::Other,
    }
}

/// A fresh reading is shown as is. Otherwise the stored reading keeps its percent and time,
/// dimmed with the reason when this attempt failed. Within the floor the previous attempt
/// decides: a query stamped after the stored reading got no valid reply, so it is dimmed too.
/// A pause keeps the stored reading undimmed with the pause note. With none stored the reason is
/// the level.
fn view(
    device: &Device,
    outcome: Outcome,
    stored: Option<(Reading, SystemTime)>,
    last_query: Option<SystemTime>,
    now: SystemTime,
) -> DeviceView {
    let (shown, updated_at, failed) = match (outcome, stored) {
        (Outcome::Reading(reading, read_at), _) => (Some(reading), read_at, false),
        (Outcome::Waiting, Some((reading, read_at))) => {
            let unanswered = last_query.is_some_and(|last| last > read_at);
            (Some(reading), read_at, unanswered)
        }
        // Nothing was asked, so nothing failed: the last level stays as it was.
        (Outcome::Paused(_), Some((reading, read_at))) => (Some(reading), read_at, false),
        (_, Some((reading, read_at))) => (Some(reading), read_at, true),
        (_, None) => (None, now, false),
    };
    let row = DeviceView {
        id: row_id(device),
        name: device.name.clone(),
        kind: kind(device.kind),
        path: if device.cable {
            Path::Cable
        } else {
            Path::Dongle
        },
        level: Level::NoData(reason(outcome)),
        charging: false,
        // An unlinked headset is a disconnected row: the menu greys it and keeps the stored level.
        connected: outcome != Outcome::Offline,
        updated_at,
        stale: false,
        note: None,
    };
    let Some(reading) = shown else {
        return row;
    };
    // A dimmed reading with its age already says the device did not answer, so that adds no note;
    // a cable shows only the bolt.
    // A paused row says why its reading does not move and is never dimmed or marked offline;
    // the pause outranks a stored asleep flag, which is unknown while paused.
    let paused = matches!(outcome, Outcome::Paused(_));
    let note = if paused {
        Some(reason(outcome))
    } else if failed {
        match outcome {
            Outcome::Waiting if reading.asleep => Some(ASLEEP_TEXT.to_owned()),
            Outcome::Waiting | Outcome::NoReply | Outcome::Offline => None,
            _ if on_cable(outcome) => None,
            _ => Some(reason(outcome)),
        }
    } else {
        reading.asleep.then(|| ASLEEP_TEXT.to_owned())
    };
    DeviceView {
        level: Level::Percent(reading.percent),
        charging: reading.charging || on_cable(outcome),
        stale: !paused && (failed || reading.asleep),
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
    use crate::device::tests::custom;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    const NOW: u64 = 1_800_000_000;

    /// A catalogue row as the worker sees it; leaked so a test can hold `&'static` rows.
    fn device(vid: u16, pid: u16) -> &'static Device {
        Box::leak(Box::new(Device::from(known(vid, pid).unwrap())))
    }

    fn mchose() -> &'static Device {
        device(0x3837, 0x6011)
    }

    fn reading(percent: u8, charging: bool, asleep: bool) -> Reading {
        Reading {
            percent,
            charging,
            asleep,
        }
    }

    #[test]
    fn only_waiting_needs_a_floor_wait_and_a_stored_reading_for_every_device() {
        let (mchose, n9) = (mchose(), device(0x373B, 0x1204));
        let floor = mchose.family.query_floor().as_secs();
        let mut log = QueryLog::default();

        // Nothing openable: nothing to list either way.
        assert!(only_waiting(&[], &log, at(NOW)));
        // No stamp or reading yet: the device must be queried.
        assert!(!only_waiting(&[mchose], &log, at(NOW)));

        // Stamped inside the floor, but no stored reading: no stored level to show, list in depth.
        log.stamp(mchose.vid, mchose.pid, at(NOW - 1));
        assert!(!only_waiting(&[mchose], &log, at(NOW)));

        log.record_reading(
            mchose.vid,
            mchose.pid,
            reading(80, false, false),
            at(NOW - 1),
        );
        assert!(only_waiting(&[mchose], &log, at(NOW)));
        // The floor is over: a query is due.
        assert!(!only_waiting(&[mchose], &log, at(NOW + floor)));
        // A future stamp (clock moved back) goes through the claim, not the wait.
        assert!(!only_waiting(&[mchose], &log, at(NOW - 10)));

        // One device that is due keeps the whole listing.
        log.stamp(n9.vid, n9.pid, at(NOW - 1));
        log.record_reading(n9.vid, n9.pid, reading(50, false, false), at(NOW - 1));
        assert!(only_waiting(&[mchose, n9], &log, at(NOW)));
        assert!(!only_waiting(
            &[mchose, device(0x3554, 0xFA09)],
            &log,
            at(NOW)
        ));
        // A custom device with nothing stored is never skipped.
        let mine = custom(0x1234, 0x5678, Family::Compx);
        assert!(!only_waiting(&[mchose, &mine], &log, at(NOW)));
    }

    #[test]
    fn one_cycle_records_every_reading_in_one_update() {
        let (mchose, n9) = (mchose(), device(0x373B, 0x1204));
        let outcomes = [
            (mchose, Outcome::Reading(reading(80, false, false), at(NOW))),
            (n9, Outcome::NoReply),
            (
                device(0x3554, 0xFA09),
                Outcome::Reading(reading(55, true, false), at(NOW + 1)),
            ),
        ];
        // Only valid readings are batched; a failed device records nothing.
        let readings = readings_of(&outcomes);
        assert_eq!(readings.len(), 2);

        let root = std::env::temp_dir().join(format!("juicetray-receivers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let path = root.join("query-log.toml");
        std::fs::create_dir_all(&root).unwrap();
        let mut stamped = QueryLog::default();
        stamped.stamp(0x373B, 0x1204, at(NOW));
        stamped.save_to(&path).unwrap();

        record_readings(Some(&path), &outcomes);

        let log = QueryLog::load_from(&path);
        assert_eq!(
            log.reading(0x3837, 0x6011),
            Some((reading(80, false, false), at(NOW)))
        );
        assert_eq!(
            log.reading(0x3554, 0xFA09),
            Some((reading(55, true, false), at(NOW + 1)))
        );
        assert_eq!(log.reading(0x373B, 0x1204), None);
        // The stamp saved before the send is untouched.
        assert_eq!(log.last(0x373B, 0x1204), Some(at(NOW)));

        // No reading, no write.
        let none =
            std::env::temp_dir().join(format!("juicetray-receivers-none-{}", std::process::id()));
        record_readings(
            Some(&none.join("query-log.toml")),
            &[(n9, Outcome::NoReply)],
        );
        assert!(!none.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every attempt that ends without a fresh reading or a floor wait.
    const FAILURES: [Outcome; 8] = [
        Outcome::NoChannel,
        Outcome::NeedsCapture,
        Outcome::Busy,
        Outcome::Ambiguous,
        Outcome::NotRecorded,
        Outcome::BudgetUsed,
        Outcome::NoReply,
        Outcome::Invalid(ON_CABLE_TEXT),
    ];

    /// The ids of `devices`, in order.
    fn ids(devices: &[Device]) -> Vec<(u16, u16)> {
        devices.iter().map(|d| (d.vid, d.pid)).collect()
    }

    #[test]
    fn only_verified_allowed_or_custom_devices_become_rows() {
        let (mchose, vgn, f75) = ((0x3837, 0x6011), (0x391D, 0x1A05), (0x3554, 0xFA09));
        let mine = custom(0x1234, 0x5678, Family::Compx);
        let present = [mchose, vgn, f75, (0x1234, 0x5678), (0x046D, 0xC52B), mchose];

        // Nothing added: only the verified model, once.
        assert_eq!(ids(&queried(&present, &[], &[])), [mchose]);
        // `allow` lets an unverified catalogue model through; a custom device always passes.
        assert_eq!(
            ids(&queried(&present, &[f75], std::slice::from_ref(&mine))),
            [mchose, f75, (0x1234, 0x5678)]
        );
        // An unverified model with no `allow` entry stays out, whatever a custom entry says.
        let shadow = custom(vgn.0, vgn.1, Family::Compx);
        assert!(queried(&[vgn], &[], &[shadow]).is_empty());
        // A custom device that is not plugged in gives no row.
        assert!(queried(&[], &[], &[mine]).is_empty());
    }

    #[test]
    fn gate_blocks_a_device_that_needs_a_capture() {
        let vgn = device(0x391D, 0x1A05);
        let needs_capture = Device {
            needs_capture: true,
            ..vgn.clone()
        };
        assert_eq!(gate(&needs_capture), Err(Outcome::NeedsCapture));
        assert_eq!(gate(vgn), Ok(()));
        assert_eq!(gate(mchose()), Ok(()));
        assert_eq!(
            plan(gate(&needs_capture), Ok(()), Ok(()), true),
            Step::Show(Outcome::NeedsCapture)
        );
    }

    #[test]
    fn custom_bitmouse_is_queried_on_a_receiver_and_on_its_cable() {
        // Both connections pass the gate and are opened; the door picks each one's frame.
        let receiver = custom(0x373B, 0x2000, Family::AtkBitmouse);
        let cable = Device {
            cable: true,
            ..receiver.clone()
        };
        for device in [&receiver, &cable] {
            assert_eq!(gate(device), Ok(()));
            assert_eq!(plan(gate(device), Ok(()), Ok(()), true), Step::Query);
        }
        let both = [receiver, cable];
        let opened: Vec<(u16, u16, bool)> = openable(&both, &[])
            .iter()
            .map(|d| (d.vid, d.pid, d.cable))
            .collect();
        assert_eq!(opened, [(0x373B, 0x2000, false), (0x373B, 0x2000, true)]);
    }

    #[test]
    fn closed_gate_never_reaches_the_door() {
        for refused in [Outcome::NeedsCapture, Outcome::NoChannel] {
            for floor in [Ok(()), Err(ClaimError::ClockBack)] {
                assert_eq!(plan(Err(refused), Ok(()), floor, true), Step::Show(refused));
            }
            // The gate also outranks a missing channel and a floor wait.
            assert_eq!(
                plan(Err(refused), Err(Outcome::NoChannel), Ok(()), true),
                Step::Show(refused)
            );
            let wait = Err(ClaimError::Wait(Duration::from_secs(4)));
            assert_eq!(plan(Err(refused), Ok(()), wait, true), Step::Show(refused));
        }
        assert_eq!(plan(Ok(()), Ok(()), Ok(()), true), Step::Query);
    }

    #[test]
    fn no_channel_comes_before_the_floor() {
        let wait = Err(ClaimError::Wait(Duration::from_secs(4)));
        assert_eq!(
            plan(Ok(()), Err(Outcome::NoChannel), wait, true),
            Step::Show(Outcome::NoChannel)
        );
    }

    #[test]
    fn floor_comes_before_the_pick_and_the_budget() {
        let wait = || Err(ClaimError::Wait(Duration::from_secs(4)));
        assert_eq!(
            plan(Ok(()), Ok(()), wait(), true),
            Step::Show(Outcome::Waiting)
        );
        assert_eq!(
            plan(Ok(()), Err(Outcome::Busy), wait(), true),
            Step::Show(Outcome::Waiting)
        );
        assert_eq!(
            plan(Ok(()), Ok(()), wait(), false),
            Step::Show(Outcome::Waiting)
        );
    }

    #[test]
    fn pick_refusals_and_budget_skip_the_query() {
        for refused in [Outcome::Busy, Outcome::Ambiguous] {
            assert_eq!(
                plan(Ok(()), Err(refused), Ok(()), true),
                Step::Show(refused)
            );
        }
        assert_eq!(
            plan(Ok(()), Ok(()), Ok(()), false),
            Step::Show(Outcome::BudgetUsed)
        );
        // A future stamp is left to `claim_query`, which replaces it and refuses.
        assert_eq!(
            plan(Ok(()), Ok(()), Err(ClaimError::ClockBack), true),
            Step::Query
        );
    }

    #[test]
    fn fresh_reading_row() {
        let row = view(
            mchose(),
            Outcome::Reading(reading(92, true, false), at(NOW)),
            Some((reading(40, false, false), at(NOW - 600))),
            Some(at(NOW - 30)),
            at(NOW),
        );
        assert_eq!(
            row,
            DeviceView {
                id: "usb-3837-6011".into(),
                name: "MCHOSE V9 Pro 2".into(),
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
    fn open_vendor_app_pauses_its_brand_before_any_io() {
        let (mchose, n9) = (mchose(), device(0x373B, 0x1204));
        let devices = [mchose.clone(), n9.clone()];
        let hub = ["MCHOSE HUB.exe"];
        let paused = Outcome::Paused("MCHOSE HUB");
        assert_eq!(open_gate(mchose, &hub), Err(paused));
        // Another brand's app leaves the device alone; the background helpers never count.
        assert_eq!(open_gate(n9, &hub), Ok(()));
        assert_eq!(open_gate(mchose, &["MCHOSE HUB Agent.exe"]), Ok(()));
        // A paused device is never listed in depth, so it is never opened or sent to.
        assert_eq!(openable(&devices, &hub), [n9]);
        assert_eq!(openable(&devices, &[]).len(), 2);
        assert_eq!(
            plan(open_gate(mchose, &hub), Ok(()), Ok(()), true),
            Step::Show(paused)
        );
        // The device's own refusal still comes first.
        let mut needs_capture = mchose.clone();
        needs_capture.needs_capture = true;
        assert_eq!(open_gate(&needs_capture, &hub), Err(Outcome::NeedsCapture));
    }

    #[test]
    fn paused_row_keeps_the_last_level_undimmed_with_the_pause_note() {
        let paused = Outcome::Paused("MCHOSE HUB");
        let stored = (reading(87, false, true), at(NOW - 60));
        let row = view(mchose(), paused, Some(stored), Some(at(NOW - 60)), at(NOW));
        assert_eq!(row.level, Level::Percent(87));
        assert!(row.connected && !row.stale);
        assert_eq!(row.updated_at, at(NOW - 60));
        assert_eq!(
            ui_model::menu_line(&row, at(NOW)),
            "MCHOSE V9 Pro 2 — 2.4G · 87% · paused (MCHOSE HUB open)"
        );
        // Nothing stored yet: the pause is the level.
        let bare = view(mchose(), paused, None, None, at(NOW));
        assert_eq!(bare.level, Level::NoData("paused (MCHOSE HUB open)".into()));
        assert!(bare.connected && !bare.stale && bare.note.is_none());
    }

    #[test]
    fn asleep_reading_is_dimmed() {
        let row = view(
            mchose(),
            Outcome::Reading(reading(55, false, true), at(NOW)),
            None,
            None,
            at(NOW),
        );
        assert_eq!(row.level, Level::Percent(55));
        assert!(row.stale);
        assert_eq!(row.note.as_deref(), Some("sleep"));
    }

    #[test]
    fn within_floor_shows_the_stored_reading_with_its_age() {
        let row = view(
            mchose(),
            Outcome::Waiting,
            Some((reading(70, true, false), at(NOW - 120))),
            Some(at(NOW - 120)),
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
            None,
            at(NOW),
        );
        assert!(asleep.stale);
        assert_eq!(asleep.note.as_deref(), Some("sleep"));

        let empty = view(mchose(), Outcome::Waiting, None, Some(at(NOW - 5)), at(NOW));
        assert_eq!(empty.level, Level::NoData("…".into()));
        assert!(!empty.stale);
        assert_eq!(empty.note, None);
    }

    #[test]
    fn floor_wait_after_an_unanswered_query_dims_the_stored_reading() {
        let stored = Some((reading(70, true, false), at(NOW - 600)));
        // The last query was stamped after the stored reading and left no new one.
        let row = view(
            mchose(),
            Outcome::Waiting,
            stored,
            Some(at(NOW - 120)),
            at(NOW),
        );
        assert_eq!(row.level, Level::Percent(70));
        assert_eq!(row.updated_at, at(NOW - 600));
        assert!(row.stale);
        assert_eq!(row.note, None);

        // An asleep stored reading keeps its own note next to the unanswered-query one.
        let asleep = view(
            mchose(),
            Outcome::Waiting,
            Some((reading(70, true, true), at(NOW - 600))),
            Some(at(NOW - 120)),
            at(NOW),
        );
        assert!(asleep.stale);
        assert_eq!(asleep.note.as_deref(), Some("sleep"));

        // A reading recorded at the stamp's second is the answer to that query.
        let answered = view(
            mchose(),
            Outcome::Waiting,
            stored,
            Some(at(NOW - 600)),
            at(NOW),
        );
        assert!(!answered.stale);
        assert_eq!(answered.note, None);

        // Nothing stored: the waiting text stays the level.
        let empty = view(
            mchose(),
            Outcome::Waiting,
            None,
            Some(at(NOW - 120)),
            at(NOW),
        );
        assert_eq!(empty.level, Level::NoData("…".into()));
        assert!(!empty.stale);
    }

    #[test]
    fn every_failure_keeps_the_stored_percent_dimmed() {
        let stored = (reading(61, true, false), at(NOW - 300));
        for outcome in FAILURES {
            let row = view(
                mchose(),
                outcome,
                Some(stored),
                Some(at(NOW - 400)),
                at(NOW),
            );
            assert_eq!(row.level, Level::Percent(61), "{outcome:?}");
            assert!(row.charging, "{outcome:?}");
            assert_eq!(row.updated_at, at(NOW - 300), "{outcome:?}");
            assert!(row.stale, "{outcome:?}");
            let note = (outcome != Outcome::NoReply && !on_cable(outcome)).then(|| reason(outcome));
            assert_eq!(row.note, note, "{outcome:?}");
            assert!(row.connected);
        }
    }

    #[test]
    fn failure_without_stored_reading_is_no_data() {
        for outcome in FAILURES {
            let row = view(mchose(), outcome, None, None, at(NOW));
            assert_eq!(row.level, Level::NoData(reason(outcome)), "{outcome:?}");
            assert_eq!(row.note, None, "{outcome:?}");
            assert!(!row.stale, "{outcome:?}");
        }
    }

    #[test]
    fn only_atk_serial_dongles_ask_the_status_first() {
        for device in KNOWN.iter().map(Device::from) {
            let asks = device.family == Family::AtkSerial && !device.cable;
            assert_eq!(online_first(&device), asks);
            assert_eq!(writes_needed(&device), if asks { 2 } else { 1 });
        }
        let wired = device(0x373B, 0x11CA);
        assert!(wired.cable && wired.family == Family::AtkSerial);
        assert!(!online_first(wired));
        // The status query is catalogue-only, so a custom device goes straight to the battery.
        let mine = custom(0x373B, 0x2001, Family::AtkSerial);
        assert!(!online_first(&mine));
        assert_eq!(writes_needed(&mine), 1);
    }

    #[test]
    fn status_reply_verdicts() {
        assert_eq!(
            status_offline(Some(Decoded::Online(Online::No))),
            Some(true)
        );
        assert_eq!(
            status_offline(Some(Decoded::Online(Online::Yes))),
            Some(false)
        );
        // No reply, an unparseable one or an unsupported answer must not hide the device.
        for unusable in [
            None,
            Some(Decoded::Online(Online::Unsupported)),
            Some(Decoded::Invalid("x")),
            Some(Decoded::NotTheReply),
        ] {
            assert_eq!(status_offline(unusable), None, "{unusable:?}");
        }
    }

    #[test]
    fn offline_is_a_disconnected_row() {
        let n9 = device(0x373B, 0x1204);
        let stored = (reading(40, false, false), at(NOW - 3600));
        let kept = view(n9, Outcome::Offline, Some(stored), Some(at(NOW)), at(NOW));
        assert_eq!(kept.level, Level::Percent(40));
        assert!(!kept.connected);
        assert!(kept.is_offline());
        assert_eq!(kept.note, None);
        assert_eq!(kept.updated_at, at(NOW - 3600));

        let bare = view(n9, Outcome::Offline, None, None, at(NOW));
        assert_eq!(bare.level, Level::NoData("offline".into()));
        assert!(!bare.connected);

        // Every other outcome keeps the row connected.
        assert!(view(n9, Outcome::NoReply, None, None, at(NOW)).connected);
    }

    #[test]
    fn reason_texts() {
        let text = reason;
        assert_eq!(text(Outcome::Waiting), "…");
        assert_eq!(text(Outcome::NoChannel), "unsupported");
        assert_eq!(text(Outcome::NeedsCapture), "unsupported");
        assert_eq!(text(Outcome::Busy), "in use");
        assert_eq!(text(Outcome::NoReply), "no data");
        assert_eq!(text(Outcome::Offline), "offline");
        assert_eq!(text(Outcome::Paused("ATK HUB")), "paused (ATK HUB open)");
        assert_eq!(text(Outcome::BudgetUsed), "error");
        assert_eq!(text(Outcome::Invalid("unknown power state")), "error");
        assert_eq!(text(Outcome::Invalid(ON_CABLE_TEXT)), "charging");
    }

    #[test]
    fn ids_kinds_and_paths() {
        let cases = [
            (
                0x3554,
                0xFA09,
                "usb-3554-fa09",
                DeviceKind::Keyboard,
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
                Path::Dongle,
            ),
        ];
        for (vid, pid, id, kind, path) in cases {
            let row = view(device(vid, pid), Outcome::Waiting, None, None, at(NOW));
            assert_eq!((row.id.as_str(), row.kind, row.path), (id, kind, path));
        }
    }

    #[test]
    fn custom_device_row() {
        let mine = Device {
            kind: Kind::Headset,
            ..custom(0x1234, 0x5678, Family::Compx)
        };
        let fresh = Outcome::Reading(reading(64, false, false), at(NOW));
        let row = view(&mine, fresh, None, None, at(NOW));
        assert_eq!(
            (row.id.as_str(), row.name.as_str(), row.kind, row.path),
            (
                "usb-1234-5678",
                "My mouse",
                DeviceKind::Headset,
                Path::Dongle
            )
        );
        assert_eq!(row.level, Level::Percent(64));
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
            plan(Ok(()), picked, Ok(()), true),
            Step::Show(Outcome::NoChannel)
        );
    }
}
