//! The Add window's device work, run by the refresh worker (the one door owner): reading a
//! device's interfaces once its interface count is stable, and the single try.

use std::thread;
use std::time::SystemTime;

use door::{Door, Presence, STABLE_GAP, ShapeCache};
use protocol::{Decoded, Family, Link, families_for};

use super::flow::{Inspection, Interfaces, TryOutcome};
use crate::log;
use crate::plug::Id;
use crate::query::{
    ClaimError, claim_query, floor_check, open_hid_target, open_serial_target, target_why,
};
use crate::query_log::{self, QueryLog};

/// The device's families from its interface shapes, read only once two listings `STABLE_GAP`
/// apart show the same counts; the shapes are cached until the device leaves.
pub fn inspect((vid, pid): Id, cache: &mut ShapeCache) -> Inspection {
    let _span = log::span_fmt(format_args!("add device inspect {vid:04X}:{pid:04X}"));
    let listing = door::list();
    let before = listing.present_all();
    let product = listing.product(vid, pid).map(str::to_owned);
    drop(listing);
    cache.retain_present(&before);
    let Some(counts) = presence(&before, (vid, pid)) else {
        return Inspection::Gone;
    };
    let shapes = match cache.get(vid, pid) {
        Some(shapes) => shapes.to_vec(),
        None => {
            thread::sleep(STABLE_GAP);
            let listing = door::list();
            let after = listing.present_all();
            match presence(&after, (vid, pid)) {
                None => return Inspection::Gone,
                Some(now) if now != counts => return Inspection::Settling,
                Some(_) => {}
            }
            // Opens only this device's vendor collections, for their descriptors.
            let inventory = listing.into_inventory(&[(vid, pid)]);
            let shapes = inventory.shapes(vid, pid);
            // No shapes because HID could not be listed is a failure, not "no protocol".
            if shapes.is_empty()
                && let Some(error) = inventory.errors.first()
            {
                return Inspection::Failed(error.clone());
            }
            cache.insert(vid, pid, shapes.clone(), &before, &after);
            shapes
        }
    };
    let serial = counts.serial > 0;
    let log = query_log::path()
        .map(|path| QueryLog::load_from(&path))
        .unwrap_or_default();
    let now = SystemTime::now();
    let families = families_for(vid, pid, &shapes, serial)
        .into_iter()
        .map(|family| {
            let wait = match floor_check(&log, family, vid, pid, now) {
                Err(ClaimError::Wait(wait)) => Some(wait),
                _ => None,
            };
            (family, wait)
        })
        .collect();
    Inspection::Ready(Interfaces {
        families,
        product,
        shapes,
        serial,
    })
}

/// One battery query as `family`: the time is stamped before the write, nothing is retried, and
/// a valid reading is stored like a refresh's. Every refusal of `Door::send_as` applies.
pub fn try_as((vid, pid): Id, family: Family) -> TryOutcome {
    let _span = log::span_fmt(format_args!(
        "add device try {vid:04X}:{pid:04X} as {}",
        family.name()
    ));
    let listing = door::list();
    if listing.present(&[(vid, pid)]).is_empty() {
        return TryOutcome::Gone;
    }
    let inventory = listing.into_inventory(&[(vid, pid)]);
    let opened = match family.link() {
        Link::Hid => open_hid_target(&inventory, family, vid, pid),
        Link::Serial => open_serial_target(&inventory, vid, pid),
    };
    let mut target = match opened {
        Ok((target, _)) => target,
        Err(error) => return TryOutcome::Failed(target_why(family, error)),
    };
    let Some(log_path) = query_log::path() else {
        return TryOutcome::Failed("no data folder to record the query time".to_owned());
    };
    match claim_query(&log_path, family, vid, pid, SystemTime::now()) {
        Ok(()) => {}
        Err(ClaimError::Wait(wait)) => return TryOutcome::Waiting(wait),
        // The stamp was replaced by now, so the floor starts over.
        Err(ClaimError::ClockBack) => return TryOutcome::Waiting(family.query_floor()),
        Err(ClaimError::Record(cause)) => {
            return TryOutcome::Failed(format!("could not record the query time ({cause})"));
        }
    }
    let mut door = Door::new(1);
    let exchange = match door.send_as(&mut target, family) {
        Ok(exchange) => exchange,
        Err(error) => {
            log::line(&format!(
                "add device {vid:04X}:{pid:04X}: send failed: {error:#}"
            ));
            drop(target);
            if door::present(&[(vid, pid)]).is_empty() {
                return TryOutcome::Gone;
            }
            // The door spends its budget right before the write, so a spent budget means the
            // query went out (or may have); an unspent one means a refusal before any I/O.
            return if door.writes_left() == 0 {
                TryOutcome::Interrupted(format!("{error:#}"))
            } else {
                TryOutcome::Failed(format!("{error:#}"))
            };
        }
    };
    let outcome = match exchange.reply {
        None => TryOutcome::NoReply,
        Some((_, Decoded::Reading(reading))) => {
            record(&log_path, (vid, pid), reading);
            TryOutcome::Reading(reading)
        }
        Some((_, Decoded::Invalid(why))) => TryOutcome::Invalid(why.to_owned()),
        // The door never keeps a non-reply, and only the online query yields `Online`.
        Some((_, Decoded::NotTheReply | Decoded::Online(_))) => {
            TryOutcome::Invalid("not the reply".to_owned())
        }
    };
    log::trace(format_args!(
        "add device {vid:04X}:{pid:04X} tried: {outcome:?}"
    ));
    outcome
}

fn presence(all: &[Presence], id: Id) -> Option<Presence> {
    all.iter().find(|p| (p.vid, p.pid) == id).copied()
}

/// Stores the reading, so the first refresh after [Add] shows it instead of waiting out the floor.
fn record(path: &std::path::Path, (vid, pid): Id, reading: protocol::Reading) {
    let saved = QueryLog::update(path, |log| {
        log.record_reading(vid, pid, reading, SystemTime::now());
        Ok::<_, anyhow::Error>(())
    });
    if let Err(error) = saved {
        log::line(&format!(
            "add device: record failed: {}",
            error.root_cause()
        ));
    }
}
