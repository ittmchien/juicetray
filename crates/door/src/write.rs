//! The one write door: a write budget, the family's fixed battery query (plus the Compx and
//! ATK serial online-status query), one write (or one SetFeature + GetFeature pair for a
//! feature-report family) per query and a bounded wait for each reply. A BITMOUSE query is the
//! battery frame for the known connection (the catalogue row's, else the caller's), then on
//! silence (unless the row is verified) the read-only type query and at most one re-send for the
//! detected connection, all on one budget unit. No API here takes raw bytes. `send` takes the
//! family from the catalogue; `send_as` takes it from the caller and checks it against what the
//! device exposes.

use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use protocol::{
    Decoded, DeviceType, Family, Link, SERIAL_FRAME_LEN, battery_query_for, decode, decode_online,
    decode_type, is_recovery, is_type_error, known, online_query, pick_target, type_query,
};

use crate::transport::Target;

/// Reports that are not the reply kept per query; later ones are only counted.
pub const MAX_KEPT_UNRELATED: usize = 16;

/// One HID read gets a buffer this large.
pub(crate) const HID_READ_LEN: usize = 256;
const MIN_READ_TIMEOUT: Duration = Duration::from_millis(1);

/// What one query sent and got back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    pub family: Family,
    /// The last battery (or online) frame written; `reply` answers it.
    pub sent: &'static [u8],
    /// The reply and its decoding (never `NotTheReply`); `None` = no reply before the deadline.
    pub reply: Option<(Vec<u8>, Decoded)>,
    pub unrelated: Vec<Vec<u8>>,
    pub dropped: usize,
    /// BITMOUSE only: the connection `sent` was chosen for; `None` for every other family.
    pub connection: Option<Connection>,
    /// BITMOUSE only: the type-query fallback, when the first battery frame got no reply.
    pub fallback: Option<Fallback>,
}

/// The connection a BITMOUSE battery frame is sent for, and where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Connection {
    /// The mouse on its USB cable (target 0), not a receiver (target 1).
    pub cable: bool,
    pub source: Source,
}

/// Where a BITMOUSE connection came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The device's type answer, with its ATK HUB type name (`wired1K`…).
    Detected { name: &'static str },
    /// The catalogue row.
    Catalogue,
    /// The caller's `cable` (an off-catalogue id).
    Caller,
}

/// The type query sent after a silent battery frame. `sent` differs from `silent` only when the
/// detected connection differs, so the battery frame was sent once more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fallback {
    /// The battery frame that got no reply.
    pub silent: &'static [u8],
    /// The type answer as read; `None` = no answer (or an unknown type code).
    pub type_reply: Option<Vec<u8>>,
    /// The fallback's write or read error; the exchange still stands, without a reply.
    pub error: Option<String>,
}

/// The two frames the door can send; each decides how its reply is decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Query {
    Battery,
    Online,
}

impl Exchange {
    /// Keeps `bytes` as the reply, or as an unrelated report; true once the reply is in.
    /// An echo of the request decodes as `Invalid`, so it is filed as unrelated to keep the wait
    /// open.
    fn take(&mut self, query: Query, bytes: Vec<u8>) -> bool {
        let decoded = match query {
            _ if bytes == self.sent => Decoded::NotTheReply,
            Query::Battery => decode(self.family, &bytes),
            Query::Online => decode_online(self.family, &bytes),
        };
        match decoded {
            Decoded::NotTheReply => {
                self.keep_unrelated(bytes);
                false
            }
            decoded => {
                self.reply = Some((bytes, decoded));
                true
            }
        }
    }

    /// Keeps a report that answers nothing we wait for, up to `MAX_KEPT_UNRELATED`; later ones
    /// are only counted.
    fn keep_unrelated(&mut self, bytes: Vec<u8>) {
        if self.unrelated.len() < MAX_KEPT_UNRELATED {
            self.unrelated.push(bytes);
        } else {
            self.dropped += 1;
        }
    }
}

/// Owns the write budget; every device write goes through `send`.
pub struct Door {
    writes_left: u8,
}

impl Door {
    pub fn new(budget: u8) -> Door {
        Door {
            writes_left: budget,
        }
    }

    /// Queries this door may still send; callers check it before claiming a query time.
    pub fn writes_left(&self) -> u8 {
        self.writes_left
    }

    /// Sends the battery query of the target's family once and waits for the reply.
    pub fn send(&mut self, target: &mut Target) -> anyhow::Result<Exchange> {
        self.send_query(target, Query::Battery)
    }

    /// Sends the online-status query (Compx, ATK serial) once and waits for the reply; every
    /// other family is refused before any I/O.
    pub fn send_online(&mut self, target: &mut Target) -> anyhow::Result<Exchange> {
        self.send_query(target, Query::Online)
    }

    /// Sends `family`'s battery query once after the `check_as` refusals; `cable` (wired, not a
    /// receiver) picks the first BITMOUSE frame of an off-catalogue id.
    pub fn send_as(
        &mut self,
        target: &mut Target,
        family: Family,
        cable: bool,
    ) -> anyhow::Result<Exchange> {
        check_as(target, family)?;
        let deadline = Instant::now() + family.reply_timeout();
        self.exchange_until(target, family, cable, Query::Battery, deadline)
    }

    // A catalogue target: its row's connection picks the frame, so the `cable` here is unused.
    fn send_query(&mut self, target: &mut Target, query: Query) -> anyhow::Result<Exchange> {
        let family = catalogue_family(target)?;
        let deadline = Instant::now() + family.reply_timeout();
        self.exchange_until(target, family, false, query, deadline)
    }

    /// `send` with an explicit deadline for the battery wait; a BITMOUSE fallback still gets
    /// its own reply windows.
    #[cfg(test)]
    pub(crate) fn send_until(
        &mut self,
        target: &mut Target,
        deadline: Instant,
    ) -> anyhow::Result<Exchange> {
        let family = catalogue_family(target)?;
        self.exchange_until(target, family, false, Query::Battery, deadline)
    }

    /// Refusals happen before any I/O; the budget drops before the write, so a failed write is
    /// never retried.
    fn exchange_until(
        &mut self,
        target: &mut Target,
        family: Family,
        cable: bool,
        query: Query,
        deadline: Instant,
    ) -> anyhow::Result<Exchange> {
        let (sent, connection) = match query {
            Query::Battery => {
                let connection = first_connection(family, target.vid, target.pid, cable);
                (
                    battery_query_for(family, connection.is_some_and(|c| c.cable)),
                    connection,
                )
            }
            Query::Online => {
                let frame = online_query(family)
                    .ok_or_else(|| anyhow!("this family has no online query; nothing was sent"))?;
                (frame, None)
            }
        };
        if self.writes_left == 0 {
            bail!("write budget used up; nothing was sent");
        }

        self.writes_left -= 1;
        let mut exchange = Exchange {
            family,
            sent,
            reply: None,
            unrelated: Vec::new(),
            dropped: 0,
            connection,
            fallback: None,
        };
        // A feature family's query is one SetFeature + GetFeature pair, never an output report.
        if family.uses_feature_report() {
            let mut reply = vec![0u8; sent.len()];
            let len = target
                .transport
                .feature_exchange(sent, family.reply_timeout(), &mut reply)
                .context("feature exchange failed (the query may have been sent)")?;
            reply.truncate(len);
            if !reply.is_empty() {
                exchange.take(query, reply);
            }
            return Ok(exchange);
        }
        target.transport.write(sent).context("write failed")?;
        await_reply(target, query, deadline, &mut exchange)
            .context("reading the reply failed (the query was sent)")?;
        // Only a silent BITMOUSE battery frame gets the fallback; a verified row's connection is
        // proven on hardware, so its silence means the mouse is off and costs no more writes.
        let proven = known(target.vid, target.pid).is_some_and(|device| device.verified);
        if exchange.reply.is_none()
            && !proven
            && let Some(used) = connection
            && let Err(error) = type_fallback(target, used, &mut exchange)
        {
            // Best effort: the battery query already went out and stayed silent, so a fallback
            // I/O error is kept on the exchange instead of failing it.
            let fallback = exchange.fallback.get_or_insert(Fallback {
                silent: sent,
                type_reply: None,
                error: None,
            });
            fallback.error = Some(format!("{error:#}"));
        }
        Ok(exchange)
    }
}

/// The connection a BITMOUSE battery query starts with: the catalogue row's, else the caller's
/// `cable`; `None` for a family without a type query, whose frame never depends on it.
fn first_connection(family: Family, vid: u16, pid: u16, cable: bool) -> Option<Connection> {
    type_query(family)?;
    Some(match known(vid, pid) {
        Some(device) => Connection {
            cable: device.cable,
            source: Source::Catalogue,
        },
        None => Connection {
            cable,
            source: Source::Caller,
        },
    })
}

/// The battery frame the door writes first to `vid:pid` as `family`, `cable` as in `send_as`;
/// it is certain to have gone out once a write was attempted.
pub fn first_battery_query(family: Family, vid: u16, pid: u16, cable: bool) -> &'static [u8] {
    let connection = first_connection(family, vid, pid, cable);
    battery_query_for(family, connection.is_some_and(|c| c.cable))
}

/// After a silent BITMOUSE battery frame sent for `used`: the read-only type query, then one
/// re-send for the detected connection when it differs. Each wait gets its own reply window.
fn type_fallback(
    target: &mut Target,
    used: Connection,
    exchange: &mut Exchange,
) -> anyhow::Result<()> {
    let family = exchange.family;
    let Some(type_frame) = type_query(family) else {
        return Ok(());
    };
    let window = || Instant::now() + family.reply_timeout();
    target
        .transport
        .write(type_frame)
        .context("type query write failed")?;
    let (type_reply, detected) = await_type(target, window(), exchange)
        .context("reading the type reply failed (the type query was sent)")?
        .unzip();
    exchange.fallback = Some(Fallback {
        silent: exchange.sent,
        type_reply,
        error: None,
    });
    // No type answer, or a late battery reply ended the wait: nothing to re-send.
    let Some(detected) = detected else {
        return Ok(());
    };
    exchange.connection = Some(Connection {
        cable: detected.cable,
        source: Source::Detected {
            name: detected.name,
        },
    });
    if detected.cable == used.cable {
        return Ok(());
    }
    exchange.sent = battery_query_for(family, detected.cable);
    target
        .transport
        .write(exchange.sent)
        .context("battery re-send write failed")?;
    // A late type error (it carries the type command) is not the battery reply: keep waiting.
    read_until(target, window(), |bytes| {
        if is_type_error(family, &bytes) {
            exchange.keep_unrelated(bytes);
            return None;
        }
        exchange.take(Query::Battery, bytes).then_some(())
    })
    .context("reading the re-sent reply failed (the query was sent)")?;
    Ok(())
}

/// Reads until the type answer, a late battery reading (kept as the reply, ends the wait with
/// `None`) or `deadline`; every other report is kept as unrelated.
fn await_type(
    target: &mut Target,
    deadline: Instant,
    exchange: &mut Exchange,
) -> anyhow::Result<Option<(Vec<u8>, DeviceType)>> {
    let family = exchange.family;
    let found = read_until(target, deadline, |bytes| {
        if let Some(device_type) = decode_type(family, &bytes) {
            return Some(Some((bytes, device_type)));
        }
        // Only a reading counts: an error frame may lack its command, so it may be the type answer.
        match decode(family, &bytes) {
            reading @ Decoded::Reading(_) => {
                exchange.reply = Some((bytes, reading));
                Some(None)
            }
            _ => {
                exchange.keep_unrelated(bytes);
                None
            }
        }
    })?;
    Ok(found.flatten())
}

/// The catalogue family of the target, refused when unknown or opened over the other link.
fn catalogue_family(target: &Target) -> anyhow::Result<Family> {
    let (vid, pid) = (target.vid, target.pid);
    let family = known(vid, pid)
        .ok_or_else(|| anyhow!("unknown device {vid:04X}:{pid:04X}; nothing was sent"))?
        .family;
    check_link(target, family)?;
    Ok(family)
}

fn check_link(target: &Target, family: Family) -> anyhow::Result<()> {
    if family.link() != target.link {
        let (vid, pid) = (target.vid, target.pid);
        bail!("link mismatch: {vid:04X}:{pid:04X} is not queried over this link; nothing was sent");
    }
    Ok(())
}

/// The `send_as` refusals; none of them touches the device.
fn check_as(target: &Target, family: Family) -> anyhow::Result<()> {
    let (vid, pid) = (target.vid, target.pid);
    let name = family.name();
    if is_recovery(vid, pid) {
        bail!("{vid:04X}:{pid:04X} is a recovery id; nothing was sent");
    }
    // A Generic spec is one catalogue row's own frame; no other id is ever sent it.
    if matches!(family, Family::Generic(_)) && known(vid, pid).map(|d| d.family) != Some(family) {
        bail!("{vid:04X}:{pid:04X} has no catalogue Generic row; nothing was sent");
    }
    if !target.families.contains(&family) {
        bail!("{vid:04X}:{pid:04X} does not expose {name}; nothing was sent");
    }
    check_link(target, family)?;
    match family.link() {
        Link::Hid => {
            let picked = pick_target(family, &target.shapes)
                .ok()
                .map(|index| &target.shapes[index]);
            if picked != target.opened.as_ref() {
                bail!(
                    "the opened collection of {vid:04X}:{pid:04X} is not the {name} target; nothing was sent"
                );
            }
        }
        Link::Serial => {
            if known(vid, pid).is_none() {
                bail!("serial port {vid:04X}:{pid:04X} is not in the catalogue; nothing was sent");
            }
        }
    }
    Ok(())
}

/// Time left until `deadline`, at least 1 ms; `None` once it has passed.
pub(crate) fn time_left(deadline: Instant) -> Option<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    (!left.is_zero()).then(|| left.max(MIN_READ_TIMEOUT))
}

/// Reads until the reply, the deadline or a 0-byte read.
fn await_reply(
    target: &mut Target,
    query: Query,
    deadline: Instant,
    exchange: &mut Exchange,
) -> anyhow::Result<()> {
    read_until(target, deadline, |bytes| {
        exchange.take(query, bytes).then_some(())
    })?;
    Ok(())
}

/// Offers each unit read to `found` until it returns `Some`, the deadline or a 0-byte read. A
/// HID unit is one read; a serial unit gathers chunks into one 64-byte frame, and a partial
/// frame at the end is still offered.
fn read_until<T>(
    target: &mut Target,
    deadline: Instant,
    mut found: impl FnMut(Vec<u8>) -> Option<T>,
) -> anyhow::Result<Option<T>> {
    let mut buf = [0u8; HID_READ_LEN];
    let mut pending = Vec::new();
    while let Some(timeout) = time_left(deadline) {
        let want = match target.link {
            Link::Hid => HID_READ_LEN,
            Link::Serial => SERIAL_FRAME_LEN - pending.len(),
        };
        let len = target.transport.read(&mut buf[..want], timeout)?;
        if len == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..len]);
        if target.link == Link::Serial && pending.len() < SERIAL_FRAME_LEN {
            continue;
        }
        if let Some(value) = found(std::mem::take(&mut pending)) {
            return Ok(Some(value));
        }
    }
    Ok(if pending.is_empty() {
        None
    } else {
        found(pending)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Transport;
    use protocol::{
        GenericReport, GenericSpec, HidShape, Reading, ReportKind, ReportSize, battery_query,
    };
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// Everything the fake saw: each write and the timeout of each read call.
    #[derive(Default)]
    struct Log {
        writes: Vec<Vec<u8>>,
        timeouts: Vec<Duration>,
        /// Each feature exchange: the SetFeature bytes and the settle pause.
        features: Vec<(Vec<u8>, Duration)>,
    }

    enum Step {
        Bytes(Vec<u8>),
        Fail,
    }

    /// Plays back scripted reads; an empty script reads 0 bytes.
    struct Fake {
        log: Rc<RefCell<Log>>,
        reads: VecDeque<Step>,
        /// The index of the write that fails (after it is logged).
        fail_write_at: Option<usize>,
    }

    impl Transport for Fake {
        fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
            let mut log = self.log.borrow_mut();
            let index = log.writes.len();
            log.writes.push(bytes.to_vec());
            if self.fail_write_at == Some(index) {
                bail!("fake write error");
            }
            Ok(())
        }

        fn read(&mut self, buf: &mut [u8], timeout: Duration) -> anyhow::Result<usize> {
            self.log.borrow_mut().timeouts.push(timeout);
            match self.reads.pop_front() {
                None => Ok(0),
                Some(Step::Fail) => bail!("fake read error"),
                Some(Step::Bytes(bytes)) => {
                    assert!(bytes.len() <= buf.len(), "scripted read exceeds the buffer");
                    buf[..bytes.len()].copy_from_slice(&bytes);
                    Ok(bytes.len())
                }
            }
        }

        /// Logs the SetFeature, then plays the next scripted step as the GetFeature.
        fn feature_exchange(
            &mut self,
            request: &[u8],
            settle: Duration,
            buf: &mut [u8],
        ) -> anyhow::Result<usize> {
            self.log
                .borrow_mut()
                .features
                .push((request.to_vec(), settle));
            match self.reads.pop_front() {
                None => Ok(0),
                Some(Step::Fail) => bail!("fake feature error"),
                Some(Step::Bytes(bytes)) => {
                    assert!(
                        bytes.len() <= buf.len(),
                        "scripted reply exceeds the buffer"
                    );
                    buf[..bytes.len()].copy_from_slice(&bytes);
                    Ok(bytes.len())
                }
            }
        }
    }

    fn target(vid: u16, pid: u16, link: Link, reads: Vec<Step>) -> (Target, Rc<RefCell<Log>>) {
        target_with(vid, pid, link, reads, None)
    }

    fn target_with(
        vid: u16,
        pid: u16,
        link: Link,
        reads: Vec<Step>,
        fail_write_at: Option<usize>,
    ) -> (Target, Rc<RefCell<Log>>) {
        let log = Rc::new(RefCell::new(Log::default()));
        let fake = Fake {
            log: Rc::clone(&log),
            reads: reads.into(),
            fail_write_at,
        };
        let target = Target {
            vid,
            pid,
            link,
            families: known(vid, pid).map(|d| d.family).into_iter().collect(),
            shapes: Vec::new(),
            opened: None,
            transport: Box::new(fake),
        };
        (target, log)
    }

    /// A target as `open_hid`/`open_serial` builds it: the device's families and shapes, and
    /// the opened collection (`None` = a serial port).
    fn opened_as(
        vid: u16,
        pid: u16,
        families: &[Family],
        shapes: Vec<HidShape>,
        opened: Option<usize>,
    ) -> (Target, Rc<RefCell<Log>>) {
        let link = if opened.is_some() {
            Link::Hid
        } else {
            Link::Serial
        };
        let (mut target, log) = target(vid, pid, link, Vec::new());
        target.families = families.to_vec();
        target.opened = opened.map(|index| shapes[index].clone());
        target.shapes = shapes;
        (target, log)
    }

    fn shape(usage_page: u16, usage: u16, reports: Vec<ReportSize>) -> HidShape {
        HidShape {
            interface: None,
            usage_page,
            usage,
            reports,
        }
    }

    fn both(id: u8, bytes: usize) -> Vec<ReportSize> {
        [ReportKind::Input, ReportKind::Output]
            .into_iter()
            .map(|kind| ReportSize { kind, id, bytes })
            .collect()
    }

    /// A system mouse and the preferred Compx collection, as a Compx receiver lists them.
    fn compx_shapes() -> Vec<HidShape> {
        vec![
            shape(0x0001, 0x0002, both(0x00, 4)),
            shape(0xFF02, 0x0002, both(0x08, 17)),
        ]
    }

    fn bitmouse_shapes() -> Vec<HidShape> {
        let report = |kind, bytes| ReportSize {
            kind,
            id: 0x08,
            bytes,
        };
        vec![shape(
            0xFF05,
            0x0001,
            vec![
                report(ReportKind::Input, 127),
                report(ReportKind::Output, 64),
            ],
        )]
    }

    /// Asserts the refusal names `why` and that nothing touched the device or the budget.
    fn assert_refused(
        door: &Door,
        result: anyhow::Result<Exchange>,
        log: &RefCell<Log>,
        why: &str,
    ) {
        let text = error_text(result);
        assert!(text.contains(why), "{text}");
        assert!(text.contains("nothing was sent"), "{text}");
        assert!(log.borrow().writes.is_empty());
        assert!(log.borrow().timeouts.is_empty());
        assert_eq!(door.writes_left(), 1);
    }

    #[test]
    fn send_as_sends_the_family_frame_to_a_custom_device() {
        // 3554:1234 is not in the catalogue: only `send_as` can query it.
        let (mut target, log) =
            opened_as(0x3554, 0x1234, &[Family::Compx], compx_shapes(), Some(1));
        let exchange = Door::new(1)
            .send_as(&mut target, Family::Compx, false)
            .expect("send_as");
        assert_eq!(
            log.borrow().writes,
            vec![battery_query(Family::Compx).to_vec()]
        );
        assert_eq!(exchange.family, Family::Compx);
        let (mut target, _log) =
            opened_as(0x3554, 0x1234, &[Family::Compx], compx_shapes(), Some(1));
        assert!(error_text(Door::new(1).send(&mut target)).contains("unknown device"));
    }

    #[test]
    fn send_as_refuses_a_family_the_device_does_not_expose() {
        let door = &mut Door::new(1);
        let (mut target, log) =
            opened_as(0x3554, 0x1234, &[Family::Compx], compx_shapes(), Some(1));
        let result = door.send_as(&mut target, Family::Report13, false);
        assert_refused(door, result, &log, "does not expose VGN report 0x13");
    }

    #[test]
    fn send_as_refuses_a_collection_that_is_not_the_pick() {
        let door = &mut Door::new(1);
        // The system mouse collection is open, not the Compx one.
        let (mut target, log) =
            opened_as(0x3554, 0x1234, &[Family::Compx], compx_shapes(), Some(0));
        let result = door.send_as(&mut target, Family::Compx, false);
        assert_refused(door, result, &log, "is not the Compx report 8 target");

        // Two Compx candidates and neither is FF02:0002: no pick, so neither may be queried.
        let shapes = vec![
            shape(0xFF00, 0x0001, both(0x08, 17)),
            shape(0xFF01, 0x0001, both(0x08, 17)),
        ];
        let (mut target, log) = opened_as(0x3554, 0x1234, &[Family::Compx], shapes, Some(0));
        let result = door.send_as(&mut target, Family::Compx, false);
        assert_refused(door, result, &log, "is not the Compx report 8 target");
    }

    #[test]
    fn send_as_refuses_a_recovery_id() {
        let door = &mut Door::new(1);
        for pid in [0xF2FF, 0xF400, 0xF410] {
            let (mut target, log) =
                opened_as(0x3554, pid, &[Family::Compx], compx_shapes(), Some(1));
            let result = door.send_as(&mut target, Family::Compx, false);
            assert_refused(door, result, &log, "recovery id");
        }
    }

    #[test]
    fn send_as_refuses_a_serial_port_outside_the_catalogue() {
        let door = &mut Door::new(1);
        let (mut target, log) = opened_as(0x373B, 0x9999, &[Family::AtkSerial], Vec::new(), None);
        let result = door.send_as(&mut target, Family::AtkSerial, false);
        assert_refused(door, result, &log, "is not in the catalogue");

        let (mut target, log) = opened_as(0x373B, 0x1204, &[Family::AtkSerial], Vec::new(), None);
        Door::new(1)
            .send_as(&mut target, Family::AtkSerial, false)
            .expect("catalogue serial port");
        assert_eq!(
            log.borrow().writes,
            vec![battery_query(Family::AtkSerial).to_vec()]
        );
    }

    #[test]
    fn send_as_refuses_the_other_link() {
        let door = &mut Door::new(1);
        // A HID collection of the N9 Ultra never gets the serial frame.
        let (mut target, log) = opened_as(
            0x373B,
            0x1204,
            &[Family::AtkSerial],
            compx_shapes(),
            Some(1),
        );
        let result = door.send_as(&mut target, Family::AtkSerial, false);
        assert_refused(door, result, &log, "link mismatch");
    }

    /// The type answer of a BITMOUSE device: code 0 = dongle1K, 2 = wired1K (captured layout).
    fn type_reply(code: u8) -> Vec<u8> {
        padded(&[0x08, 0x72, 0x00, 0x3B, 0x00, 0x1A, 0x01, code], 127)
    }

    /// The battery answer of a BITMOUSE device (captured 373B:1299 layout).
    fn battery_reply(percent: u8) -> Vec<u8> {
        padded(&[0x08, 0x72, 0x00, 0x3B, 0x00, 0x07, 0x01, percent], 127)
    }

    /// A 0-byte read: ends the current wait with nothing read.
    fn silence() -> Vec<u8> {
        Vec::new()
    }

    /// The BITMOUSE battery frame for a receiver (target 1); its bytes are checked in `protocol`.
    fn receiver_frame() -> &'static [u8] {
        battery_query_for(Family::AtkBitmouse, false)
    }

    /// The BITMOUSE battery frame for a mouse on its cable (target 0).
    fn cable_frame() -> &'static [u8] {
        battery_query_for(Family::AtkBitmouse, true)
    }

    /// `send_as` on BITMOUSE `vid:pid` with the caller's `cable` while the device plays `reads`;
    /// every write and the exchange, all on one budget unit.
    fn bitmouse_send(
        vid: u16,
        pid: u16,
        cable: bool,
        reads: Vec<Vec<u8>>,
    ) -> (Vec<Vec<u8>>, Exchange) {
        bitmouse_script(vid, pid, cable, bytes(&reads), None)
    }

    /// `bitmouse_send` with scripted read steps and the index of a write that fails.
    fn bitmouse_script(
        vid: u16,
        pid: u16,
        cable: bool,
        reads: Vec<Step>,
        fail_write_at: Option<usize>,
    ) -> (Vec<Vec<u8>>, Exchange) {
        let family = Family::AtkBitmouse;
        let (mut target, log) = opened_as(vid, pid, &[family], bitmouse_shapes(), Some(0));
        target.transport = Box::new(Fake {
            log: log.clone(),
            reads: reads.into(),
            fail_write_at,
        });
        let mut door = Door::new(1);
        let exchange = door.send_as(&mut target, family, cable).expect("send_as");
        assert_eq!(door.writes_left(), 0);
        let writes = log.borrow().writes.clone();
        (writes, exchange)
    }

    fn type_frame() -> Vec<u8> {
        type_query(Family::AtkBitmouse).unwrap().to_vec()
    }

    #[test]
    fn bitmouse_first_frame_follows_the_catalogue_row_else_the_caller() {
        let catalogue = |cable| {
            Some(Connection {
                cable,
                source: Source::Catalogue,
            })
        };
        let caller = |cable| {
            Some(Connection {
                cable,
                source: Source::Caller,
            })
        };
        // 373B:1154 is an unverified cable row, 373B:1155 an unverified receiver row, 373B:9999
        // off the catalogue; no answer at all, so the type query follows the silent battery frame.
        for (pid, cable, first, connection) in [
            (0x1154, false, cable_frame(), catalogue(true)),
            (0x1155, true, receiver_frame(), catalogue(false)),
            (0x9999, true, cable_frame(), caller(true)),
            (0x9999, false, receiver_frame(), caller(false)),
        ] {
            let (writes, exchange) = bitmouse_send(0x373B, pid, cable, Vec::new());
            assert_eq!(writes, [first.to_vec(), type_frame()], "{pid:04X}");
            assert_eq!(exchange.connection, connection, "{pid:04X}");
            let fallback = Fallback {
                silent: first,
                type_reply: None,
                error: None,
            };
            assert_eq!(exchange.fallback, Some(fallback), "{pid:04X}");
            assert_eq!(exchange.reply, None);
        }
    }

    #[test]
    fn bitmouse_cable_row_answering_the_first_frame_is_one_write() {
        // 373B:1299 on its cable (captured): echo, then the 60 % reply to target 0.
        let echo = cable_frame().to_vec();
        let reads = bytes(&[echo.clone(), battery_reply(0x3C)]);
        let (mut target, log) = target(0x373B, 0x1299, Link::Hid, reads);
        let mut door = Door::new(1);
        let exchange = door.send(&mut target).expect("send");
        assert_eq!(log.borrow().writes, [cable_frame().to_vec()]);
        assert_eq!(door.writes_left(), 0);
        assert_eq!(
            exchange.reply,
            Some((battery_reply(0x3C), reading(60, false)))
        );
        assert_eq!(exchange.unrelated, vec![echo]);
        assert_eq!(
            exchange.connection,
            Some(Connection {
                cable: true,
                source: Source::Catalogue,
            })
        );
        assert_eq!(exchange.fallback, None);

        // 373B:1216 (receiver row) answering target 1 is one write too.
        let (writes, exchange) = bitmouse_send(0x373B, 0x1216, false, vec![battery_reply(84)]);
        assert_eq!(writes, [receiver_frame().to_vec()]);
        assert_eq!(
            exchange.reply,
            Some((battery_reply(84), reading(84, false)))
        );
    }

    #[test]
    fn bitmouse_silent_frame_resends_for_the_detected_connection() {
        let detected = |name, cable| {
            Some(Connection {
                cable,
                source: Source::Detected { name },
            })
        };
        // An off-catalogue cable mouse tried as a receiver: silence, the type query's echo (kept
        // as unrelated), wired1K, then the cable frame reads the level.
        let reads = vec![silence(), type_frame(), type_reply(2), battery_reply(0x3C)];
        let (writes, exchange) = bitmouse_send(0x373B, 0x9999, false, reads);
        assert_eq!(exchange.unrelated, vec![type_frame()]);
        assert_eq!(
            writes,
            [
                receiver_frame().to_vec(),
                type_frame(),
                cable_frame().to_vec()
            ]
        );
        assert_eq!(exchange.sent, cable_frame());
        assert_eq!(
            exchange.reply,
            Some((battery_reply(0x3C), reading(60, false)))
        );
        assert_eq!(exchange.connection, detected("wired1K", true));
        let fallback = Fallback {
            silent: receiver_frame(),
            type_reply: Some(type_reply(2)),
            error: None,
        };
        assert_eq!(exchange.fallback, Some(fallback));

        // A catalogue cable row whose device says dongle1K gets the receiver frame once more.
        let reads = vec![silence(), type_reply(0)];
        let (writes, exchange) = bitmouse_send(0x373B, 0x1154, false, reads);
        assert_eq!(
            writes,
            [
                cable_frame().to_vec(),
                type_frame(),
                receiver_frame().to_vec()
            ]
        );
        assert_eq!(exchange.connection, detected("dongle1K", false));
        assert_eq!(exchange.reply, None);

        // A type answer that matches the frame already sent: no re-send.
        let reads = vec![silence(), type_reply(0)];
        let (writes, exchange) = bitmouse_send(0x373B, 0x1155, false, reads);
        assert_eq!(writes, [receiver_frame().to_vec(), type_frame()]);
        assert_eq!(exchange.sent, receiver_frame());
        assert_eq!(exchange.connection, detected("dongle1K", false));

        // An unknown type code is no answer: kept as unrelated, the row's connection stays.
        let reads = vec![silence(), type_reply(9)];
        let (writes, exchange) = bitmouse_send(0x373B, 0x1155, false, reads);
        assert_eq!(writes, [receiver_frame().to_vec(), type_frame()]);
        assert_eq!(exchange.unrelated, vec![type_reply(9)]);
        assert_eq!(exchange.fallback.unwrap().type_reply, None);
        assert_eq!(
            exchange.connection,
            Some(Connection {
                cable: false,
                source: Source::Catalogue,
            })
        );
    }

    #[test]
    fn bitmouse_type_wait_has_its_own_window() {
        // The caller's deadline has passed: the battery wait reads nothing, the type wait still
        // reads within the family's reply window.
        let (mut target, log) = target(0x373B, 0x1155, Link::Hid, bytes(&[type_reply(0)]));
        let exchange = Door::new(1)
            .send_until(&mut target, Instant::now())
            .expect("send");
        assert_eq!(
            log.borrow().writes,
            [receiver_frame().to_vec(), type_frame()]
        );
        assert_timeouts_bounded(&log.borrow().timeouts, Family::AtkBitmouse.reply_timeout());
        assert!(exchange.fallback.unwrap().type_reply.is_some());
    }

    #[test]
    fn bitmouse_silent_verified_row_is_one_write() {
        // 373B:1216 (receiver) and 373B:1299 (cable) are proven on hardware: silence = mouse off.
        for (pid, first) in [(0x1216, receiver_frame()), (0x1299, cable_frame())] {
            let (writes, exchange) = bitmouse_send(0x373B, pid, false, Vec::new());
            assert_eq!(writes, [first.to_vec()], "{pid:04X}");
            assert_eq!(exchange.fallback, None, "{pid:04X}");
            assert_eq!(exchange.reply, None, "{pid:04X}");
        }
    }

    #[test]
    fn bitmouse_error_status_reply_skips_the_type_query() {
        let error = padded(&[0x08, 0x72, 0xFF, 0x3B, 0x00, 0x07, 0x01, 0x00], 127);
        let reads = vec![error.clone()];
        let (writes, exchange) = bitmouse_send(0x373B, 0x1155, false, reads);
        assert_eq!(writes, [receiver_frame().to_vec()]);
        assert_eq!(
            exchange.reply,
            Some((error, Decoded::Invalid("device returned an error")))
        );
        assert_eq!(exchange.fallback, None);
    }

    #[test]
    fn bitmouse_late_battery_reply_in_the_type_wait_skips_the_re_send() {
        // The receiver frame's answer arrives only after the type query went out.
        let reads = vec![silence(), battery_reply(84)];
        let (writes, exchange) = bitmouse_send(0x373B, 0x9999, false, reads);
        assert_eq!(writes, [receiver_frame().to_vec(), type_frame()]);
        assert_eq!(exchange.sent, receiver_frame());
        assert_eq!(
            exchange.reply,
            Some((battery_reply(84), reading(84, false)))
        );
        assert_eq!(exchange.unrelated, Vec::<Vec<u8>>::new());
        assert_eq!(
            exchange.connection,
            Some(Connection {
                cable: false,
                source: Source::Caller,
            })
        );
        let fallback = Fallback {
            silent: receiver_frame(),
            type_reply: None,
            error: None,
        };
        assert_eq!(exchange.fallback, Some(fallback));

        // An error frame in the type wait may answer the type query: it stays unrelated.
        let error = padded(&[0x08, 0x72, 0xFF, 0x3B], 127);
        let reads = vec![silence(), error.clone()];
        let (_, exchange) = bitmouse_send(0x373B, 0x9999, false, reads);
        assert_eq!(exchange.reply, None);
        assert_eq!(exchange.unrelated, vec![error]);
    }

    #[test]
    fn bitmouse_fallback_io_error_is_kept_and_the_exchange_stands() {
        let failed = |exchange: &Exchange, why: &str| {
            assert_eq!(exchange.reply, None);
            let error = exchange.fallback.as_ref().and_then(|f| f.error.clone());
            assert!(
                error.as_deref().is_some_and(|e| e.contains(why)),
                "{error:?}"
            );
        };
        // The type query's write fails: both writes were attempted, the exchange is still Ok.
        let (writes, exchange) =
            bitmouse_script(0x373B, 0x9999, false, bytes(&[silence()]), Some(1));
        assert_eq!(writes, [receiver_frame().to_vec(), type_frame()]);
        failed(&exchange, "type query write failed");
        assert_eq!(exchange.fallback.as_ref().unwrap().silent, receiver_frame());

        // Reading the type answer fails.
        let reads = vec![Step::Bytes(silence()), Step::Fail];
        let (writes, exchange) = bitmouse_script(0x373B, 0x9999, false, reads, None);
        assert_eq!(writes.len(), 2);
        failed(&exchange, "reading the type reply failed");

        // The re-send for the detected cable connection fails: the type answer is kept.
        let reads = bytes(&[silence(), type_reply(2)]);
        let (writes, exchange) = bitmouse_script(0x373B, 0x9999, false, reads, Some(2));
        assert_eq!(writes.last().map(Vec::as_slice), Some(cable_frame()));
        failed(&exchange, "battery re-send write failed");
        assert_eq!(exchange.fallback.unwrap().type_reply, Some(type_reply(2)));
        assert_eq!(exchange.sent, cable_frame());

        // Reading after the re-send fails: both writes went out, the type answer is kept.
        let reads = vec![
            Step::Bytes(silence()),
            Step::Bytes(type_reply(2)),
            Step::Fail,
        ];
        let (writes, exchange) = bitmouse_script(0x373B, 0x9999, false, reads, None);
        assert_eq!(writes.last().map(Vec::as_slice), Some(cable_frame()));
        failed(&exchange, "reading the re-sent reply failed");
        assert_eq!(exchange.fallback.unwrap().type_reply, Some(type_reply(2)));
        assert_eq!(exchange.sent, cable_frame());
    }

    #[test]
    fn bitmouse_re_send_wait_keeps_a_late_type_error_as_unrelated() {
        // After the re-send, an error frame that carries the type command is not the battery reply.
        let type_error = padded(&[0x08, 0x72, 0xFF, 0x3B, 0x00, 0x1A, 0x01, 0x00], 127);
        let reads = vec![
            silence(),
            type_reply(2),
            type_error.clone(),
            battery_reply(0x3C),
        ];
        let (writes, exchange) = bitmouse_send(0x373B, 0x9999, false, reads);
        assert_eq!(writes.last().map(Vec::as_slice), Some(cable_frame()));
        assert_eq!(exchange.unrelated, vec![type_error]);
        assert_eq!(
            exchange.reply,
            Some((battery_reply(0x3C), reading(60, false)))
        );

        // An error frame for the battery command still ends the re-send wait as the reply.
        let battery_error = padded(&[0x08, 0x72, 0xFF, 0x3B, 0x00, 0x07, 0x01, 0x00], 127);
        let reads = vec![silence(), type_reply(2), battery_error.clone()];
        let (_, exchange) = bitmouse_send(0x373B, 0x9999, false, reads);
        assert_eq!(
            exchange.reply,
            Some((battery_error, Decoded::Invalid("device returned an error")))
        );
    }

    fn padded(head: &[u8], len: usize) -> Vec<u8> {
        let mut bytes = head.to_vec();
        bytes.resize(len, 0);
        bytes
    }

    /// The spec's real MCHOSE capture (84 %), as read: report id first, 64 bytes.
    fn mchose_capture() -> Vec<u8> {
        padded(
            &[
                0x55, 0xaa, 0x01, 0x01, 0x03, 0x00, 0x01, 0x0b, 0x01, 0x54, 0x5d,
            ],
            64,
        )
    }

    /// An MCHOSE push notification: not the battery reply.
    fn mchose_notification() -> Vec<u8> {
        padded(
            &[
                0x55, 0xaa, 0x01, 0x00, 0x03, 0x00, 0x04, 0x22, 0x01, 0x54, 0x00,
            ],
            64,
        )
    }

    fn bytes(list: &[Vec<u8>]) -> Vec<Step> {
        list.iter().cloned().map(Step::Bytes).collect()
    }

    /// A deadline no in-memory test reaches.
    fn far() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    fn reading(percent: u8, charging: bool) -> Decoded {
        Decoded::Reading(Reading {
            percent,
            charging,
            asleep: false,
        })
    }

    fn error_text(result: anyhow::Result<Exchange>) -> String {
        format!("{:#}", result.expect_err("the door must refuse"))
    }

    #[test]
    fn sends_exactly_the_family_frame_once() {
        let (mut target, log) = target(0x3837, 0x6011, Link::Hid, bytes(&[mchose_capture()]));
        let exchange = Door::new(1).send(&mut target).expect("send");
        let query = battery_query(Family::Mchose);
        assert_eq!(log.borrow().writes, vec![query.to_vec()]);
        assert_eq!(exchange.sent, query);
        assert_eq!(exchange.family, Family::Mchose);
        let (reply, decoded) = exchange.reply.expect("a reply");
        assert_eq!(reply, mchose_capture());
        assert_eq!(decoded, reading(84, false));
    }

    #[test]
    fn budget_refuses_second_query() {
        let mut door = Door::new(1);
        let (mut target, log) = target(0x3837, 0x6011, Link::Hid, bytes(&[mchose_capture()]));
        door.send_until(&mut target, far()).expect("first send");
        assert!(error_text(door.send_until(&mut target, far())).contains("budget"));
        assert_eq!(log.borrow().writes.len(), 1);
    }

    #[test]
    fn unknown_device_refused() {
        let (mut target, log) = target(0x046D, 0xC52B, Link::Hid, bytes(&[mchose_capture()]));
        assert!(error_text(Door::new(1).send(&mut target)).contains("unknown device"));
        assert!(log.borrow().writes.is_empty());
        assert!(log.borrow().timeouts.is_empty());
    }

    #[test]
    fn n9_ultra_hid_target_refused() {
        let (mut target, log) = target(0x373B, 0x1204, Link::Hid, Vec::new());
        assert!(error_text(Door::new(1).send(&mut target)).contains("link mismatch"));
        assert!(log.borrow().writes.is_empty());
        assert!(log.borrow().timeouts.is_empty());
    }

    #[test]
    fn hid_returns_first_reply_and_keeps_others() {
        let extra = mchose_notification();
        let reads = bytes(&[mchose_notification(), mchose_capture(), extra]);
        let (mut target, log) = target(0x3837, 0x6011, Link::Hid, reads);
        let exchange = Door::new(1).send_until(&mut target, far()).expect("send");
        assert_eq!(
            exchange.reply.map(|(reply, _)| reply),
            Some(mchose_capture())
        );
        assert_eq!(exchange.unrelated, vec![mchose_notification()]);
        assert_eq!(exchange.dropped, 0);
        assert_eq!(log.borrow().timeouts.len(), 2);
    }

    #[test]
    fn zero_read_ends_wait() {
        let reads = vec![Step::Bytes(Vec::new()), Step::Bytes(mchose_capture())];
        let (mut target, log) = target(0x3837, 0x6011, Link::Hid, reads);
        let exchange = Door::new(1).send_until(&mut target, far()).expect("send");
        assert_eq!(exchange.reply, None);
        assert_eq!(log.borrow().timeouts.len(), 1);
    }

    #[test]
    fn expired_deadline_stops_reading() {
        let reads = bytes(&vec![mchose_notification(); 100]);
        let (mut target, log) = target(0x3837, 0x6011, Link::Hid, reads);
        let exchange = Door::new(1)
            .send_until(&mut target, Instant::now())
            .expect("send");
        assert_eq!(log.borrow().writes.len(), 1);
        assert_eq!(log.borrow().timeouts.len(), 0);
        assert_eq!(exchange.reply, None);
    }

    fn assert_timeouts_bounded(timeouts: &[Duration], limit: Duration) {
        assert!(!timeouts.is_empty());
        for timeout in timeouts {
            assert!((MIN_READ_TIMEOUT..=limit).contains(timeout), "{timeout:?}");
        }
        assert!(timeouts.windows(2).all(|pair| pair[1] <= pair[0]));
    }

    #[test]
    fn reads_never_exceed_timeout() {
        let reads = bytes(&vec![mchose_notification(); 5]);
        let (mut hid, hid_log) = target(0x3837, 0x6011, Link::Hid, reads);
        Door::new(1).send(&mut hid).expect("hid send");
        assert_timeouts_bounded(&hid_log.borrow().timeouts, Family::Mchose.reply_timeout());

        let (mut serial, serial_log) =
            target(0x373B, 0x1204, Link::Serial, bytes(&[vec![0x05; 10]]));
        Door::new(1).send(&mut serial).expect("serial send");
        assert_timeouts_bounded(
            &serial_log.borrow().timeouts,
            Family::AtkSerial.reply_timeout(),
        );
    }

    #[test]
    fn unrelated_reports_capped() {
        let reads = bytes(&vec![mchose_notification(); 20]);
        let (mut target, _log) = target(0x3837, 0x6011, Link::Hid, reads);
        let exchange = Door::new(1).send_until(&mut target, far()).expect("send");
        assert_eq!(exchange.unrelated.len(), MAX_KEPT_UNRELATED);
        assert_eq!(exchange.dropped, 4);
        assert_eq!(exchange.reply, None);
    }

    #[test]
    fn read_error_stops() {
        let reads = vec![Step::Fail, Step::Bytes(mchose_capture())];
        let (mut target, log) = target(0x3837, 0x6011, Link::Hid, reads);
        assert!(Door::new(1).send_until(&mut target, far()).is_err());
        assert_eq!(log.borrow().timeouts.len(), 1);
    }

    #[test]
    fn write_error_stops() {
        let reads = bytes(&[mchose_capture()]);
        let (mut target, log) = target_with(0x3837, 0x6011, Link::Hid, reads, Some(0));
        let mut door = Door::new(1);
        assert!(error_text(door.send_until(&mut target, far())).contains("write failed"));
        assert_eq!(log.borrow().timeouts.len(), 0);
        // The budget was spent before the write, so the failed write is not retried.
        assert!(error_text(door.send_until(&mut target, far())).contains("budget"));
        assert_eq!(log.borrow().writes.len(), 1);
    }

    #[test]
    fn serial_frame_accumulates() {
        let frame = padded(&[0x05, 0x00, 0x03, 0x00, 0xd2], SERIAL_FRAME_LEN);
        let reads = bytes(&[frame[..10].to_vec(), frame[10..].to_vec()]);
        let (mut target, log) = target(0x373B, 0x1204, Link::Serial, reads);
        let exchange = Door::new(1).send_until(&mut target, far()).expect("send");
        assert_eq!(
            log.borrow().writes,
            vec![battery_query(Family::AtkSerial).to_vec()]
        );
        assert_eq!(exchange.reply, Some((frame, reading(82, true))));
        assert_eq!(log.borrow().timeouts.len(), 2);
    }

    #[test]
    fn echo_is_not_the_reply() {
        // The device echoes the request before the real reply: the echo must not end the wait.
        let echo = battery_query(Family::Mchose).to_vec();
        let reads = bytes(&[echo.clone(), mchose_capture()]);
        let (mut first, _log) = target(0x3837, 0x6011, Link::Hid, reads);
        let exchange = Door::new(1).send_until(&mut first, far()).expect("send");
        assert_eq!(exchange.reply, Some((mchose_capture(), reading(84, false))));
        assert_eq!(exchange.unrelated, vec![echo.clone()]);

        // Echo only: no reply, the echo is listed as other.
        let (mut second, _log) = target(
            0x3837,
            0x6011,
            Link::Hid,
            bytes(std::slice::from_ref(&echo)),
        );
        let exchange = Door::new(1).send_until(&mut second, far()).expect("send");
        assert_eq!(exchange.reply, None);
        assert_eq!(exchange.unrelated, vec![echo]);
    }

    #[test]
    fn invalid_reply_reported() {
        let bad_checksum = vec![
            0x08, 0x04, 0, 0, 0, 0, 0x54, 0x01, 0x0f, 0xa0, 0, 0, 0, 0, 0, 0, 0x44,
        ];
        let reads = bytes(std::slice::from_ref(&bad_checksum));
        // 391D:1A05 is a Compx device, so the report-8 checksum applies.
        let (mut target, _log) = target(0x391D, 0x1A05, Link::Hid, reads);
        let exchange = Door::new(1).send_until(&mut target, far()).expect("send");
        assert_eq!(
            exchange.reply,
            Some((bad_checksum, Decoded::Invalid("bad checksum")))
        );
    }

    /// A Compx online-status reply (id first) with a valid checksum.
    fn online_reply(status: u8, value: u8) -> Vec<u8> {
        let mut reply = padded(&[0x08, 0x03, status, 0, 0, 0, value], 17);
        reply[16] = protocol::compx::checksum(&reply[..16]);
        reply
    }

    #[test]
    fn online_sends_exactly_the_online_frame_for_compx() {
        let reads = bytes(&[online_reply(0, 1)]);
        let (mut target, log) = target(0x391D, 0x1A05, Link::Hid, reads);
        let exchange = Door::new(1).send_online(&mut target).expect("send");
        let query = online_query(Family::Compx).unwrap();
        assert_eq!(log.borrow().writes, vec![query.to_vec()]);
        assert_eq!(exchange.sent, query);
        assert_eq!(
            exchange.reply,
            Some((online_reply(0, 1), Decoded::Online(protocol::Online::Yes)))
        );
    }

    #[test]
    fn atk_serial_online_frame_accumulates_and_ignores_a_battery_frame() {
        let status = padded(&[0x05, 0x00, 0x04, 0x00, 0x01], SERIAL_FRAME_LEN);
        let battery = padded(&[0x05, 0x00, 0x03, 0x00, 0xd2], SERIAL_FRAME_LEN);
        // A stale battery frame comes first: it is not the status reply and stays unrelated.
        let reads = bytes(&[
            battery.clone(),
            status[..10].to_vec(),
            status[10..].to_vec(),
        ]);
        let (mut target, log) = target(0x373B, 0x1204, Link::Serial, reads);
        let exchange = Door::new(1).send_online(&mut target).expect("send");
        let query = online_query(Family::AtkSerial).unwrap();
        assert_eq!(log.borrow().writes, vec![query.to_vec()]);
        assert_eq!(
            exchange.reply,
            Some((status, Decoded::Online(protocol::Online::Yes)))
        );
        assert_eq!(exchange.unrelated, vec![battery]);
    }

    #[test]
    fn online_echo_is_not_the_reply() {
        let echo = online_query(Family::Compx).unwrap().to_vec();
        let (mut target, _log) = target(
            0x391D,
            0x1A05,
            Link::Hid,
            bytes(std::slice::from_ref(&echo)),
        );
        let exchange = Door::new(1)
            .exchange_until(&mut target, Family::Compx, false, Query::Online, far())
            .expect("send");
        assert_eq!(exchange.reply, None);
        assert_eq!(exchange.unrelated, vec![echo]);
    }

    #[test]
    fn allow_list_is_battery_plus_compx_and_atk_serial_online() {
        for device in protocol::KNOWN {
            let link = device.family.link();
            let open = || target(device.vid, device.pid, link, Vec::new());

            let (mut target, log) = open();
            Door::new(1).send(&mut target).expect("battery send");
            // A feature family sends through SetFeature only; every other family writes only.
            let (written, featured): (Vec<Vec<u8>>, Vec<Vec<u8>>) = {
                let log = log.borrow();
                let featured = log.features.iter().map(|(bytes, _)| bytes.clone());
                (log.writes.clone(), featured.collect())
            };
            // The row's connection picks the frame; a silent unverified BITMOUSE row then gets the
            // type query.
            let battery = battery_query_for(device.family, device.cable);
            let expected: Vec<Vec<u8>> = [battery]
                .into_iter()
                .chain(type_query(device.family).filter(|_| !device.verified))
                .map(<[u8]>::to_vec)
                .collect();
            let id = format!("{:04X}:{:04X}", device.vid, device.pid);
            if device.family.uses_feature_report() {
                assert_eq!((written, featured), (Vec::new(), expected), "{id}");
            } else {
                assert_eq!((written, featured), (expected, Vec::new()), "{id}");
            }

            let (mut target, log) = open();
            let online = Door::new(1).send_online(&mut target);
            match (device.family, online) {
                (Family::Compx | Family::AtkSerial, Ok(_)) => assert_eq!(
                    log.borrow().writes,
                    vec![online_query(device.family).unwrap().to_vec()]
                ),
                (family @ (Family::Compx | Family::AtkSerial), Err(error)) => {
                    panic!("{family:?} online refused: {error:#}")
                }
                (_, Ok(_)) => panic!("{:04X}:{:04X} accepted online", device.vid, device.pid),
                (_, Err(error)) => {
                    assert!(
                        format!("{error:#}").contains("no online query"),
                        "{error:#}"
                    );
                    assert!(log.borrow().writes.is_empty());
                    assert!(log.borrow().timeouts.is_empty());
                    assert!(log.borrow().features.is_empty());
                }
            }
        }
    }

    /// A MonsGeek-dongle-style GetFeature reply: id 0, canRead 1, keyboard 87 %, connected.
    fn rongyuan_reply() -> Vec<u8> {
        padded(&[0x00, 0x01, 0x57, 0x00, 0x00, 0x01, 0x01], 65)
    }

    #[test]
    fn rongyuan_is_one_feature_exchange_of_the_f7_frame() {
        let (mut target, log) = target(0x3151, 0x5038, Link::Hid, bytes(&[rongyuan_reply()]));
        let exchange = Door::new(1).send(&mut target).expect("send");
        let frame = battery_query(Family::RongYuanF7);
        assert_eq!(&frame[..2], &[0x00, 0xF7]);
        assert!(frame[2..].iter().all(|&byte| byte == 0));
        assert_eq!(
            log.borrow().features,
            vec![(frame.to_vec(), Family::RongYuanF7.reply_timeout())]
        );
        // No output report and no input read on this channel.
        assert!(log.borrow().writes.is_empty());
        assert!(log.borrow().timeouts.is_empty());
        assert_eq!(exchange.sent, frame);
        assert_eq!(exchange.reply, Some((rongyuan_reply(), reading(87, false))));
    }

    #[test]
    fn rongyuan_feature_failure_is_not_retried() {
        let mut door = Door::new(1);
        let reads = vec![Step::Fail, Step::Bytes(rongyuan_reply())];
        let (mut failing, log) = target(0x3151, 0x5038, Link::Hid, reads);
        assert!(error_text(door.send(&mut failing)).contains("feature exchange failed"));
        assert!(error_text(door.send(&mut failing)).contains("budget"));
        assert_eq!(log.borrow().features.len(), 1);
        // An online query is refused before any I/O.
        let (mut online, log) = target(0x3151, 0x5038, Link::Hid, Vec::new());
        let result = Door::new(1).send_online(&mut online);
        assert!(error_text(result).contains("no online query"));
        assert!(log.borrow().features.is_empty());
    }

    #[test]
    fn rongyuan_empty_or_echoed_reply_is_no_reply() {
        let (mut empty, _log) = target(0x3151, 0x5038, Link::Hid, Vec::new());
        let exchange = Door::new(1).send(&mut empty).expect("send");
        assert_eq!(exchange.reply, None);

        let echo = battery_query(Family::RongYuanF7).to_vec();
        let reads = bytes(std::slice::from_ref(&echo));
        let (mut echoed, _log) = target(0x3151, 0x5038, Link::Hid, reads);
        let exchange = Door::new(1).send(&mut echoed).expect("send");
        assert_eq!(exchange.reply, None);
        assert_eq!(exchange.unrelated, vec![echo]);

        // An unfilled all-zero reply is the reply, but carries no level.
        let zeros = vec![0u8; 65];
        let reads = bytes(std::slice::from_ref(&zeros));
        let (mut unfilled, _log) = target(0x3151, 0x5038, Link::Hid, reads);
        let exchange = Door::new(1).send(&mut unfilled).expect("send");
        assert_eq!(
            exchange.reply,
            Some((zeros, Decoded::Invalid("percent 0: no data")))
        );
    }

    #[test]
    fn vgn_kc2_skips_pushes_until_the_b4_06_reply() {
        let push = padded(&[0xB6, 0xE2, 0x01, 0x00, 0x55], 21);
        let mut reply = padded(&[0xB4, 0x06], 64);
        reply[20] = 0x4B;
        let reads = bytes(&[push.clone(), reply.clone()]);
        let (mut target, log) = target(0x391D, 0x1A21, Link::Hid, reads);
        let exchange = Door::new(1).send_until(&mut target, far()).expect("send");
        assert_eq!(
            log.borrow().writes,
            vec![battery_query(Family::VgnKc2).to_vec()]
        );
        assert!(log.borrow().features.is_empty());
        assert_eq!(exchange.reply, Some((reply, reading(75, false))));
        assert_eq!(exchange.unrelated, vec![push]);
    }

    /// A Driveall `55 10` reply as read (hidapi drops report id 0): payload at [8], level at
    /// payload[17], chargeStatus at payload[18].
    fn driveall_reply(percent: u8, charge: u8) -> Vec<u8> {
        let mut reply = padded(&[0x55, 0x10, 0x30], 64);
        reply[25] = percent;
        reply[26] = charge;
        reply
    }

    #[test]
    fn driveall_is_one_get_device_info_write_skipping_notifications() {
        let asleep = padded(&[0x55, 0xFC, 0x06], 64);
        let reads = bytes(&[asleep.clone(), driveall_reply(87, 0)]);
        let (mut target, log) = target(0x0C45, 0xFEF9, Link::Hid, reads);
        let exchange = Door::new(1).send_until(&mut target, far()).expect("send");
        let frame = battery_query(Family::Driveall);
        assert_eq!(&frame[..9], &[0x00, 0xAA, 0x10, 0x30, 0, 0, 0, 0x01, 0]);
        assert_eq!(log.borrow().writes, vec![frame.to_vec()]);
        assert!(log.borrow().features.is_empty());
        assert_eq!(
            exchange.reply,
            Some((driveall_reply(87, 0), reading(87, false)))
        );
        assert_eq!(exchange.unrelated, vec![asleep]);
    }

    #[test]
    fn driveall_without_a_55_10_reply_is_no_reading_and_no_retry() {
        // Only dongle notifications arrive (keyboard asleep, then disconnected).
        let notices = [
            padded(&[0x55, 0xFC, 0x06], 64),
            padded(&[0x55, 0xFC, 0x04], 64),
        ];
        let (mut offline, log) = target(0x0C45, 0xFEF9, Link::Hid, bytes(&notices));
        let mut door = Door::new(1);
        let exchange = door.send_until(&mut offline, far()).expect("send");
        assert_eq!(exchange.reply, None);
        assert_eq!(exchange.unrelated, notices.to_vec());
        assert_eq!(log.borrow().writes.len(), 1);
        // The budget is spent: a second query in the same refresh is refused before any I/O.
        assert!(error_text(door.send(&mut offline)).contains("budget"));
        assert_eq!(log.borrow().writes.len(), 1);
        // Driveall has no online query.
        let (mut online, log) = target(0x0C45, 0xFEF9, Link::Hid, Vec::new());
        assert!(error_text(Door::new(1).send_online(&mut online)).contains("no online query"));
        assert!(log.borrow().writes.is_empty());
    }

    #[test]
    fn driveall_reports_nonzero_charge_status_as_charging() {
        let reads = bytes(&[driveall_reply(64, 1)]);
        let (mut target, _log) = target(0x0C45, 0xFEF9, Link::Hid, reads);
        let exchange = Door::new(1).send_until(&mut target, far()).expect("send");
        assert_eq!(
            exchange.reply,
            Some((driveall_reply(64, 1), reading(64, true)))
        );
    }

    /// A Generic output row shaped like Driveall: report 0, reply `55 10 …` read without id 0.
    static GENERIC_OUTPUT: GenericSpec = GenericSpec {
        usage_page: 0xFF60,
        usage: 0x0061,
        report: GenericReport::Output,
        report_bytes: 64,
        request: &[0x00, 0xAA, 0x10, 0x30, 0, 0, 0, 0x01, 0],
        reply_prefix: &[0x00, 0x55, 0x10],
        level_byte: 26,
        charging: None,
        scale: 100,
    };

    /// A Generic feature row on report 0 whose reply carries the level at [2].
    static GENERIC_FEATURE: GenericSpec = GenericSpec {
        usage_page: 0xFFFF,
        usage: 0x0002,
        report: GenericReport::Feature,
        report_bytes: 64,
        request: &[0x00, 0xF7, 0, 0],
        reply_prefix: &[0x00],
        level_byte: 2,
        charging: None,
        scale: 100,
    };

    #[test]
    fn generic_output_is_one_write_and_no_retry() {
        let family = Family::Generic(&GENERIC_OUTPUT);
        let notice = padded(&[0x55, 0xFC, 0x06], 64);
        let mut reply = padded(&[0x55, 0x10, 0x30], 64);
        reply[25] = 64;
        let reads = bytes(&[notice.clone(), reply.clone()]);
        let (mut device, log) = target(0x1234, 0x5678, Link::Hid, reads);
        let mut door = Door::new(1);
        let exchange = door
            .exchange_until(&mut device, family, false, Query::Battery, far())
            .expect("send");
        assert_eq!(log.borrow().writes, vec![GENERIC_OUTPUT.request.to_vec()]);
        assert!(log.borrow().features.is_empty());
        assert_eq!(exchange.reply, Some((reply, reading(64, false))));
        assert_eq!(exchange.unrelated, vec![notice]);
        // The budget of 1 is spent: a second query is refused before any I/O.
        let again = door.exchange_until(&mut device, family, false, Query::Battery, far());
        assert!(error_text(again).contains("budget"));
        assert_eq!(log.borrow().writes.len(), 1);
        // No reply is no reading, and still one write.
        let (mut silent, log) = target(0x1234, 0x5678, Link::Hid, Vec::new());
        let exchange = Door::new(1)
            .exchange_until(&mut silent, family, false, Query::Battery, far())
            .expect("send");
        assert_eq!(exchange.reply, None);
        assert_eq!(log.borrow().writes.len(), 1);
        let online = Door::new(1).exchange_until(&mut silent, family, false, Query::Online, far());
        assert!(error_text(online).contains("no online query"));
    }

    #[test]
    fn generic_feature_is_one_set_get_pair() {
        let family = Family::Generic(&GENERIC_FEATURE);
        let reply = padded(&[0x00, 0x01, 0x57], 4);
        let (mut target, log) = target(
            0x1234,
            0x5678,
            Link::Hid,
            bytes(std::slice::from_ref(&reply)),
        );
        let mut door = Door::new(1);
        let exchange = door
            .exchange_until(&mut target, family, false, Query::Battery, far())
            .expect("send");
        assert!(log.borrow().writes.is_empty());
        assert_eq!(
            log.borrow().features,
            vec![(GENERIC_FEATURE.request.to_vec(), Duration::from_millis(100))]
        );
        assert_eq!(exchange.reply, Some((reply, reading(87, false))));
        assert!(
            error_text(door.exchange_until(&mut target, family, false, Query::Battery, far()))
                .contains("budget")
        );
        assert_eq!(log.borrow().features.len(), 1);
    }

    #[test]
    fn send_as_never_sends_generic_outside_its_catalogue_row() {
        let family = Family::Generic(&GENERIC_OUTPUT);
        let shapes = vec![shape(0xFF60, 0x0061, both(0, 64))];
        // Even a target claiming the family and opened on its collection is refused.
        let (mut target, log) = opened_as(0x1234, 0x5678, &[family], shapes, Some(0));
        let mut door = Door::new(1);
        let result = door.send_as(&mut target, family, false);
        assert_refused(&door, result, &log, "no catalogue Generic row");
        assert!(log.borrow().features.is_empty());
    }

    #[test]
    fn refused_online_keeps_the_budget() {
        let mut door = Door::new(1);
        let (mut mchose, log) = target(0x3837, 0x6011, Link::Hid, Vec::new());
        assert!(door.send_online(&mut mchose).is_err());
        assert_eq!(door.writes_left(), 1);
        assert!(log.borrow().writes.is_empty());
    }
}
