//! The one write door: a write budget, the family's fixed battery query (plus the Compx and
//! ATK serial online-status query), one write per query and a bounded wait for the reply. No
//! API here takes raw bytes.

use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use protocol::{
    Decoded, Family, Link, SERIAL_FRAME_LEN, battery_query, decode, decode_online, known,
    online_query,
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
    pub sent: &'static [u8],
    /// The reply and its decoding (never `NotTheReply`); `None` = no reply before the deadline.
    pub reply: Option<(Vec<u8>, Decoded)>,
    pub unrelated: Vec<Vec<u8>>,
    pub dropped: usize,
}

/// The two frames the door can send; each decides how its reply is decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Query {
    Battery,
    Online,
}

impl Exchange {
    /// Keeps `bytes` as the reply, or as an unrelated report; true once the reply is in.
    /// An echo of the request decodes as `Invalid`, so it is filed as unrelated to keep the wait open.
    fn take(&mut self, query: Query, bytes: Vec<u8>) -> bool {
        let decoded = match query {
            _ if bytes == self.sent => Decoded::NotTheReply,
            Query::Battery => decode(self.family, &bytes),
            Query::Online => decode_online(self.family, &bytes),
        };
        match decoded {
            Decoded::NotTheReply if self.unrelated.len() < MAX_KEPT_UNRELATED => {
                self.unrelated.push(bytes);
                false
            }
            Decoded::NotTheReply => {
                self.dropped += 1;
                false
            }
            decoded => {
                self.reply = Some((bytes, decoded));
                true
            }
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

    fn send_query(&mut self, target: &mut Target, query: Query) -> anyhow::Result<Exchange> {
        // The deadline is the family's reply timeout; an unknown device is refused in `exchange_until`.
        let timeout =
            known(target.vid, target.pid).map_or(Duration::ZERO, |d| d.family.reply_timeout());
        self.exchange_until(target, query, Instant::now() + timeout)
    }

    /// `send` with an explicit deadline.
    #[cfg(test)]
    pub(crate) fn send_until(
        &mut self,
        target: &mut Target,
        deadline: Instant,
    ) -> anyhow::Result<Exchange> {
        self.exchange_until(target, Query::Battery, deadline)
    }

    /// Refusals happen before any I/O; the budget drops before the write, so a failed write is
    /// never retried.
    fn exchange_until(
        &mut self,
        target: &mut Target,
        query: Query,
        deadline: Instant,
    ) -> anyhow::Result<Exchange> {
        let (vid, pid) = (target.vid, target.pid);
        let family = known(vid, pid)
            .ok_or_else(|| anyhow!("unknown device {vid:04X}:{pid:04X}; nothing was sent"))?
            .family;
        if family.link() != target.link {
            bail!(
                "link mismatch: {vid:04X}:{pid:04X} is not queried over this link; nothing was sent"
            );
        }
        let sent = match query {
            Query::Battery => battery_query(family),
            Query::Online => online_query(family)
                .ok_or_else(|| anyhow!("this family has no online query; nothing was sent"))?,
        };
        if self.writes_left == 0 {
            bail!("write budget used up; nothing was sent");
        }

        self.writes_left -= 1;
        target.transport.write(sent).context("write failed")?;

        let mut exchange = Exchange {
            family,
            sent,
            reply: None,
            unrelated: Vec::new(),
            dropped: 0,
        };
        await_reply(target, query, deadline, &mut exchange)
            .context("reading the reply failed (the query was sent)")?;
        Ok(exchange)
    }
}

/// Time left until `deadline`, at least 1 ms; `None` once it has passed.
pub(crate) fn time_left(deadline: Instant) -> Option<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    (!left.is_zero()).then(|| left.max(MIN_READ_TIMEOUT))
}

/// Reads until the reply, the deadline or a 0-byte read. A HID unit is one read; a serial
/// unit gathers chunks into one 64-byte frame, and a partial frame at the end is still decoded.
fn await_reply(
    target: &mut Target,
    query: Query,
    deadline: Instant,
    exchange: &mut Exchange,
) -> anyhow::Result<()> {
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
        if exchange.take(query, std::mem::take(&mut pending)) {
            return Ok(());
        }
    }
    if !pending.is_empty() {
        exchange.take(query, pending);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Transport;
    use protocol::Reading;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// Everything the fake saw: each write and the timeout of each read call.
    #[derive(Default)]
    struct Log {
        writes: Vec<Vec<u8>>,
        timeouts: Vec<Duration>,
    }

    enum Step {
        Bytes(Vec<u8>),
        Fail,
    }

    /// Plays back scripted reads; an empty script reads 0 bytes.
    struct Fake {
        log: Rc<RefCell<Log>>,
        reads: VecDeque<Step>,
        fail_write: bool,
    }

    impl Transport for Fake {
        fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
            self.log.borrow_mut().writes.push(bytes.to_vec());
            if self.fail_write {
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
    }

    fn target(vid: u16, pid: u16, link: Link, reads: Vec<Step>) -> (Target, Rc<RefCell<Log>>) {
        target_with(vid, pid, link, reads, false)
    }

    fn target_with(
        vid: u16,
        pid: u16,
        link: Link,
        reads: Vec<Step>,
        fail_write: bool,
    ) -> (Target, Rc<RefCell<Log>>) {
        let log = Rc::new(RefCell::new(Log::default()));
        let fake = Fake {
            log: Rc::clone(&log),
            reads: reads.into(),
            fail_write,
        };
        let target = Target {
            vid,
            pid,
            link,
            transport: Box::new(fake),
        };
        (target, log)
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
        let (mut target, log) = target_with(0x3837, 0x6011, Link::Hid, reads, true);
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
            .exchange_until(&mut target, Query::Online, far())
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
            assert_eq!(
                log.borrow().writes,
                vec![battery_query(device.family).to_vec()],
                "{:04X}:{:04X}",
                device.vid,
                device.pid
            );

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
                }
            }
        }
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
