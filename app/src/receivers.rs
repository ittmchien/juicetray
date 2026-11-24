//! Receiver and cable devices (USB HID or serial) for the refresh worker: presence is checked
//! from the device lists alone, only confirmed or allowed devices are ever opened, one door per
//! read, at most one query per such device, never past the query floor.
//! Every decision before the first I/O is the pure `plan`; `view` turns the outcome into a row.
//! An owner-asked check (`check_receiver`) is the same plan for one device with its
//! confirmation gate passed; nothing else is relaxed.

use std::collections::HashMap;
use std::path::Path as FsPath;
use std::time::{Duration, SystemTime};

use door::{Door, HidCollection, Inventory, SerialEntry};
use protocol::{
    Decoded, Family, KNOWN, KnownDevice, Link, ON_CABLE_TEXT, PickError, Reading, known,
};
use ui_model::{DeviceKind, DeviceView, Level, Path};

use crate::log;
use crate::query::{
    ClaimError, TargetError, claim_query, floor_check, open_hid_target, open_serial_target,
    pick_hid, pick_port, wait_text,
};
use crate::query_log::{self, QueryLog};

/// Queries one read may send across all devices.
const READ_BUDGET: u8 = 8;

const ASLEEP_TEXT: &str = "asleep";

/// Shown on a stored reading when the last query (newer than the reading) got no valid reply.
const LAST_QUERY_FAILED_TEXT: &str = "no reply";

/// What happened to one device in this read; everything but `Reading` and `Waiting` is a
/// failed or refused attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    NoChannel,
    NeedsCapture,
    Unconfirmed,
    Waiting,
    /// An owner-asked check within the floor: nothing was sent; the time left.
    TooSoon(Duration),
    Busy,
    Ambiguous,
    NotRecorded,
    BudgetUsed,
    NoReply,
    Invalid(&'static str),
    /// A valid reading and the time it was taken.
    Reading(Reading, SystemTime),
}

/// The pre-I/O verdict: show an outcome without touching the device, or open and query it.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Show(Outcome),
    Query,
}

/// Rows for every known device that is plugged in, in `KNOWN` order. `allow` lists the
/// unconfirmed devices the owner lets the worker query. Presence comes from the device lists
/// alone; only the openable devices are listed in depth (and opened), so every other one is
/// never touched. `notes` holds the last failed check per row id.
pub fn read_receivers(
    allow: &[(u16, u16)],
    notes: &CheckNotes,
    now: SystemTime,
) -> Vec<DeviceView> {
    let ids: Vec<(u16, u16)> = KNOWN
        .iter()
        .map(|device| (device.vid, device.pid))
        .collect();
    let present = present_ids(&ids);
    let inventory = list_in_depth(&openable(&present, allow));
    let log_path = query_log::path();
    let log = load_log(log_path.as_deref());
    let mut door = Door::new(READ_BUDGET);

    KNOWN
        .iter()
        .filter(|device| present.contains(&(device.vid, device.pid)))
        .map(|device| {
            let outcome = attempt(
                device,
                allow,
                inventory.as_ref(),
                (&log, log_path.as_deref()),
                &mut door,
                now,
            );
            let (vid, pid) = (device.vid, device.pid);
            let row = view(
                device,
                outcome,
                log.reading(vid, pid),
                log.last(vid, pid),
                now,
            );
            with_check_note(row, outcome, notes)
        })
        .collect()
}

/// The last failed check's reason per row id, kept by the worker for this session.
pub type CheckNotes = HashMap<String, String>;

/// An owner-asked check's result: the device's new row, and the `VID:PID` to add to `allow`
/// (only a valid reading of a device not confirmed in the table).
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub row: DeviceView,
    pub confirm: Option<(u16, u16)>,
    outcome: Outcome,
}

impl Checked {
    /// A reading clears the device's note and a failed query sets it (latest wins); a floor
    /// wait sent nothing, so the earlier note stays.
    pub fn remember(&self, notes: &mut CheckNotes) {
        match self.outcome {
            Outcome::Reading(..) => {
                notes.remove(&self.row.id);
            }
            Outcome::TooSoon(_) | Outcome::Waiting => {}
            failed => {
                notes.insert(self.row.id.clone(), reason(failed));
            }
        }
    }
}

/// A device the gate still holds back shows its last check's reason as the note, so a read
/// does not hide why the check failed.
fn with_check_note(mut row: DeviceView, outcome: Outcome, notes: &CheckNotes) -> DeviceView {
    if outcome == Outcome::Unconfirmed
        && let Some(note) = notes.get(&row.id)
    {
        row.note = Some(note.clone());
    }
    row
}

/// One query for the known device whose row id is `id`, on the owner's click: the same plan,
/// floor, claim and door as a read, with the confirmation gate passed for this id alone, and
/// only this device listed in depth. `None` for an id outside `KNOWN` or an unplugged device.
pub fn check_receiver(id: &str, allow: &[(u16, u16)], now: SystemTime) -> Option<Checked> {
    let (device, allow) = check_scope(id, allow)?;
    let present = present_ids(&[(device.vid, device.pid)]);
    if present.is_empty() {
        return None;
    }
    let inventory = list_in_depth(&openable(&present, &allow));
    let log_path = query_log::path();
    let log = load_log(log_path.as_deref());
    let mut door = Door::new(1);
    let outcome = attempt(
        device,
        &allow,
        inventory.as_ref(),
        (&log, log_path.as_deref()),
        &mut door,
        now,
    );
    // The owner clicked, so a floor wait shows the time left instead of the waiting text.
    let floor = floor_check(&log, device.family, device.vid, device.pid, now);
    let outcome = match (outcome, floor) {
        (Outcome::Waiting, Err(ClaimError::Wait(left))) => Outcome::TooSoon(left),
        (outcome, _) => outcome,
    };
    let (vid, pid) = (device.vid, device.pid);
    Some(checked(
        device,
        outcome,
        log.reading(vid, pid),
        log.last(vid, pid),
        now,
    ))
}

/// Whether the menu offers "Check battery now" for the row `id`: a known device the gate holds
/// back only for want of confirmation.
pub fn can_check(id: &str, allow: &[(u16, u16)]) -> bool {
    known_receiver(id).is_some_and(|device| gate(device, allow) == Err(Outcome::Unconfirmed))
}

/// The known device behind the row id `id` and `allow` with that device added, so the gate
/// opens for it alone; `None` for any other id.
fn check_scope(id: &str, allow: &[(u16, u16)]) -> Option<(&'static KnownDevice, Vec<(u16, u16)>)> {
    let device = known_receiver(id)?;
    let mut allow = allow.to_vec();
    allow.push((device.vid, device.pid));
    Some((device, allow))
}

fn known_receiver(id: &str) -> Option<&'static KnownDevice> {
    KNOWN.iter().find(|device| row_id(device) == id)
}

/// The row of a check; only a valid reading confirms, and only a device the table leaves
/// unconfirmed.
fn checked(
    device: &KnownDevice,
    outcome: Outcome,
    stored: Option<(Reading, SystemTime)>,
    last_query: Option<SystemTime>,
    now: SystemTime,
) -> Checked {
    let is_reading = matches!(outcome, Outcome::Reading(..));
    Checked {
        row: view(device, outcome, stored, last_query, now),
        confirm: (is_reading && !device.confirmed).then_some((device.vid, device.pid)),
        outcome,
    }
}

/// `door::present`, traced: it lists HID and serial devices, which is slow while one unplugs.
fn present_ids(ids: &[(u16, u16)]) -> Vec<(u16, u16)> {
    let _span = log::span("door::present");
    door::present(ids)
}

/// The present devices the gate lets through; only these are listed in depth and opened.
fn openable(present: &[(u16, u16)], allow: &[(u16, u16)]) -> Vec<(u16, u16)> {
    present
        .iter()
        .copied()
        .filter(|&(vid, pid)| known(vid, pid).is_some_and(|d| gate(d, allow).is_ok()))
        .collect()
}

/// The in-depth listing of `openable`. An empty filter would list every device, so with
/// nothing openable no inventory is made.
fn list_in_depth(openable: &[(u16, u16)]) -> Option<Inventory> {
    let inventory = (!openable.is_empty()).then(|| {
        let _span = log::span("door::inventory");
        door::inventory(openable)
    })?;
    for error in &inventory.errors {
        eprintln!("receivers: {error}");
    }
    Some(inventory)
}

fn load_log(log_path: Option<&FsPath>) -> QueryLog {
    let _span = log::span("query log load");
    log_path.map(QueryLog::load_from).unwrap_or_default()
}

/// One device's outcome: the pure plan first, then the query only when the plan allows it.
fn attempt(
    device: &KnownDevice,
    allow: &[(u16, u16)],
    inventory: Option<&Inventory>,
    (log, log_path): (&QueryLog, Option<&FsPath>),
    door: &mut Door,
    now: SystemTime,
) -> Outcome {
    let _span = log::span(format!(
        "device {:04X}:{:04X} attempt",
        device.vid, device.pid
    ));
    let gate_result = gate(device, allow);
    let pick = match inventory {
        Some(inv) if gate_result.is_ok() => {
            pick_channel(&inv.hid, &inv.serial, &inv.errors, device)
                .map_err(|error| target_outcome(&error))
        }
        _ => Ok(()),
    };
    let floor = floor_check(log, device.family, device.vid, device.pid, now);
    match (
        plan(gate_result, pick, floor, door.writes_left() > 0),
        inventory,
    ) {
        (Step::Show(outcome), _) => outcome,
        (Step::Query, Some(inv)) => query(inv, device, door, log_path),
        // `Query` is planned only for an openable device, which is in the inventory.
        (Step::Query, None) => Outcome::NoChannel,
    }
}

/// Whether the device may be opened at all: it must have a known working command, and be
/// confirmed or allowed by the owner.
fn gate(device: &KnownDevice, allow: &[(u16, u16)]) -> Result<(), Outcome> {
    if device.needs_capture {
        return Err(Outcome::NeedsCapture);
    }
    if device.confirmed || allow.contains(&(device.vid, device.pid)) {
        return Ok(());
    }
    Err(Outcome::Unconfirmed)
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

/// Safety order: the gate (capture, confirmation), then no channel, then the floor, then the
/// pick's own refusal, then the budget. A future stamp (`ClockBack`) goes on to `claim_query`,
/// which replaces it and refuses, so nothing is sent then either.
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

/// Opens the target, claims the query time (saved before the write), sends the one query and
/// records a valid reading. No retries; full errors go to the log only.
fn query(
    inventory: &Inventory,
    device: &KnownDevice,
    door: &mut Door,
    log_path: Option<&FsPath>,
) -> Outcome {
    let (vid, pid, family) = (device.vid, device.pid, device.family);
    let opened = {
        let _span = log::span(format!("device {vid:04X}:{pid:04X} open"));
        match family.link() {
            Link::Hid => open_hid_target(inventory, family, vid, pid),
            Link::Serial => open_serial_target(inventory, vid, pid),
        }
    };
    let mut target = match opened {
        Ok((target, _)) => target,
        Err(error) => {
            eprintln!("receiver {vid:04X}:{pid:04X}: {error:?}");
            return target_outcome(&error);
        }
    };
    let claimed = {
        let _span = log::span(format!("device {vid:04X}:{pid:04X} claim"));
        match log_path {
            Some(path) => claim_query(path, family, vid, pid, SystemTime::now()),
            None => Err(ClaimError::Record("no data directory".to_owned())),
        }
    };
    match claimed {
        Ok(()) => {}
        Err(ClaimError::Record(cause)) => {
            eprintln!("receiver {vid:04X}:{pid:04X}: could not record the query time ({cause})");
            return Outcome::NotRecorded;
        }
        Err(ClaimError::Wait(_) | ClaimError::ClockBack) => return Outcome::Waiting,
    }

    let sent = {
        let _span = log::span(format!("device {vid:04X}:{pid:04X} send and decode"));
        door.send(&mut target)
    };
    let exchange = match sent {
        Ok(exchange) => exchange,
        Err(error) => {
            eprintln!("receiver {vid:04X}:{pid:04X}: {error:#}");
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
    if let (Outcome::Reading(reading, _), Some(path)) = (outcome, log_path) {
        let recorded = {
            let _span = log::span(format!("device {vid:04X}:{pid:04X} record reading"));
            QueryLog::update(path, |log| {
                log.record_reading(vid, pid, reading, read_at);
                Ok::<_, anyhow::Error>(())
            })
        };
        if let Err(error) = recorded {
            eprintln!(
                "receiver {vid:04X}:{pid:04X}: could not record the reading ({})",
                error.root_cause()
            );
        }
    }
    outcome
}

/// The menu text for an outcome; an unconfirmed device points at its menu action.
fn reason(outcome: Outcome) -> String {
    match outcome {
        Outcome::NoChannel => "no channel".to_owned(),
        Outcome::NeedsCapture => "unsupported".to_owned(),
        Outcome::Unconfirmed => "unchecked".to_owned(),
        Outcome::Waiting => "waiting".to_owned(),
        Outcome::TooSoon(left) => wait_text(left),
        Outcome::Busy => "busy (vendor app)".to_owned(),
        Outcome::Ambiguous => "2+ receivers".to_owned(),
        Outcome::NotRecorded => "log error".to_owned(),
        Outcome::BudgetUsed => "skipped".to_owned(),
        Outcome::NoReply => "no reply".to_owned(),
        // The detailed `why` is logged where the reply is decoded; the menu keeps two words.
        Outcome::Invalid(why) => if why == ON_CABLE_TEXT {
            why
        } else {
            "bad reply"
        }
        .to_owned(),
        Outcome::Reading(..) => ASLEEP_TEXT.to_owned(),
    }
}

/// The row id the menu and config use for a receiver or cable device.
fn row_id(device: &KnownDevice) -> String {
    format!("usb-{:04x}-{:04x}", device.vid, device.pid)
}

fn kind(family: Family) -> DeviceKind {
    match family {
        Family::Report13 => DeviceKind::Keyboard,
        Family::Compx | Family::AtkBitmouse => DeviceKind::Mouse,
        Family::Mchose | Family::AtkSerial => DeviceKind::Headset,
    }
}

/// A fresh reading is shown as is. Otherwise the stored reading keeps its percent and time,
/// dimmed with the reason when this attempt failed. Within the floor the previous attempt
/// decides: a query stamped after the stored reading got no valid reply, so it is dimmed too.
/// With none stored the reason is the level.
fn view(
    device: &KnownDevice,
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
        (_, Some((reading, read_at))) => (Some(reading), read_at, true),
        (_, None) => (None, now, false),
    };
    let row = DeviceView {
        id: row_id(device),
        name: device.name.to_owned(),
        kind: kind(device.family),
        path: if device.cable {
            Path::Cable
        } else {
            Path::Dongle
        },
        level: Level::NoData(reason(outcome)),
        charging: false,
        connected: true,
        updated_at,
        stale: false,
        note: None,
    };
    let Some(reading) = shown else {
        return row;
    };
    // An asleep reading whose last query went unanswered keeps both notes.
    let note = if failed {
        Some(match outcome {
            Outcome::Waiting if reading.asleep => {
                format!("{ASLEEP_TEXT}; {LAST_QUERY_FAILED_TEXT}")
            }
            Outcome::Waiting => LAST_QUERY_FAILED_TEXT.to_owned(),
            _ => reason(outcome),
        })
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
    const FAILURES: [Outcome; 9] = [
        Outcome::NoChannel,
        Outcome::NeedsCapture,
        Outcome::Unconfirmed,
        Outcome::Busy,
        Outcome::Ambiguous,
        Outcome::NotRecorded,
        Outcome::BudgetUsed,
        Outcome::NoReply,
        Outcome::Invalid(ON_CABLE_TEXT),
    ];

    #[test]
    fn gate_blocks_capture_first_then_unconfirmed() {
        let vgn = device(0x391D, 0x1A05);
        // A device that needs a capture stays closed even when the owner allows it.
        let needs_capture = KnownDevice {
            needs_capture: true,
            ..*vgn
        };
        let id = (needs_capture.vid, needs_capture.pid);
        assert_eq!(gate(&needs_capture, &[id]), Err(Outcome::NeedsCapture));
        assert_eq!(gate(&needs_capture, &[]), Err(Outcome::NeedsCapture));
        assert_eq!(gate(vgn, &[]), Err(Outcome::Unconfirmed));
        assert_eq!(gate(vgn, &[(0x3837, 0x6011)]), Err(Outcome::Unconfirmed));
        assert_eq!(gate(vgn, &[(0x391D, 0x1A05)]), Ok(()));
        assert_eq!(gate(mchose(), &[]), Ok(()));
        // X1 SE is confirmed in the table, so it passes with no allow entry.
        assert_eq!(gate(device(0x373B, 0x1216), &[]), Ok(()));
    }

    const VGN: (u16, u16) = (0x391D, 0x1A05);

    #[test]
    fn check_is_offered_only_for_unconfirmed_not_allowed_devices() {
        assert!(can_check("usb-391d-1a05", &[]));
        assert!(can_check("usb-391d-1a05", &[(0x3837, 0x6011)]));
        assert!(!can_check("usb-391d-1a05", &[VGN]));
        assert!(!can_check("usb-3837-6011", &[]));
        assert!(!can_check("usb-373b-1216", &[]));
        assert!(!can_check("bt:a4-c3-f0-11-22-33", &[]));
        assert!(!can_check("usb-046d-c52b", &[]));
        assert!(!can_check("usb-391D-1A05", &[]));
    }

    #[test]
    fn forced_check_opens_only_its_own_device() {
        let all: Vec<(u16, u16)> = KNOWN.iter().map(|d| (d.vid, d.pid)).collect();
        let (vgn, allow) = check_scope("usb-391d-1a05", &[]).unwrap();
        assert_eq!((vgn.vid, vgn.pid), VGN);
        let opened_unconfirmed: Vec<(u16, u16)> = openable(&all, &allow)
            .into_iter()
            .filter(|&(vid, pid)| !device(vid, pid).confirmed)
            .collect();
        assert_eq!(opened_unconfirmed, [VGN]);
        assert_eq!(
            gate(device(0x3554, 0xFA09), &allow),
            Err(Outcome::Unconfirmed)
        );
        assert_eq!(plan(gate(vgn, &allow), Ok(()), Ok(()), true), Step::Query);

        // Nothing else is relaxed: the floor, the pick and a missing capture still refuse.
        let wait = Err(ClaimError::Wait(Duration::from_secs(4)));
        assert_eq!(
            plan(gate(vgn, &allow), Ok(()), wait, true),
            Step::Show(Outcome::Waiting)
        );
        assert_eq!(
            plan(gate(vgn, &allow), Err(Outcome::Busy), Ok(()), true),
            Step::Show(Outcome::Busy)
        );
        let needs_capture = KnownDevice {
            needs_capture: true,
            ..*vgn
        };
        assert_eq!(gate(&needs_capture, &allow), Err(Outcome::NeedsCapture));

        assert!(check_scope("bt:a4-c3-f0-11-22-33", &[]).is_none());
        assert!(check_scope("usb-046d-c52b", &[]).is_none());
    }

    #[test]
    fn only_a_valid_reading_confirms_a_check() {
        let vgn = device(VGN.0, VGN.1);
        let good = checked(
            vgn,
            Outcome::Reading(reading(80, false, false), at(NOW)),
            None,
            None,
            at(NOW),
        );
        assert_eq!(good.confirm, Some(VGN));
        assert_eq!(good.row.level, Level::Percent(80));

        let failures = [
            (Outcome::NoReply, "no reply"),
            (Outcome::Busy, "busy (vendor app)"),
            (Outcome::TooSoon(Duration::from_millis(6_500)), "wait 7 s"),
            (Outcome::Invalid("unknown power state"), "bad reply"),
        ];
        for (outcome, text) in failures {
            let row = checked(vgn, outcome, None, None, at(NOW));
            assert_eq!(row.confirm, None, "{outcome:?}");
            assert_eq!(row.row.level, Level::NoData(text.into()), "{outcome:?}");

            // A stored reading keeps its percent and carries the reason as the note.
            let stored = Some((reading(61, false, false), at(NOW - 300)));
            let row = checked(vgn, outcome, stored, None, at(NOW));
            assert_eq!(row.confirm, None, "{outcome:?}");
            assert_eq!(row.row.note.as_deref(), Some(text), "{outcome:?}");
        }

        // A table-confirmed device needs no `allow` entry.
        let mchose_reading = Outcome::Reading(reading(50, false, false), at(NOW));
        assert_eq!(
            checked(mchose(), mchose_reading, None, None, at(NOW)).confirm,
            None
        );
    }

    #[test]
    fn failed_check_reason_survives_later_reads() {
        let vgn = device(VGN.0, VGN.1);
        let id = "usb-391d-1a05";
        let mut notes = CheckNotes::new();
        let check = |outcome| checked(vgn, outcome, None, None, at(NOW));
        let later_read = |notes: &CheckNotes| {
            let row = view(vgn, Outcome::Unconfirmed, None, None, at(NOW));
            with_check_note(row, Outcome::Unconfirmed, notes)
        };

        check(Outcome::NoReply).remember(&mut notes);
        let row = later_read(&notes);
        assert_eq!(row.level, Level::NoData("unchecked".into()));
        assert_eq!(row.note.as_deref(), Some("no reply"));

        // The latest failure wins; a floor wait sent nothing and keeps it.
        check(Outcome::Busy).remember(&mut notes);
        check(Outcome::TooSoon(Duration::from_secs(5))).remember(&mut notes);
        assert_eq!(
            later_read(&notes).note.as_deref(),
            Some("busy (vendor app)")
        );

        // A note only rides on a device the gate still holds back.
        let queried = view(vgn, Outcome::NoReply, None, None, at(NOW));
        assert_eq!(
            with_check_note(queried, Outcome::NoReply, &notes).note,
            None
        );

        // A valid reading clears it.
        check(Outcome::Reading(reading(80, false, false), at(NOW))).remember(&mut notes);
        assert!(!notes.contains_key(id));
        assert_eq!(later_read(&notes).note, None);
    }

    #[test]
    fn closed_gate_never_reaches_the_door() {
        for refused in [Outcome::Unconfirmed, Outcome::NeedsCapture] {
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
        assert_eq!(row.note.as_deref(), Some("asleep"));
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
        assert_eq!(asleep.note.as_deref(), Some("asleep"));

        let empty = view(mchose(), Outcome::Waiting, None, Some(at(NOW - 5)), at(NOW));
        assert_eq!(empty.level, Level::NoData("waiting".into()));
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
        assert_eq!(row.note.as_deref(), Some("no reply"));

        // An asleep stored reading keeps its own note next to the unanswered-query one.
        let asleep = view(
            mchose(),
            Outcome::Waiting,
            Some((reading(70, true, true), at(NOW - 600))),
            Some(at(NOW - 120)),
            at(NOW),
        );
        assert!(asleep.stale);
        assert_eq!(asleep.note.as_deref(), Some("asleep; no reply"));

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
        assert_eq!(empty.level, Level::NoData("waiting".into()));
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
            assert_eq!(row.note, Some(reason(outcome)), "{outcome:?}");
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
    fn reason_texts() {
        let text = reason;
        assert_eq!(text(Outcome::Unconfirmed), "unchecked");
        assert_eq!(text(Outcome::TooSoon(Duration::from_secs(4))), "wait 4 s");
        assert_eq!(text(Outcome::NoChannel), "no channel");
        assert_eq!(text(Outcome::NeedsCapture), "unsupported");
        assert_eq!(text(Outcome::Busy), "busy (vendor app)");
        assert_eq!(text(Outcome::NoReply), "no reply");
        assert_eq!(text(Outcome::BudgetUsed), "skipped");
        assert_eq!(text(Outcome::Invalid("unknown power state")), "bad reply");
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
