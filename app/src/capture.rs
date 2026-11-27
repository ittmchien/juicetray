//! Plug-in listening (Windows): reads, never writes, every collection the open rule allows for
//! `LISTEN` after an unknown device's arrival, so the Add device report shows what it sent.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use door::{Access, HidCollection};
#[cfg(windows)]
use door::{Inventory, Listing, Target};
use protocol::{HidShape, ReportKind, is_recovery};

use crate::log;
use crate::plug::{Id, id_text};
#[cfg(windows)]
use crate::plug::{Settings, offers};
use crate::scan::{head_line, interface_label};
#[cfg(windows)]
use crate::vendor_apps::{paused_by, running};

#[cfg(windows)]
mod keys;

/// How long a capture listens after the trigger.
pub const LISTEN: Duration = Duration::from_secs(30);
/// Past its end by this much, a capture counts as finished even without its last update.
pub const GRACE: Duration = Duration::from_secs(2);
/// Distinct frames kept per collection.
const KEEP: usize = 4;
/// Bytes that make a frame distinct; also the bytes kept.
const HEAD: usize = 32;
/// Frames this close to a keyboard event keep no bytes.
const KEY_WINDOW: Duration = Duration::from_millis(500);
/// Finished entries older than this are dropped on the next claim.
const EVICT: Duration = Duration::from_secs(600);
/// Bytes of the whole `Listen:` section of a report.
const REPORT_MAX: usize = 2000;
/// The last line of a cut `Listen:` section.
const CUT: &str = "  … cut";

/// First read per target per round.
const SLICE: Duration = Duration::from_millis(20);
/// Reads after a hit, until one comes back empty.
const DRAIN_SLICE: Duration = Duration::from_millis(1);
/// Drain reads per target per round.
const DRAIN: usize = 64;
/// An arrival rescans this long after the latest one, so a plug-in's burst of arrivals scans once.
const REARRIVAL: Duration = Duration::from_millis(250);
/// The generic desktop page's keyboard and keypad: collections whose frames may hold keys.
const KEYBOARD_PAGE: u16 = 0x0001;
const KEYBOARD_USAGES: [u16; 2] = [0x0006, 0x0007];
/// Periodic rescan while the collections settle, and the vendor-app check.
#[cfg(windows)]
const RESCAN: Duration = Duration::from_secs(2);

/// Where a capture stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Listening {
        until: Instant,
    },
    /// `ended`: why it ended early (`stopped`, `device left`, `<app> opened`).
    Done {
        ran: Duration,
        ended: Option<String>,
    },
    NotRun(String),
}

/// One distinct frame. `from..=first`: when its first copy may have arrived (the last empty read
/// of its handle .. the read that got it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    /// At most `HEAD` bytes.
    pub bytes: Vec<u8>,
    pub len: usize,
    pub count: u32,
    pub from: Duration,
    pub first: Duration,
}

/// What one collection sent: every frame counted, the first distinct ones kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frames {
    pub total: u32,
    pub kept: Vec<Kept>,
    /// Frames past the kept ones.
    pub other: u32,
    /// Frames dropped because a key may have been typed while they were sent.
    pub typed: u32,
}

/// One collection of a capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heard {
    pub label: String,
    pub frames: Frames,
    pub error: Option<String>,
}

/// One id's capture, as the report reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listen {
    pub state: State,
    pub started: Instant,
    pub heard: Vec<Heard>,
}

impl Frames {
    /// One frame read at `to` that arrived after `from` (its handle's last empty read); one near
    /// a key in `keys` keeps no bytes.
    pub fn record(&mut self, from: Duration, to: Duration, bytes: &[u8], keys: &[Duration]) {
        self.total = self.total.saturating_add(1);
        if near_key(from, to, keys, KEY_WINDOW) {
            self.typed = self.typed.saturating_add(1);
            return;
        }
        let head = &bytes[..bytes.len().min(HEAD)];
        if let Some(kept) = self.kept.iter_mut().find(|kept| kept.bytes == head) {
            kept.count = kept.count.saturating_add(1);
        } else if self.kept.len() < KEEP {
            self.kept.push(Kept {
                bytes: head.to_vec(),
                len: bytes.len(),
                count: 1,
                from,
                first: to,
            });
        } else {
            // ponytail: "the rest" counts reports, not distinct frames; bounded memory.
            self.other = self.other.saturating_add(1);
        }
    }

    /// A key event at `at`, seen after the frames: kept frames whose arrival span lies near it
    /// move to `typed`.
    pub fn key(&mut self, at: Duration) {
        let mut typed = 0u32;
        self.kept.retain(|kept| {
            let near = near_key(kept.from, kept.first, &[at], KEY_WINDOW);
            if near {
                typed = typed.saturating_add(kept.count);
            }
            !near
        });
        self.typed = self.typed.saturating_add(typed);
    }

    /// Frames read before the key filter ran are not trusted, bytes or not: all move to `typed`.
    pub fn untrust(&mut self) {
        let kept = self
            .kept
            .iter()
            .fold(0u32, |sum, kept| sum.saturating_add(kept.count));
        self.typed = self.typed.saturating_add(kept).saturating_add(self.other);
        self.kept.clear();
        self.other = 0;
    }
}

/// Whether a key event falls in `from − window ..= to + window` (no subtraction, so no underflow).
fn near_key(from: Duration, to: Duration, keys: &[Duration], window: Duration) -> bool {
    keys.iter()
        .any(|&key| key + window >= from && key <= to + window)
}

impl Listen {
    /// Time left while listening; `None` once done, not run, or past `until + GRACE`.
    pub fn left(&self, now: Instant) -> Option<Duration> {
        match self.state {
            State::Listening { until } if now < until + GRACE => {
                Some(until.saturating_duration_since(now))
            }
            _ => None,
        }
    }
}

/// The report's `Listen:` section: a header, then each collection's counts and kept frames;
/// `tried_at` marks a Try that fell inside the capture.
pub fn report_lines(
    listen: Option<&Listen>,
    now: Instant,
    tried_at: Option<Instant>,
) -> Vec<String> {
    let Some(listen) = listen else {
        return vec!["Listen: not run (no plug-in seen)".to_owned()];
    };
    // Only the one line while listening, so the details box stays still.
    let (mut header, end) = match &listen.state {
        State::NotRun(why) => return vec![format!("Listen: not run ({why})")],
        State::Listening { .. } if listen.left(now).is_some() => {
            return vec!["Listen: still listening".to_owned()];
        }
        State::Listening { until } => ("Listen: did not finish".to_owned(), *until),
        State::Done { ran, ended } => {
            let mut header = format!("Listen: {} s after plug-in, read-only", ran.as_secs());
            if let Some(ended) = ended {
                header.push_str(&format!(" ({ended})"));
            }
            (header, listen.started + *ran)
        }
    };
    if let Some(at) = tried_at.filter(|at| (listen.started..=end).contains(at)) {
        let offset = at.duration_since(listen.started).as_millis();
        header.push_str(&format!("; Try sent at +{offset} ms"));
    }
    // A capture that did not finish never ran its last key check: counts only, no bytes.
    let finished = matches!(listen.state, State::Done { .. });
    let mut body = Vec::new();
    for heard in &listen.heard {
        body.push(match heard.frames.total {
            0 => format!("  {}: no reports", heard.label),
            total => format!("  {}: {total} report(s)", heard.label),
        });
        let shown: &[Kept] = if finished { &heard.frames.kept } else { &[] };
        body.extend(shown.iter().map(|kept| {
            format!(
                "    +{} ms x{} {}",
                kept.first.as_millis(),
                kept.count,
                head_line("rx", &kept.bytes, kept.len)
            )
        }));
        if heard.frames.other > 0 {
            body.push(format!("    {} other report(s)", heard.frames.other));
        }
        if heard.frames.typed > 0 {
            body.push(format!(
                "    {} report(s) dropped: keys were typed",
                heard.frames.typed
            ));
        }
        if let Some(error) = &heard.error {
            body.push(format!("    {error}"));
        }
    }
    if listen.heard.is_empty() && finished {
        body.push("  nothing allowed to open".to_owned());
    }
    capped(header, body)
}

/// The header and as many whole body lines as fit in `REPORT_MAX` bytes, then `CUT` if any
/// line was dropped.
fn capped(header: String, body: Vec<String>) -> Vec<String> {
    let whole = header.len() + body.iter().map(|line| line.len() + 1).sum::<usize>();
    let mut lines = vec![header];
    if whole <= REPORT_MAX {
        lines.extend(body);
        return lines;
    }
    let mut size = lines[0].len() + 1 + CUT.len();
    for line in body {
        if size + line.len() + 1 > REPORT_MAX {
            break;
        }
        size += line.len() + 1;
        lines.push(line);
    }
    lines.push(CUT.to_owned());
    lines
}

/// Collections to open now: opened at enumeration, with an input report (or a descriptor not
/// parsed), not tried yet in this capture; each path once.
fn new_collections<'a>(hid: &'a [HidCollection], tried: &[String]) -> Vec<&'a HidCollection> {
    let mut picked: Vec<&HidCollection> = Vec::new();
    for collection in hid {
        let reports = &collection.shape.reports;
        let has_input = reports.is_empty()
            || reports
                .iter()
                .any(|report| report.kind == ReportKind::Input);
        if matches!(collection.access, Access::Opened { .. })
            && has_input
            && !tried.contains(&collection.path)
            && !picked.iter().any(|done| done.path == collection.path)
        {
            picked.push(collection);
        }
    }
    picked
}

/// Whether a collection is a keyboard or keypad, whose frames may hold keys.
fn is_keyboard(shape: &HidShape) -> bool {
    shape.usage_page == KEYBOARD_PAGE && KEYBOARD_USAGES.contains(&shape.usage)
}

/// A device left only once it was seen present.
fn device_left(seen: bool, present: bool) -> bool {
    seen && !present
}

/// Whether to take a new inventory: none yet, the id's (listed, openable) counts changed, or the
/// last one had a failure (`Access::Failed`, an open error or a key filter that did not start).
fn inventory_due(last: Option<((usize, usize), bool)>, counts: (usize, usize)) -> bool {
    last.is_none_or(|(seen, retry)| retry || seen != counts)
}

type Store = BTreeMap<Id, Entry>;

/// Every capture by id; the window and the popup path share it.
static CAPTURES: Mutex<Store> = Mutex::new(BTreeMap::new());
/// Generations never repeat, even after eviction, so a late thread never matches a new entry.
static NEXT_GEN: AtomicU64 = AtomicU64::new(1);

struct Entry {
    listen: Listen,
    generation: u64,
    /// [Stop] was pressed.
    stop: bool,
    /// The id arrived again: the running capture lists the devices again.
    rescan: bool,
    /// A window started or joined it, so it opens without waiting for an offer.
    window: bool,
}

/// The flags a capture thread reads each round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Control {
    stop: bool,
    rescan: bool,
    window: bool,
}

// Pure store logic on a map, so tests use a local map and never race other tests' entries.

/// A new capture of `id` and its generation; `None` for a recovery id or while one still listens
/// (that one rescans, and a window start lifts its gate).
fn claim_in(store: &mut Store, id: Id, now: Instant, window: bool) -> Option<u64> {
    store.retain(|_, entry| {
        matches!(entry.listen.state, State::Listening { .. }) || now < entry.listen.started + EVICT
    });
    if is_recovery(id.0, id.1) {
        return None;
    }
    if let Some(entry) = store.get_mut(&id)
        && entry.listen.left(now).is_some()
    {
        entry.rescan = true;
        entry.window |= window;
        return None;
    }
    let generation = NEXT_GEN.fetch_add(1, Ordering::Relaxed);
    store.insert(
        id,
        Entry {
            listen: Listen {
                state: State::Listening {
                    until: now + LISTEN,
                },
                started: now,
                heard: Vec::new(),
            },
            generation,
            stop: false,
            rescan: false,
            window,
        },
    );
    Some(generation)
}

/// The capture's flags, taking `rescan`; `None` once a newer capture replaced it.
fn control_in(store: &mut Store, id: Id, generation: u64) -> Option<Control> {
    let entry = store
        .get_mut(&id)
        .filter(|entry| entry.generation == generation)?;
    let control = Control {
        stop: entry.stop,
        rescan: entry.rescan,
        window: entry.window,
    };
    entry.rescan = false;
    Some(control)
}

/// Changes the capture, unless a newer one replaced it.
fn update_in(store: &mut Store, id: Id, generation: u64, change: impl FnOnce(&mut Listen)) {
    if let Some(entry) = store.get_mut(&id)
        && entry.generation == generation
    {
        change(&mut entry.listen);
    }
}

/// Drops the capture, unless a newer one replaced it.
fn forget_in(store: &mut Store, id: Id, generation: u64) {
    if store
        .get(&id)
        .is_some_and(|entry| entry.generation == generation)
    {
        store.remove(&id);
    }
}

/// An arrival or removal of `id`: a running capture lists the devices again; nothing starts.
fn nudge_in(store: &mut Store, id: Id) {
    if let Some(entry) = store.get_mut(&id)
        && matches!(entry.listen.state, State::Listening { .. })
    {
        entry.rescan = true;
    }
}

fn stop_in(store: &mut Store, id: Id) {
    if let Some(entry) = store.get_mut(&id) {
        entry.stop = true;
    }
}

/// Runs `change` on the global store; a panic elsewhere never locks the store for good.
fn with<T>(change: impl FnOnce(&mut Store) -> T) -> T {
    change(&mut CAPTURES.lock().unwrap_or_else(PoisonError::into_inner))
}

fn claim(id: Id, now: Instant, window: bool) -> Option<u64> {
    with(|store| claim_in(store, id, now, window))
}

fn control(id: Id, generation: u64) -> Option<Control> {
    with(|store| control_in(store, id, generation))
}

fn update(id: Id, generation: u64, change: impl FnOnce(&mut Listen)) {
    with(|store| update_in(store, id, generation, change));
}

fn forget(id: Id, generation: u64) {
    with(|store| forget_in(store, id, generation));
}

/// Tells a running capture of `id` that the device arrived or left.
#[cfg(windows)]
pub fn nudge(id: Id) {
    with(|store| nudge_in(store, id));
}

/// [Stop]: the capture of `id` ends with what it has.
pub fn stop(id: Id) {
    with(|store| stop_in(store, id));
}

/// The capture of `id` as it stands now.
pub fn snapshot(id: Id) -> Option<Listen> {
    with(|store| store.get(&id).map(|entry| entry.listen.clone()))
}

/// What one listing says about the id; `counts` = (collections listed, openable).
#[derive(Debug, Clone, Copy)]
struct Scan {
    listed: bool,
    present: bool,
    counts: (usize, usize),
    /// The device lists offer the id (popup path only).
    offered: bool,
}

/// One opened collection: read only, never written.
trait Reader {
    fn read(&mut self, timeout: Duration) -> anyhow::Result<Option<Vec<u8>>>;
}

/// The devices as a capture sees them: `door` on Windows, a scripted fake in tests.
trait Bus {
    fn scan(&mut self) -> Scan;
    /// The id's collections from the last scan's listing (opens descriptors only).
    fn collections(&mut self) -> Vec<HidCollection>;
    fn open(&mut self, collection: &HidCollection) -> anyhow::Result<Box<dyn Reader>>;
    fn vendor_app(&mut self) -> Option<&'static str>;
    /// Holds the key watcher for this capture (idempotent once it holds it).
    fn watch_keys(&mut self) -> Result<(), String>;
    /// Keyboard events (make and break, any keyboard) after `after`.
    fn key_times(&mut self, after: Instant) -> Vec<Instant>;
}

/// An opened collection and the time of its last empty read (or its open).
struct Open {
    heard: usize,
    path: String,
    reader: Box<dyn Reader>,
    empty_at: Duration,
}

/// One capture's loop state, kept on its own thread; paths never leave it.
struct Capture<'b, B> {
    bus: &'b mut B,
    id: Id,
    generation: u64,
    start: Instant,
    /// Opening is allowed: a window started or joined it, or the device lists offer the id.
    gate: bool,
    seen: bool,
    /// Paths opened in this capture (a read-failed one stays, so it is never reopened).
    tried: Vec<String>,
    /// Each `heard` entry's path.
    paths: Vec<String>,
    open: Vec<Open>,
    /// Keyboard event times since `start`, the synthetic one at watch start included.
    keys: Vec<Duration>,
    needs_keys: bool,
    watching: bool,
    filter_error: Option<String>,
    key_cursor: Instant,
    /// The counts at the last inventory and whether it had a failure.
    last: Option<((usize, usize), bool)>,
    /// Rescans in a row with equal counts.
    stable: u32,
    prev: Option<(usize, usize)>,
    read_failed: bool,
    list_failed: bool,
    ever_opened: bool,
    next_scan: Instant,
    /// An arrival asked for a rescan, due at `next_scan`.
    rescan_asked: bool,
    next_app: Instant,
    /// The brand's app open at the last check: nothing is opened while it holds the device.
    app: Option<&'static str>,
}

/// Listens to `id` read-only from `start` to `until`: opens each new collection the open rule
/// allows and records what it sends until stopped, the device leaves or its vendor app opens.
fn run(
    bus: &mut impl Bus,
    id: Id,
    generation: u64,
    start: Instant,
    until: Instant,
    rescan: Duration,
) {
    let mut capture = Capture {
        bus,
        id,
        generation,
        start,
        gate: false,
        seen: false,
        tried: Vec::new(),
        paths: Vec::new(),
        open: Vec::new(),
        keys: Vec::new(),
        needs_keys: false,
        watching: false,
        filter_error: None,
        key_cursor: start.checked_sub(KEY_WINDOW).unwrap_or(start),
        last: None,
        stable: 0,
        prev: None,
        read_failed: false,
        list_failed: false,
        ever_opened: false,
        next_scan: start,
        rescan_asked: false,
        next_app: start,
        app: None,
    };
    let mut ended = None;
    loop {
        let now = Instant::now();
        // A newer capture of the id replaced this one: nothing of it may be written.
        let Some(flags) = control(id, generation) else {
            return;
        };
        capture.gate |= flags.window;
        if flags.stop {
            ended = Some("stopped".to_owned());
            break;
        }
        if now >= until {
            break;
        }
        if now >= capture.next_app {
            capture.next_app = now + rescan;
            capture.app = capture.bus.vendor_app();
        }
        // The brand's app holds the device: an open gate ends here; a closed one keeps scanning, so
        // a popup id offered while the app is open still says so.
        if capture.gate && capture.app.is_some() {
            break;
        }
        capture.take_keys();
        if flags.rescan {
            capture.rescan_asked = true;
            capture.next_scan = capture.next_scan.max(now + REARRIVAL);
        }
        // ponytail: lists HID every rescan until stable, per plugged candidate.
        let periodic = capture.stable < 2 || capture.last.is_some_and(|(_, retry)| retry);
        if capture.read_failed || ((periodic || capture.rescan_asked) && now >= capture.next_scan) {
            capture.next_scan = now + rescan;
            capture.rescan_asked = false;
            if capture.scan() {
                ended = Some("device left".to_owned());
                break;
            }
        }
        if capture.open.is_empty() {
            thread::sleep(SLICE);
            continue;
        }
        capture.read_open();
    }
    let ran = start.elapsed();
    capture.open.clear();
    // ponytail: a re-plug in this ~750 ms tail only nudges the exiting loop and gets no capture;
    // add a `closing` flag to the entry so `claim_in` replaces it if owners hit this.
    capture.last_keys();
    capture.finish(ended, ran);
}

impl<B: Bus> Capture<'_, B> {
    /// A key typed up to `KEY_WINDOW` after the last read still drops that frame: waits, then
    /// takes the keys once more (the report waits out `GRACE`, which covers it).
    fn last_keys(&mut self) {
        if self.watching {
            thread::sleep(KEY_WINDOW);
            self.take_keys();
        }
    }

    /// Keyboard events since the last call: kept frames whose arrival span lies near one lose
    /// their bytes, and frames read later are checked against them.
    fn take_keys(&mut self) {
        if !self.watching {
            return;
        }
        for key in self.bus.key_times(self.key_cursor) {
            self.key_cursor = key;
            let at = key.saturating_duration_since(self.start);
            self.keys.push(at);
            update(self.id, self.generation, |listen| {
                for heard in &mut listen.heard {
                    heard.frames.key(at);
                }
            });
        }
    }

    /// One listing: true when the device left. A listing that failed decides nothing; otherwise
    /// it may open the popup gate and take a new inventory.
    fn scan(&mut self) -> bool {
        self.read_failed = false;
        let scan = self.bus.scan();
        if !scan.listed {
            self.list_failed = true;
            return false;
        }
        self.list_failed = false;
        if device_left(self.seen, scan.present) {
            return true;
        }
        self.seen |= scan.present;
        self.gate |= scan.offered;
        self.stable = if self.prev == Some(scan.counts) {
            self.stable + 1
        } else {
            1
        };
        self.prev = Some(scan.counts);
        if self.gate && self.app.is_none() && inventory_due(self.last, scan.counts) {
            let retry = self.inventory();
            self.last = Some((scan.counts, retry));
        }
        false
    }

    /// Opens the new collections (after the key filter starts, for a keyboard id); true when
    /// something failed, so the inventory is taken again.
    fn inventory(&mut self) -> bool {
        let collections = self.bus.collections();
        let mut retry = collections
            .iter()
            .any(|collection| matches!(collection.access, Access::Failed(_)));
        self.needs_keys |= collections
            .iter()
            .any(|collection| is_keyboard(&collection.shape));
        if self.needs_keys && !self.watching {
            // Frames read before the filter ran may hold typed keys.
            update(self.id, self.generation, |listen| {
                for heard in &mut listen.heard {
                    heard.frames.untrust();
                }
            });
            match self.bus.watch_keys() {
                Ok(()) => {
                    self.watching = true;
                    self.filter_error = None;
                    // A key at watch start: frames that may have arrived before it keep no bytes.
                    self.keys.push(self.start.elapsed());
                }
                Err(why) => {
                    self.filter_error = Some(why);
                    retry = true;
                    // Closed until the filter runs; their paths open again then.
                    let closed: Vec<String> =
                        self.open.drain(..).map(|target| target.path).collect();
                    self.tried.retain(|path| !closed.contains(path));
                }
            }
        }
        if !self.needs_keys || self.watching {
            for collection in new_collections(&collections, &self.tried) {
                retry |= !self.open_collection(collection);
            }
        }
        retry
    }

    /// Opens one collection for reading under its `heard` entry; false when the open failed (it
    /// is tried again at the next inventory).
    fn open_collection(&mut self, collection: &HidCollection) -> bool {
        let index = match self.paths.iter().position(|path| *path == collection.path) {
            Some(index) => index,
            None => {
                self.paths.push(collection.path.clone());
                let label = interface_label(&collection.shape);
                update(self.id, self.generation, |listen| {
                    listen.heard.push(Heard {
                        label,
                        frames: Frames::default(),
                        error: None,
                    });
                });
                self.paths.len() - 1
            }
        };
        // Taken before the open: reports buffered from the open on fall inside the first span.
        let at = self.start.elapsed();
        let error = match self.bus.open(collection) {
            Ok(reader) => {
                self.tried.push(collection.path.clone());
                self.open.push(Open {
                    heard: index,
                    path: collection.path.clone(),
                    reader,
                    empty_at: at,
                });
                self.ever_opened = true;
                None
            }
            Err(error) => Some(format!("open failed: {error:#}")),
        };
        let opened = error.is_none();
        update(self.id, self.generation, |listen| {
            if let Some(heard) = listen.heard.get_mut(index) {
                heard.error = error;
            }
        });
        opened
    }

    /// One read round over the open collections, published once per collection; a collection
    /// whose read failed is closed for this capture and the devices are listed again.
    fn read_open(&mut self) {
        let (id, generation, start) = (self.id, self.generation, self.start);
        let keys = &self.keys;
        let read_failed = &mut self.read_failed;
        self.open.retain_mut(|target| {
            let mut frames = Vec::new();
            let error = read_target(target, start, &mut frames)
                .err()
                .map(|error| format!("read failed: {error:#}"));
            let failed = error.is_some();
            if !frames.is_empty() || failed {
                update(id, generation, |listen| {
                    let Some(heard) = listen.heard.get_mut(target.heard) else {
                        return;
                    };
                    for (from, to, bytes) in &frames {
                        heard.frames.record(*from, *to, bytes, keys);
                    }
                    if failed {
                        heard.error = error;
                    }
                });
            }
            *read_failed |= failed;
            !failed
        });
    }

    /// The final state after `ran`; a popup capture that never passed its gate leaves no entry.
    /// Only counts reach the log, never bytes.
    fn finish(self, ended: Option<String>, ran: Duration) {
        if !self.gate {
            forget(self.id, self.generation);
            return;
        }
        let filter_failed = (self.needs_keys && !self.watching)
            .then(|| self.filter_error.clone().unwrap_or_default());
        let unlisted = !self.ever_opened && self.list_failed;
        let app = self.app;
        let mut summary = None;
        update(self.id, self.generation, |listen| {
            if matches!(listen.state, State::Listening { .. }) {
                // The app is open: nothing heard means it held the device before anything opened.
                listen.state = match (app, filter_failed) {
                    (Some(app), _) if listen.heard.is_empty() => {
                        State::NotRun(format!("{app} open"))
                    }
                    (Some(app), _) => State::Done {
                        ran,
                        ended: Some(format!("{app} opened")),
                    },
                    (None, Some(why)) => State::NotRun(format!("keys cannot be filtered: {why}")),
                    (None, None) if unlisted => {
                        State::NotRun("HID devices could not be listed".to_owned())
                    }
                    (None, None) => State::Done { ran, ended },
                };
            }
            let (reports, typed) = listen.heard.iter().fold((0u32, 0u32), |(r, t), heard| {
                (
                    r.saturating_add(heard.frames.total),
                    t.saturating_add(heard.frames.typed),
                )
            });
            summary = Some((reports, typed, listen.heard.len()));
        });
        if let Some((reports, typed, collections)) = summary {
            log::line(&format!(
                "listen {}: {reports} report(s), {typed} typed, on {collections} collection(s)",
                id_text(self.id)
            ));
        }
    }
}

/// One `SLICE` read, then up to `DRAIN` quick reads, into `frames` as (arrived after, read at,
/// bytes); the time is taken before each read, so an empty one proves nothing was buffered then.
fn read_target(
    target: &mut Open,
    start: Instant,
    frames: &mut Vec<(Duration, Duration, Vec<u8>)>,
) -> anyhow::Result<()> {
    for read in 0..=DRAIN {
        let at = start.elapsed();
        let timeout = if read == 0 { SLICE } else { DRAIN_SLICE };
        let Some(bytes) = target.reader.read(timeout)? else {
            target.empty_at = at;
            break;
        };
        frames.push((target.empty_at, start.elapsed(), bytes));
    }
    Ok(())
}

/// Who starts a capture: the Add device window opens at once; the popup path opens nothing
/// until the device lists offer the id (`plug::offers` under these settings).
#[cfg(windows)]
#[derive(Debug)]
pub enum Origin {
    Window,
    Popup(Settings),
}

/// Starts listening to `id` on its own `listen VID:PID` thread, or nudges the capture already
/// listening; never blocks the caller beyond the store lock.
#[cfg(windows)]
pub fn start(id: Id, origin: Origin) {
    let now = Instant::now();
    let Some(generation) = claim(id, now, matches!(origin, Origin::Window)) else {
        return;
    };
    let settings = match origin {
        Origin::Window => None,
        Origin::Popup(settings) => Some(settings),
    };
    let spawned = thread::Builder::new()
        .name(format!("listen {}", id_text(id)))
        .spawn(move || {
            let mut bus = DoorBus {
                id,
                settings,
                listing: None,
                inventory: None,
                keys: None,
            };
            run(&mut bus, id, generation, now, now + LISTEN, RESCAN);
        });
    if let Err(error) = spawned {
        update(id, generation, |listen| {
            listen.state = State::NotRun(format!("could not start: {error}"));
        });
    }
}

/// The devices through `door`, read-only: listings, the id's inventory and its opened targets.
#[cfg(windows)]
struct DoorBus {
    id: Id,
    /// Popup path only: the settings `offers` checks the id against.
    settings: Option<Settings>,
    /// The last scan's listing, until an inventory takes it.
    listing: Option<Listing>,
    /// The inventory targets open from (the door re-checks the open rule there).
    inventory: Option<Inventory>,
    keys: Option<keys::Lease>,
}

#[cfg(windows)]
impl Bus for DoorBus {
    fn scan(&mut self) -> Scan {
        let listing = door::list();
        let listed = !listing
            .errors()
            .iter()
            .any(|error| error.starts_with("HID devices could not be listed"));
        let present = !listing.present(&[self.id]).is_empty();
        let all = listing.present_all();
        let counts = all
            .iter()
            .find(|presence| (presence.vid, presence.pid) == self.id)
            .map_or((0, 0), |presence| (presence.hid, presence.openable));
        let offered = self.settings.as_ref().is_some_and(|settings| {
            !offers(&all, &[self.id], settings, |(vid, pid)| {
                listing.product(vid, pid)
            })
            .is_empty()
        });
        self.listing = Some(listing);
        Scan {
            listed,
            present,
            counts,
            offered,
        }
    }

    fn collections(&mut self) -> Vec<HidCollection> {
        let Some(listing) = self.listing.take() else {
            return Vec::new();
        };
        let inventory = listing.into_inventory(&[self.id]);
        let hid = inventory.hid.clone();
        self.inventory = Some(inventory);
        hid
    }

    fn open(&mut self, collection: &HidCollection) -> anyhow::Result<Box<dyn Reader>> {
        let Some(inventory) = &self.inventory else {
            anyhow::bail!("no inventory taken");
        };
        Ok(Box::new(inventory.open_hid(collection)?))
    }

    fn vendor_app(&mut self) -> Option<&'static str> {
        paused_by(self.id.0, &running(&[self.id.0]))
    }

    fn watch_keys(&mut self) -> Result<(), String> {
        if self.keys.is_none() {
            self.keys = Some(keys::acquire()?);
        }
        Ok(())
    }

    fn key_times(&mut self, after: Instant) -> Vec<Instant> {
        keys::times_after(after)
    }
}

#[cfg(windows)]
impl Reader for Target {
    fn read(&mut self, timeout: Duration) -> anyhow::Result<Option<Vec<u8>>> {
        self.read_once(timeout)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, VecDeque};
    use std::rc::Rc;

    use protocol::ReportSize;

    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    fn opened() -> Access {
        Access::Opened {
            descriptor: Vec::new(),
            parse_error: None,
        }
    }

    fn collection(path: &str, page: u16, usage: u16, access: Access) -> HidCollection {
        HidCollection {
            vid: 0x1234,
            pid: 0x5678,
            bus: "Usb".into(),
            manufacturer: None,
            product: None,
            has_serial: false,
            path: path.into(),
            shape: HidShape {
                interface: Some(1),
                usage_page: page,
                usage,
                reports: Vec::new(),
            },
            access,
        }
    }

    fn vendor(path: &str) -> HidCollection {
        collection(path, 0xFF00, 0x0001, opened())
    }

    fn keyboard(usage: u16) -> HidCollection {
        collection("K", 0x0001, usage, Access::Skipped("system keyboard"))
    }

    fn with_reports(mut collection: HidCollection, kinds: &[ReportKind]) -> HidCollection {
        collection.shape.reports = kinds
            .iter()
            .map(|&kind| ReportSize {
                kind,
                id: 0,
                bytes: 8,
            })
            .collect();
        collection
    }

    fn kept(bytes: &[u8], len: usize, count: u32, from: u64, first: u64) -> Kept {
        Kept {
            bytes: bytes.to_vec(),
            len,
            count,
            from: ms(from),
            first: ms(first),
        }
    }

    /// Records `bytes` read at `at` with no keys, right after an empty read.
    fn at(frames: &mut Frames, millis: u64, bytes: &[u8]) {
        frames.record(ms(millis), ms(millis), bytes, &[]);
    }

    #[test]
    fn frames_dedupe_on_first_32_bytes_and_keep_four() {
        let mut frames = Frames::default();
        at(&mut frames, 5, &[1, 2, 3]);
        at(&mut frames, 7, &[1, 2, 3]);
        at(&mut frames, 9, &[4]);
        assert_eq!(frames.total, 3);
        assert_eq!(
            frames.kept,
            [kept(&[1, 2, 3], 3, 2, 5, 5), kept(&[4], 1, 1, 9, 9)]
        );
        assert_eq!(frames.other, 0);

        let mut long = Frames::default();
        let mut a = [7u8; 40];
        let mut b = [7u8; 40];
        a[35] = 1;
        b[35] = 2;
        at(&mut long, 1, &a);
        at(&mut long, 2, &b);
        assert_eq!(long.kept.len(), 1);
        assert_eq!(long.kept[0].count, 2);
        assert_eq!(long.kept[0].bytes.len(), 32);
        assert_eq!(long.kept[0].len, 40);

        at(&mut frames, 10, &[10]);
        at(&mut frames, 11, &[11]);
        assert_eq!(frames.kept.len(), 4);
        at(&mut frames, 12, &[12]);
        assert_eq!(frames.other, 1);
        at(&mut frames, 13, &[12]);
        assert_eq!(frames.other, 2);
        at(&mut frames, 14, &[1, 2, 3]);
        assert_eq!(frames.kept[0].count, 3);
    }

    #[test]
    fn typed_frames_are_dropped_around_key_events() {
        let window = ms(500);
        assert!(near_key(ms(1000), ms(1000), &[ms(1500)], window));
        assert!(!near_key(ms(1000), ms(1000), &[ms(1501)], window));
        assert!(!near_key(ms(1000), ms(1000), &[], window));
        assert!(near_key(ms(1000), ms(1000), &[ms(500)], window));
        assert!(!near_key(ms(1000), ms(1000), &[ms(499)], window));
        // A key inside a buffered span counts even with no window.
        assert!(near_key(ms(0), ms(2000), &[ms(1000)], ms(0)));

        let mut frames = Frames::default();
        frames.record(ms(0), ms(100), &[1], &[]);
        assert_eq!(frames.kept.len(), 1);
        frames.key(ms(600));
        assert!(frames.kept.is_empty());
        assert_eq!((frames.typed, frames.total), (1, 1));
        frames.record(ms(2000), ms(2000), &[2], &[ms(600)]);
        frames.record(ms(2100), ms(2100), &[3], &[ms(600), ms(2500)]);
        assert_eq!(frames.typed, 2);
        assert_eq!(frames.kept, [kept(&[2], 1, 1, 2000, 2000)]);
        assert_eq!(frames.total, 3);
    }

    #[test]
    fn untrust_moves_everything_to_typed() {
        let mut frames = Frames {
            total: 5,
            kept: vec![kept(&[1], 1, 2, 0, 0), kept(&[2], 1, 1, 0, 0)],
            other: 1,
            typed: 1,
        };
        frames.untrust();
        assert_eq!(frames.typed, 5);
        assert!(frames.kept.is_empty());
        assert_eq!(frames.other, 0);
        assert_eq!(frames.total, 5);
    }

    fn listen(state: State, started: Instant, heard: Vec<Heard>) -> Listen {
        Listen {
            state,
            started,
            heard,
        }
    }

    /// A capture listening from `started` until `until`, nothing heard yet.
    pub(crate) fn listening(started: Instant, until: Instant) -> Listen {
        listen(State::Listening { until }, started, Vec::new())
    }

    /// A capture that ran `secs` from `started`.
    pub(crate) fn done(
        started: Instant,
        secs: u64,
        ended: Option<&str>,
        heard: Vec<Heard>,
    ) -> Listen {
        let state = State::Done {
            ran: Duration::from_secs(secs),
            ended: ended.map(str::to_owned),
        };
        listen(state, started, heard)
    }

    pub(crate) fn heard(label: &str, frames: Frames) -> Heard {
        Heard {
            label: label.to_owned(),
            frames,
            error: None,
        }
    }

    #[test]
    fn report_lines_states() {
        let t = Instant::now();
        let secs = Duration::from_secs;
        assert_eq!(
            report_lines(None, t, None),
            ["Listen: not run (no plug-in seen)"]
        );
        let not_run = listen(State::NotRun("ATK HUB open".into()), t, Vec::new());
        assert_eq!(
            report_lines(Some(&not_run), t, None),
            ["Listen: not run (ATK HUB open)"]
        );
        let running = listening(t, t + secs(10));
        assert_eq!(
            report_lines(Some(&running), t, None),
            ["Listen: still listening"]
        );
        assert_eq!(
            report_lines(Some(&running), t + secs(12), None),
            ["Listen: did not finish"]
        );

        let first = Frames {
            total: 2,
            kept: vec![kept(&[0xc0, 0x01, 0x52], 3, 2, 120, 120)],
            ..Frames::default()
        };
        // Without its last key check, a capture that did not finish shows counts, never bytes.
        let unfinished = Listen {
            heard: vec![heard("if 2 FF00:0001", first.clone())],
            ..listening(t, t + secs(10))
        };
        assert_eq!(
            report_lines(Some(&unfinished), t + secs(12), None),
            ["Listen: did not finish", "  if 2 FF00:0001: 2 report(s)"]
        );
        let second = Heard {
            error: Some("read failed: gone".into()),
            ..heard(
                "if 1 FF02:0002",
                Frames {
                    total: 1,
                    typed: 1,
                    ..Frames::default()
                },
            )
        };
        let capture = done(
            t,
            12,
            Some("stopped"),
            vec![heard("if 2 FF00:0001", first), second],
        );
        assert_eq!(
            report_lines(Some(&capture), t + secs(20), Some(t + ms(5300))),
            [
                "Listen: 12 s after plug-in, read-only (stopped); Try sent at +5300 ms",
                "  if 2 FF00:0001: 2 report(s)",
                "    +120 ms x2 rx (3 B): c0 01 52",
                "  if 1 FF02:0002: 1 report(s)",
                "    1 report(s) dropped: keys were typed",
                "    read failed: gone",
            ]
        );
        let late = report_lines(Some(&capture), t + secs(20), Some(t + secs(13)));
        assert_eq!(late[0], "Listen: 12 s after plug-in, read-only (stopped)");

        let long = Frames {
            total: 4,
            kept: vec![kept(&[0xab; 32], 64, 1, 7, 7)],
            other: 3,
            ..Frames::default()
        };
        let capture = done(t, 30, None, vec![heard("if 0 FF00:0001", long)]);
        let lines = report_lines(Some(&capture), t, None);
        assert!(lines[2].contains("rx (32 of 64 B):"), "{lines:?}");
        assert_eq!(lines[3], "    3 other report(s)");
        let silent = done(
            t,
            30,
            None,
            vec![heard("if 0 FF00:0001", Frames::default())],
        );
        assert_eq!(
            report_lines(Some(&silent), t, None)[1],
            "  if 0 FF00:0001: no reports"
        );
    }

    /// A `Done` capture of 6 collections with 4 long frames each.
    pub(crate) fn long_listen(started: Instant) -> Listen {
        let frames = Frames {
            total: 4,
            kept: (0u8..4).map(|n| kept(&[n; 32], 64, 1, 7, 7)).collect(),
            ..Frames::default()
        };
        let collections = (0..6)
            .map(|n| heard(&format!("if {n} FF00:0001"), frames.clone()))
            .collect();
        done(started, 30, None, collections)
    }

    #[test]
    fn report_lines_cut_at_2000_bytes() {
        let t = Instant::now();
        let lines = report_lines(Some(&long_listen(t)), t, None);
        assert!(lines.join("\n").len() <= 2000);
        assert!(lines[0].starts_with("Listen: 30 s"), "{lines:?}");
        assert_eq!(lines.last().map(String::as_str), Some("  … cut"));

        let short = done(
            t,
            30,
            None,
            vec![heard("if 0 FF00:0001", Frames::default())],
        );
        assert!(!report_lines(Some(&short), t, None).contains(&CUT.to_owned()));
        let empty = done(t, 30, None, Vec::new());
        assert_eq!(
            report_lines(Some(&empty), t, None),
            [
                "Listen: 30 s after plug-in, read-only",
                "  nothing allowed to open"
            ]
        );
    }

    #[test]
    fn left_ends_at_grace() {
        let t = Instant::now();
        let secs = Duration::from_secs;
        let capture = listening(t, t + secs(10));
        assert_eq!(capture.left(t), Some(secs(10)));
        assert_eq!(capture.left(t + secs(11)), Some(Duration::ZERO));
        assert_eq!(capture.left(t + secs(12)), None);
        assert_eq!(done(t, 1, None, Vec::new()).left(t), None);
        assert_eq!(
            listen(State::NotRun("x".into()), t, Vec::new()).left(t),
            None
        );
    }

    #[test]
    fn new_collections_skip_closed_open_output_only_and_repeated_paths() {
        let hid = [
            vendor("A"),
            collection("B", 0x0001, 0x0002, Access::Skipped("system mouse")),
            collection("C", 0xFF00, 0x0001, Access::Failed("x".into())),
            collection("A", 0xFF00, 0x0002, opened()),
            vendor("D"),
            with_reports(vendor("E"), &[ReportKind::Output]),
            with_reports(vendor("F"), &[]),
            with_reports(vendor("G"), &[ReportKind::Output, ReportKind::Input]),
        ];
        let picked: Vec<&str> = new_collections(&hid, &["D".to_owned()])
            .iter()
            .map(|collection| collection.path.as_str())
            .collect();
        assert_eq!(picked, ["A", "F", "G"]);
    }

    #[test]
    fn device_left_only_after_seen() {
        assert!(!device_left(false, false));
        assert!(!device_left(false, true));
        assert!(!device_left(true, true));
        assert!(device_left(true, false));
    }

    #[test]
    fn inventory_due_on_change_or_failure() {
        assert!(inventory_due(None, (0, 0)));
        assert!(!inventory_due(Some(((3, 2), false)), (3, 2)));
        // A keyboard collection appeared.
        assert!(inventory_due(Some(((3, 2), false)), (4, 2)));
        assert!(inventory_due(Some(((3, 2), false)), (3, 3)));
        assert!(inventory_due(Some(((3, 2), true)), (3, 2)));
    }

    #[test]
    fn claim_generations_ignore_stale_threads() {
        let mut s = Store::new();
        let id = (0x1111, 0x0001);
        let t = Instant::now();
        let g1 = claim_in(&mut s, id, t, false).unwrap();
        assert_eq!(claim_in(&mut s, id, t, true), None);
        assert_eq!(
            control_in(&mut s, id, g1),
            Some(Control {
                stop: false,
                rescan: true,
                window: true
            })
        );
        assert!(!control_in(&mut s, id, g1).unwrap().rescan);
        let g2 = claim_in(&mut s, id, t + LISTEN + GRACE, false).unwrap();
        assert_ne!(g2, g1);
        update_in(&mut s, id, g1, |listen| {
            listen.state = done(t, 1, None, Vec::new()).state
        });
        assert!(matches!(s[&id].listen.state, State::Listening { .. }));
        assert_eq!(control_in(&mut s, id, g1), None);
        stop_in(&mut s, id);
        assert!(control_in(&mut s, id, g2).unwrap().stop);
        forget_in(&mut s, id, g1);
        assert!(s.contains_key(&id));
        forget_in(&mut s, id, g2);
        assert!(!s.contains_key(&id));
    }

    #[test]
    fn nudge_only_rescans_a_running_capture() {
        let mut s = Store::new();
        let (running, ended) = ((0x1111, 0x0021), (0x1111, 0x0022));
        let t = Instant::now();
        let g1 = claim_in(&mut s, running, t, true).unwrap();
        let g2 = claim_in(&mut s, ended, t, true).unwrap();
        update_in(&mut s, ended, g2, |listen| {
            listen.state = done(t, 1, None, Vec::new()).state;
        });
        nudge_in(&mut s, running);
        nudge_in(&mut s, ended);
        nudge_in(&mut s, (0x1111, 0x0023));
        assert!(control_in(&mut s, running, g1).unwrap().rescan);
        assert!(!control_in(&mut s, ended, g2).unwrap().rescan);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn claim_evicts_finished_entries_and_refuses_recovery() {
        let mut s = Store::new();
        let (a, b, c) = ((0x1111, 0x000A), (0x1111, 0x000B), (0x1111, 0x000C));
        let t = Instant::now();
        let ga = claim_in(&mut s, a, t, true).unwrap();
        update_in(&mut s, a, ga, |listen| {
            listen.state = done(t, 30, None, Vec::new()).state
        });
        claim_in(&mut s, b, t + EVICT - ms(1), true).unwrap();
        assert!(s.contains_key(&a));
        claim_in(&mut s, c, t + EVICT, true).unwrap();
        assert!(!s.contains_key(&a));
        assert!(matches!(s[&b].listen.state, State::Listening { .. }));
        assert_eq!(claim_in(&mut s, (0x3554, 0xF2FF), t, true), None);
    }

    /// One scripted read: sleep this long, then this result (`Err` = the error text).
    type Read = (Duration, Result<Option<Vec<u8>>, String>);

    fn frame(bytes: &[u8]) -> Read {
        (Duration::ZERO, Ok(Some(bytes.to_vec())))
    }

    fn empty(sleep: Duration) -> Read {
        (sleep, Ok(None))
    }

    /// What the readers did: each read's timeout and when the last empty and frame reads ended.
    #[derive(Debug, Default)]
    struct Track {
        timeouts: RefCell<Vec<Duration>>,
        last_empty: Cell<Option<Instant>>,
        last_frame: Cell<Option<Instant>>,
    }

    /// Plays back a path's reads; past the script it sleeps 1 ms and reads nothing.
    struct FakeReader {
        script: VecDeque<Read>,
        track: Rc<Track>,
    }

    impl Reader for FakeReader {
        fn read(&mut self, timeout: Duration) -> anyhow::Result<Option<Vec<u8>>> {
            self.track.timeouts.borrow_mut().push(timeout);
            let (sleep, result) = self.script.pop_front().unwrap_or_else(|| empty(ms(1)));
            thread::sleep(sleep);
            match &result {
                Ok(None) => self.track.last_empty.set(Some(Instant::now())),
                Ok(Some(_)) => self.track.last_frame.set(Some(Instant::now())),
                Err(_) => {}
            }
            result.map_err(|why| anyhow::anyhow!(why))
        }
    }

    /// The key event a fake injects.
    #[derive(Clone, Copy)]
    enum Key {
        Now,
        /// 10 ms after the last empty read.
        AfterEmpty,
        /// 100 ms after the last frame was read.
        AfterFrame,
    }

    /// A scripted device (the last scan, inventory and watch result repeat); one key event at the
    /// first `key_times` call once the readers made n reads. Nothing here can write.
    struct FakeBus {
        scans: VecDeque<Scan>,
        inventories: VecDeque<Vec<HidCollection>>,
        reads: HashMap<String, VecDeque<Read>>,
        app_from: Option<(usize, &'static str)>,
        watch: VecDeque<Result<(), String>>,
        inject: Option<(usize, Key)>,
        track: Rc<Track>,
        app_calls: usize,
        key_calls: usize,
        opened: Vec<String>,
        scans_done: usize,
        watch_calls: usize,
    }

    fn present(counts: (usize, usize)) -> Scan {
        Scan {
            listed: true,
            present: true,
            counts,
            offered: false,
        }
    }

    fn fake(inventory: Vec<HidCollection>, reads: &[(&str, Vec<Read>)]) -> FakeBus {
        FakeBus {
            scans: VecDeque::from([present((1, 1))]),
            inventories: VecDeque::from([inventory]),
            reads: reads
                .iter()
                .map(|(path, script)| ((*path).to_owned(), script.iter().cloned().collect()))
                .collect(),
            app_from: None,
            watch: VecDeque::from([Ok(())]),
            inject: None,
            track: Rc::default(),
            app_calls: 0,
            key_calls: 0,
            opened: Vec::new(),
            scans_done: 0,
            watch_calls: 0,
        }
    }

    /// The front entry, or the last one again once only it is left.
    fn next<T: Clone>(queue: &mut VecDeque<T>) -> T {
        if queue.len() > 1 {
            queue.pop_front().expect("a scripted entry")
        } else {
            queue.front().cloned().expect("a scripted entry")
        }
    }

    impl Bus for FakeBus {
        fn scan(&mut self) -> Scan {
            self.scans_done += 1;
            next(&mut self.scans)
        }

        fn collections(&mut self) -> Vec<HidCollection> {
            next(&mut self.inventories)
        }

        fn open(&mut self, collection: &HidCollection) -> anyhow::Result<Box<dyn Reader>> {
            self.opened.push(collection.path.clone());
            Ok(Box::new(FakeReader {
                script: self.reads.remove(&collection.path).unwrap_or_default(),
                track: Rc::clone(&self.track),
            }))
        }

        fn vendor_app(&mut self) -> Option<&'static str> {
            self.app_calls += 1;
            self.app_from
                .filter(|(from, _)| self.app_calls >= *from)
                .map(|(_, app)| app)
        }

        fn watch_keys(&mut self) -> Result<(), String> {
            self.watch_calls += 1;
            next(&mut self.watch)
        }

        fn key_times(&mut self, _after: Instant) -> Vec<Instant> {
            self.key_calls += 1;
            let reads = self.track.timeouts.borrow().len();
            let Some((_, key)) = self.inject.take_if(|(after, _)| reads >= *after) else {
                return Vec::new();
            };
            vec![match key {
                Key::Now => Instant::now(),
                Key::AfterEmpty => self.track.last_empty.get().expect("an empty read") + ms(10),
                Key::AfterFrame => self.track.last_frame.get().expect("a frame") + ms(100),
            }]
        }
    }

    /// Claims `id` (from a window or the popup path, [Stop] pressed first if `stopped`), runs
    /// `bus` for `length` with a 5 ms rescan and returns the capture.
    fn listen_for(
        bus: &mut FakeBus,
        id: Id,
        length: Duration,
        window: bool,
        stopped: bool,
    ) -> Option<Listen> {
        let t = Instant::now();
        let generation = claim(id, t, window).expect("a fresh id");
        if stopped {
            stop(id);
        }
        run(bus, id, generation, t, t + length, ms(5));
        snapshot(id)
    }

    fn finished(listen: &Listen) -> (Duration, Option<&str>) {
        match &listen.state {
            State::Done { ran, ended } => (*ran, ended.as_deref()),
            other => panic!("not done: {other:?}"),
        }
    }

    #[test]
    fn loop_records_and_finishes_done() {
        let mut bus = fake(vec![vendor("A")], &[("A", vec![frame(&[1]), frame(&[2])])]);
        let capture = listen_for(&mut bus, (0xF001, 0x0001), ms(150), true, false).unwrap();
        let (ran, ended) = finished(&capture);
        assert_eq!(ended, None);
        assert!(ran >= ms(150));
        assert_eq!(capture.heard[0].frames.total, 2);
        assert_eq!(bus.opened, ["A"]);
    }

    #[test]
    fn drain_reads_quickly_and_stops_at_the_cap() {
        let script: Vec<Read> = (0..70u8).map(|n| frame(&[n])).collect();
        let mut bus = fake(vec![vendor("A")], &[("A", script)]);
        let capture = listen_for(&mut bus, (0xF001, 0x0002), ms(150), true, false).unwrap();
        let mut expected = vec![SLICE];
        expected.extend([DRAIN_SLICE; DRAIN]);
        expected.push(SLICE);
        let timeouts = bus.track.timeouts.borrow();
        assert_eq!(timeouts[..expected.len()], expected[..]);
        assert_eq!(capture.heard[0].frames.total, 70);
    }

    #[test]
    fn stop_ends_the_capture() {
        let mut bus = fake(vec![vendor("A")], &[]);
        let capture = listen_for(&mut bus, (0xF001, 0x0003), ms(150), true, true).unwrap();
        let (ran, ended) = finished(&capture);
        assert_eq!(ended, Some("stopped"));
        assert!(ran < ms(150));
    }

    #[test]
    fn loop_ends_when_the_device_left() {
        let absent = Scan {
            present: false,
            ..present((0, 0))
        };
        // The counts change each time, so `stable` never reaches 2 and the third scan runs.
        let mut bus = fake(Vec::new(), &[]);
        bus.scans = VecDeque::from([absent, present((2, 1)), absent]);
        let capture = listen_for(&mut bus, (0xF001, 0x0004), ms(150), true, false).unwrap();
        assert_eq!(finished(&capture).1, Some("device left"));

        let mut bus = fake(Vec::new(), &[]);
        bus.scans = VecDeque::from([absent]);
        let capture = listen_for(&mut bus, (0xF001, 0x0005), ms(150), true, false).unwrap();
        assert_eq!(finished(&capture).1, None);
    }

    #[test]
    fn vendor_app_open_or_opened() {
        let mut bus = fake(vec![vendor("A")], &[]);
        bus.app_from = Some((1, "ATK HUB"));
        let capture = listen_for(&mut bus, (0xF001, 0x0006), ms(150), true, false).unwrap();
        assert_eq!(capture.state, State::NotRun("ATK HUB open".into()));
        assert!(bus.opened.is_empty());

        let mut bus = fake(vec![vendor("A")], &[]);
        bus.app_from = Some((3, "ATK HUB"));
        let capture = listen_for(&mut bus, (0xF001, 0x0007), ms(150), true, false).unwrap();
        assert_eq!(finished(&capture).1, Some("ATK HUB opened"));
        assert_eq!(bus.opened, ["A"]);

        // A popup capture offered while the app is open says so and opens nothing.
        let offered = |offered| Scan {
            offered,
            ..present((1, 1))
        };
        let mut bus = fake(vec![vendor("A")], &[]);
        bus.app_from = Some((1, "ATK HUB"));
        bus.scans = VecDeque::from([offered(false), offered(true)]);
        let capture = listen_for(&mut bus, (0xF001, 0x0011), ms(150), false, false).unwrap();
        assert_eq!(capture.state, State::NotRun("ATK HUB open".into()));
        assert!(bus.opened.is_empty());

        // A popup capture that was never offered leaves nothing behind.
        let mut bus = fake(vec![vendor("A")], &[]);
        bus.app_from = Some((1, "ATK HUB"));
        let capture = listen_for(&mut bus, (0xF001, 0x0008), ms(150), false, false);
        assert_eq!(capture, None);
    }

    #[test]
    fn failed_then_ok_is_retried() {
        let mut bus = fake(Vec::new(), &[("A", vec![frame(&[1])])]);
        let failed = collection("A", 0xFF00, 0x0001, Access::Failed("busy".into()));
        bus.inventories = VecDeque::from([vec![failed], vec![vendor("A")]]);
        let capture = listen_for(&mut bus, (0xF001, 0x0009), ms(150), true, false).unwrap();
        assert_eq!(bus.opened, ["A"]);
        assert_eq!(capture.heard[0].frames.total, 1);
        assert_eq!(capture.heard[0].error, None);
    }

    #[test]
    fn read_error_closes_the_collection() {
        let gone = (Duration::ZERO, Err("gone".to_owned()));
        let mut bus = fake(vec![vendor("A")], &[("A", vec![gone])]);
        let capture = listen_for(&mut bus, (0xF001, 0x000A), ms(150), true, false).unwrap();
        assert_eq!(capture.heard[0].error.as_deref(), Some("read failed: gone"));
        assert_eq!(bus.opened, ["A"]);
    }

    #[test]
    fn periodic_rescans_stop_once_stable() {
        let mut bus = fake(vec![vendor("A")], &[]);
        listen_for(&mut bus, (0xF001, 0x000B), ms(150), true, false).unwrap();
        assert_eq!(bus.scans_done, 2);
    }

    #[test]
    fn popup_capture_opens_only_once_offered() {
        let offered = |offered| Scan {
            offered,
            ..present((1, 1))
        };
        let popup = |bus: &mut FakeBus, id: Id| listen_for(bus, id, ms(150), false, false);
        let mut bus = fake(vec![vendor("A")], &[]);
        bus.scans = VecDeque::from([offered(false), offered(true)]);
        assert!(popup(&mut bus, (0xF001, 0x000C)).is_some());
        assert_eq!(bus.opened, ["A"]);

        let mut bus = fake(vec![vendor("A")], &[]);
        bus.scans = VecDeque::from([offered(false)]);
        assert_eq!(popup(&mut bus, (0xF001, 0x000D)), None);
        assert!(bus.opened.is_empty());

        let mut bus = fake(vec![vendor("A")], &[]);
        bus.scans = VecDeque::from([offered(false)]);
        listen_for(&mut bus, (0xF001, 0x000E), ms(150), true, false).unwrap();
        assert_eq!(bus.opened, ["A"]);
    }

    #[test]
    fn listing_failure_without_open_is_not_run() {
        let unlisted = Scan {
            listed: false,
            ..present((0, 0))
        };
        let mut bus = fake(vec![vendor("A")], &[]);
        bus.scans = VecDeque::from([unlisted]);
        let capture = listen_for(&mut bus, (0xF001, 0x000F), ms(150), true, false).unwrap();
        assert_eq!(
            capture.state,
            State::NotRun("HID devices could not be listed".into())
        );

        // The flag resets once a listing works.
        let mut bus = fake(vec![vendor("A")], &[]);
        bus.scans = VecDeque::from([unlisted, present((1, 1))]);
        let capture = listen_for(&mut bus, (0xF001, 0x0010), ms(150), true, false).unwrap();
        assert_eq!(finished(&capture).1, None);
    }

    #[test]
    fn frames_read_right_after_watch_start_are_typed() {
        for (usage, id) in [(0x0006, (0xF002, 0x0001)), (0x0007, (0xF002, 0x0002))] {
            let reads = [("A", vec![frame(&[1]), frame(&[2])])];
            let mut bus = fake(vec![vendor("A"), keyboard(usage)], &reads);
            let capture = listen_for(&mut bus, id, ms(150), true, false).unwrap();
            let frames = &capture.heard[0].frames;
            assert_eq!(bus.watch_calls, 1);
            assert_eq!((frames.typed, frames.total), (2, 2));
            assert!(frames.kept.is_empty());
        }
        // Without a keyboard collection no filter runs and key events are never read.
        let mut bus = fake(vec![vendor("A")], &[("A", vec![frame(&[1]), frame(&[2])])]);
        bus.inject = Some((0, Key::Now));
        let capture = listen_for(&mut bus, (0xF002, 0x0003), ms(150), true, false).unwrap();
        assert_eq!((bus.watch_calls, bus.key_calls), (0, 0));
        assert_eq!(capture.heard[0].frames.kept.len(), 2);
    }

    /// A's frames after `length`, read beside a keyboard collection, with an optional key event
    /// once A made n reads; the run must include the final `KEY_WINDOW` key check.
    fn run_keyed(
        id: Id,
        script: Vec<Read>,
        inject: Option<(usize, Key)>,
        length: Duration,
    ) -> Frames {
        let mut bus = fake(vec![vendor("A"), keyboard(0x0006)], &[("A", script)]);
        bus.inject = inject;
        let began = Instant::now();
        let mut capture = listen_for(&mut bus, id, length, true, false).expect("a capture");
        assert!(began.elapsed() >= length + KEY_WINDOW);
        capture.heard.swap_remove(0).frames
    }

    #[test]
    fn loop_drops_frames_near_key_events() {
        // The third, instant empty read moves `empty_at` past 600 ms, so both frames are read
        // over 500 ms after the synthetic key at watch start.
        let script = || {
            vec![
                empty(Duration::ZERO),
                empty(ms(600)),
                empty(Duration::ZERO),
                frame(&[1]),
                frame(&[2]),
            ]
        };
        // The key comes once f2, the 5th read, was recorded.
        let frames = run_keyed((0xF002, 0x0004), script(), Some((5, Key::Now)), ms(1000));
        assert_eq!(frames.typed, 2);
        assert!(frames.kept.is_empty());

        let frames = run_keyed((0xF002, 0x0005), script(), None, ms(1000));
        assert_eq!(frames.kept.len(), 2);
    }

    #[test]
    fn buffered_frame_spanning_a_key_is_typed() {
        let script = || {
            vec![
                empty(Duration::ZERO),
                empty(ms(600)),
                empty(Duration::ZERO),
                (ms(600), Ok(Some(vec![9]))),
            ]
        };
        // The key comes after the last empty read (the 3rd), before the frame is read.
        let frames = run_keyed(
            (0xF002, 0x0006),
            script(),
            Some((3, Key::AfterEmpty)),
            ms(2000),
        );
        assert_eq!(frames.typed, 1);
        assert!(frames.kept.is_empty());

        let frames = run_keyed((0xF002, 0x0007), script(), None, ms(2000));
        assert_eq!(frames.kept.len(), 1);
    }

    #[test]
    fn keys_seen_after_the_end_still_drop_the_last_frame() {
        // The frame read starts ~700 ms in and returns ~1100 ms, after `until` (900 ms), so only
        // the final key check sees a key typed 100 ms after it; every margin is ~200 ms.
        let script = || {
            vec![
                empty(Duration::ZERO),
                empty(ms(700)),
                empty(Duration::ZERO),
                (ms(400), Ok(Some(vec![7]))),
            ]
        };
        let frames = run_keyed(
            (0xF002, 0x000A),
            script(),
            Some((4, Key::AfterFrame)),
            ms(900),
        );
        assert_eq!(frames.typed, 1);
        assert!(frames.kept.is_empty());

        let frames = run_keyed((0xF002, 0x000B), script(), None, ms(900));
        assert_eq!(frames.kept.len(), 1);
    }

    #[test]
    fn key_filter_failure_opens_nothing() {
        let setup = |watch: Vec<Result<(), String>>| {
            let mut bus = fake(Vec::new(), &[("A", vec![frame(&[1])])]);
            bus.inventories =
                VecDeque::from([vec![vendor("A")], vec![vendor("A"), keyboard(0x0006)]]);
            bus.scans = VecDeque::from([present((1, 1)), present((2, 1))]);
            bus.watch = watch.into();
            bus
        };
        let mut bus = setup(vec![Err("x".into())]);
        let capture = listen_for(&mut bus, (0xF002, 0x0008), ms(150), true, false).unwrap();
        assert_eq!(
            capture.state,
            State::NotRun("keys cannot be filtered: x".into())
        );
        assert!(capture.heard[0].frames.kept.is_empty());
        assert_eq!(capture.heard[0].frames.typed, 1);
        // Read only until the filter failed (a few 1 ms reads), never for the whole 150 ms.
        let reads = bus.track.timeouts.borrow().len();
        assert!(reads < 20, "{reads} reads");

        let mut bus = setup(vec![Err("x".into()), Ok(())]);
        let capture = listen_for(&mut bus, (0xF002, 0x0009), ms(150), true, false).unwrap();
        assert_eq!(bus.opened, ["A", "A"]);
        assert_eq!(finished(&capture).1, None);
        assert_eq!(capture.heard[0].frames.typed, 1);
    }
}
