//! The Add window's device work, run by the refresh worker (the one door owner): reading a
//! device's interfaces once its interface count is stable, and the single try.

use std::thread;
use std::time::SystemTime;

use door::{Door, Exchange, Presence, STABLE_GAP, ShapeCache, first_battery_query};
use protocol::{Decoded, Family, Link, families_for};

use super::flow::{Inspection, Interfaces, Tried, TryOutcome};
use crate::hid_caps;
use crate::log;
use crate::plug::Id;
use crate::query::{
    ClaimError, claim_query, floor_check, open_hid_target, open_serial_target, target_why,
};
use crate::query_log::{self, QueryLog};
use crate::scan::{self, ExchangeStyle, bytes_line_capped, exchange_lines};
use crate::vendor_apps;

/// The trace line of a try refused before any write.
const NOTHING_SENT: &str = "Sent: nothing";
/// The trace line of an exchange that read no reply.
const NO_REPLY: &str = "Reply: none";
/// Bytes shown per report in the trace: every family's header and level byte fit.
const TRACE_BYTES: usize = 16;
/// Reports read shown per try, the reply line included; sent frames and BITMOUSE type lines
/// come on top.
const TRACE_REPORTS: usize = 5;
/// The report trace stays short enough for an issue URL; the reply line always shows, so the
/// other reports get the remaining places.
const TRACE_STYLE: ExchangeStyle = ExchangeStyle {
    capital: true,
    max_bytes: TRACE_BYTES,
    max_others: TRACE_REPORTS - 1,
};

/// The device's families from its interface shapes, read only once two listings `STABLE_GAP`
/// apart show the same counts; the shapes are cached until the device leaves.
pub fn inspect((vid, pid): Id, cache: &mut ShapeCache) -> Inspection {
    let _span = log::span_fmt(format_args!("add device inspect {vid:04X}:{pid:04X}"));
    let listing = door::list();
    let before = listing.present_all();
    let product = listing.product(vid, pid).map(str::to_owned);
    cache.retain_present(&before);
    let Some(counts) = presence(&before, (vid, pid)) else {
        return Inspection::Gone;
    };
    // Both paths read this device's descriptors once (the open rule applies) for the report's
    // scan listing; cached shapes still decide the families. The caps read there is a
    // zero-access open outside that rule.
    let (shapes, inventory) = match cache.get(vid, pid) {
        Some(shapes) => (shapes.to_vec(), listing.into_inventory(&[(vid, pid)])),
        None => {
            drop(listing);
            thread::sleep(STABLE_GAP);
            let listing = door::list();
            let after = listing.present_all();
            match presence(&after, (vid, pid)) {
                None => return Inspection::Gone,
                Some(now) if now != counts => return Inspection::Settling,
                Some(_) => {}
            }
            // Opens only this device's collections the open rule allows, for their descriptors.
            let inventory = listing.into_inventory(&[(vid, pid)]);
            let shapes = inventory.shapes(vid, pid);
            // No shapes because HID could not be listed is a failure, not "no protocol".
            if shapes.is_empty()
                && let Some(error) = inventory.errors.first()
            {
                return Inspection::Failed(error.clone());
            }
            cache.insert(vid, pid, shapes.clone(), &before, &after);
            (shapes, inventory)
        }
    };
    let scan_lines =
        scan::device_lines(vid, pid, &inventory.hid, &inventory.serial, hid_caps::line);
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
    // For the report: brand apps that can hold the device, and the brand's other plugged ids.
    let vendor_apps = vendor_apps::running(&[vid]);
    let siblings = before
        .iter()
        .filter(|other| other.vid == vid && other.pid != pid)
        .map(|other| (other.vid, other.pid))
        .collect();
    Inspection::Ready(Interfaces {
        families,
        product,
        shapes,
        serial,
        scan_lines,
        vendor_apps,
        siblings,
    })
}

/// One `Door::send_as` query as `family` (off-catalogue BITMOUSE starts as a receiver), stamped
/// before the write; only the door's BITMOUSE fallback re-sends. A valid reading is stored.
pub fn try_as((vid, pid): Id, family: Family) -> Tried {
    let _span = log::span_fmt(format_args!(
        "add device try {vid:04X}:{pid:04X} as {}",
        family.name()
    ));
    let listing = door::list();
    if listing.present(&[(vid, pid)]).is_empty() {
        return TryOutcome::Gone.into();
    }
    let inventory = listing.into_inventory(&[(vid, pid)]);
    let opened = match family.link() {
        Link::Hid => open_hid_target(&inventory, family, vid, pid),
        Link::Serial => open_serial_target(&inventory, vid, pid),
    };
    let (mut target, target_line) = match opened {
        Ok(opened) => opened,
        Err(error) => return nothing_sent(TryOutcome::Failed(target_why(family, error)), None),
    };
    let target_line = target_text(&target_line);
    let Some(log_path) = query_log::path() else {
        let why = "no data folder to record the query time".to_owned();
        return nothing_sent(TryOutcome::Failed(why), Some(target_line));
    };
    match claim_query(&log_path, family, vid, pid, SystemTime::now()) {
        Ok(()) => {}
        Err(ClaimError::Wait(wait)) => return TryOutcome::Waiting(wait).into(),
        // The stamp was replaced by now, so the floor starts over.
        Err(ClaimError::ClockBack) => return TryOutcome::Waiting(family.query_floor()).into(),
        Err(ClaimError::Record(cause)) => {
            let why = format!("could not record the query time ({cause})");
            return nothing_sent(TryOutcome::Failed(why), Some(target_line));
        }
    }
    let mut door = Door::new(1);
    let exchange = match door.send_as(&mut target, family, false) {
        Ok(exchange) => exchange,
        Err(error) => {
            log::line(&format!(
                "add device {vid:04X}:{pid:04X}: send failed: {error:#}"
            ));
            drop(target);
            if door::present(&[(vid, pid)]).is_empty() {
                return TryOutcome::Gone.into();
            }
            // The door spends its budget right before the write, so a spent budget means the
            // query went out (or may have); an unspent one means a refusal before any I/O.
            if door.writes_left() > 0 {
                return nothing_sent(TryOutcome::Failed(format!("{error:#}")), Some(target_line));
            }
            // The first frame written is certain; what followed is in the error.
            let frame = first_battery_query(family, vid, pid, false);
            let first = bytes_line_capped("Sent", frame, TRACE_BYTES);
            return Tried {
                outcome: TryOutcome::Interrupted(format!("{error:#}")),
                connection: None,
                trace: vec![target_line, first],
            };
        }
    };
    let outcome = match &exchange.reply {
        None => TryOutcome::NoReply,
        Some((_, Decoded::Reading(reading))) => {
            record(&log_path, (vid, pid), *reading);
            TryOutcome::Reading(*reading)
        }
        Some((_, Decoded::Invalid(why))) => TryOutcome::Invalid((*why).to_owned()),
        // The door never keeps a non-reply, and only the online query yields `Online`.
        Some((_, Decoded::NotTheReply | Decoded::Online(_))) => {
            TryOutcome::Invalid("not the reply".to_owned())
        }
    };
    log::trace(format_args!(
        "add device {vid:04X}:{pid:04X} tried: {outcome:?}"
    ));
    Tried {
        outcome,
        connection: exchange.connection,
        trace: trace(target_line, &exchange),
    }
}

/// A try refused before any write: the target line when one was opened, then `Sent: nothing`.
fn nothing_sent(outcome: TryOutcome, target_line: Option<String>) -> Tried {
    let mut trace: Vec<String> = target_line.into_iter().collect();
    trace.push(NOTHING_SENT.to_owned());
    Tried {
        outcome,
        connection: None,
        trace,
    }
}

/// The redacted target line `open_hid_target` / `open_serial_target` builds, as a report line.
fn target_text(line: &str) -> String {
    format!("Target: {}", line.strip_prefix("target ").unwrap_or(line))
}

/// The report's trace of one exchange, in the probe's byte format: target, the exchange's byte
/// lines, then the reply's absence when none came.
fn trace(target_line: String, exchange: &Exchange) -> Vec<String> {
    let mut lines = vec![target_line];
    lines.extend(exchange_lines(exchange, TRACE_STYLE));
    // A BITMOUSE fallback waits in several windows, so no single wait time is named.
    if exchange.reply.is_none() {
        lines.push(NO_REPLY.to_owned());
    }
    lines
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

#[cfg(test)]
mod tests {
    use door::{Connection, Fallback, Source};
    use protocol::{battery_query_for, decode};

    use super::*;

    fn padded(head: &[u8], len: usize) -> Vec<u8> {
        let mut bytes = head.to_vec();
        bytes.resize(len, 0);
        bytes
    }

    /// A BITMOUSE exchange whose last frame is the cable one; `reply` is a 127-byte report.
    fn bitmouse(reply: Option<Vec<u8>>, fallback: Option<Fallback>) -> Exchange {
        let family = Family::AtkBitmouse;
        Exchange {
            family,
            sent: battery_query_for(family, true),
            reply: reply.map(|bytes| {
                let decoded = decode(family, &bytes);
                (bytes, decoded)
            }),
            unrelated: Vec::new(),
            dropped: 0,
            connection: Some(Connection {
                cable: true,
                source: Source::Catalogue,
            }),
            fallback,
        }
    }

    /// The captured 373B:1299 answers: type wired1K, battery 60 %.
    fn captured() -> (Vec<u8>, Vec<u8>) {
        let type_reply = padded(&[0x08, 0x72, 0x00, 0x3B, 0x00, 0x1A, 0x01, 0x02], 127);
        let battery = padded(
            &[0x08, 0x72, 0x00, 0x3B, 0x00, 0x07, 0x01, 0x3C, 0x0C, 0x0F],
            127,
        );
        (type_reply, battery)
    }

    #[test]
    fn trace_of_a_cable_row_answering_the_first_frame() {
        let (_, battery) = captured();
        let exchange = bitmouse(Some(battery), None);
        let target = target_text("target HID if 1 FF05:0001 · path P");
        assert_eq!(
            trace(target, &exchange),
            [
                "Target: HID if 1 FF05:0001 · path P",
                "Sent (16 of 64 B): 08 7c 72 02 00 00 07 01 00 00 00 00 00 00 00 00",
                "Reply (16 of 127 B): 08 72 00 3b 00 07 01 3c 0c 0f 00 00 00 00 00 00",
            ]
        );
    }

    #[test]
    fn trace_of_the_type_fallback_and_re_send() {
        let (type_reply, battery) = captured();
        let fallback = Fallback {
            silent: battery_query_for(Family::AtkBitmouse, false),
            type_reply: Some(type_reply),
            error: None,
        };
        let exchange = Exchange {
            connection: Some(Connection {
                cable: true,
                source: Source::Detected { name: "wired1K" },
            }),
            ..bitmouse(Some(battery), Some(fallback))
        };
        assert_eq!(
            trace("Target: T".to_owned(), &exchange),
            [
                "Target: T",
                "Sent (16 of 64 B): 08 7d 72 02 00 01 07 01 00 00 00 00 00 00 00 00",
                "Type sent (16 of 64 B): 08 8f 72 02 00 00 1a 01 00 00 00 00 00 00 00 00",
                "Type reply (16 of 127 B): 08 72 00 3b 00 1a 01 02 00 00 00 00 00 00 00 00",
                "Device type: wired1K (cable)",
                "Sent (16 of 64 B): 08 7c 72 02 00 00 07 01 00 00 00 00 00 00 00 00",
                "Reply (16 of 127 B): 08 72 00 3b 00 07 01 3c 0c 0f 00 00 00 00 00 00",
            ]
        );
    }

    #[test]
    fn trace_of_no_reply_caps_the_other_reports() {
        let other = padded(&[0x08, 0x73], 127);
        let fallback = Fallback {
            silent: battery_query_for(Family::AtkBitmouse, true),
            type_reply: None,
            error: None,
        };
        let exchange = Exchange {
            unrelated: vec![other; 6],
            dropped: 2,
            ..bitmouse(None, Some(fallback))
        };
        let lines = trace("Target: T".to_owned(), &exchange);
        assert!(
            lines[1].starts_with("Sent (16 of 64 B): 08 7c"),
            "{lines:?}"
        );
        assert!(lines[2].starts_with("Type sent "), "{lines:?}");
        assert_eq!(lines[3], "Device type: no reply");
        let others = lines
            .iter()
            .filter(|line| line.starts_with("Other (16 of 127 B): 08 73"));
        assert_eq!(others.count(), TRACE_REPORTS - 1);
        assert!(lines.contains(&"+4 more not shown".to_owned()), "{lines:?}");
        assert_eq!(lines.last().unwrap(), NO_REPLY);
        // The silent frame is the last one sent, so it shows once.
        assert_eq!(lines.iter().filter(|l| l.starts_with("Sent ")).count(), 1);
    }

    #[test]
    fn trace_of_another_family_has_no_type_lines() {
        let compx = protocol::battery_query(Family::Compx);
        let exchange = Exchange {
            family: Family::Compx,
            sent: compx,
            connection: None,
            ..bitmouse(None, None)
        };
        let lines = trace("T".to_owned(), &exchange);
        assert!(lines[1].starts_with("Sent (16 of 17 B): "), "{lines:?}");
        let has_type = |line: &String| line.to_lowercase().contains("type");
        assert!(!lines.iter().any(has_type), "{lines:?}");
    }
}
