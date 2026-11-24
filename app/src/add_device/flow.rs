//! The Add device window's steps as a pure state machine: detection from plug-in events, the
//! families a device may be tried as, one try, and what the owner adds or reports. The window
//! feeds it events and performs the effects it returns; nothing here touches a device.

use std::time::{Duration, Instant};

use protocol::{Family, HidShape, Reading, is_recovery, known};

use crate::config::CustomEntry;
use crate::device::{Device, lookup};
use crate::plug::{Arrivals, Id};
use crate::report::{self, Identity};
use crate::scan::reports_text;

/// Detection gives up after this long without a new device.
pub const DETECT_LIMIT: Duration = Duration::from_secs(120);
/// Wait before asking again about a device whose interfaces still change.
const INSPECT_RETRY: Duration = Duration::from_millis(500);
/// Inspections of a device that keeps changing before it counts as unreadable.
const INSPECT_TRIES: u8 = 20;
/// How often the window checks for the worker's reply.
const REPLY_POLL: Duration = Duration::from_millis(100);
const SECOND: Duration = Duration::from_secs(1);
/// [Add] ignores presses this long after the reading, so a double-click or held Enter on [Try]
/// never adds before the owner checked the level.
const ADD_GUARD: Duration = SECOND;

const WARNING: &str = "This device is not officially supported yet. Reading its battery may not \
                       work. If it does not, please report an issue at \
                       https://github.com/ittmchien/juicetray/issues";
const DETECTING: &str = "Detecting… Plug the device in, or unplug it and plug it back in.";
const BLUETOOTH_HINT: &str = "A Bluetooth device connected. Juicetray reads Bluetooth batteries \
                              on its own; this window adds USB receivers and cables only.";
const RECOVERY: &str = "The device is in recovery or update mode; Juicetray never talks to it.";
const NO_FAMILY: &str = "None of its interfaces matches a battery protocol Juicetray knows.";
const NEEDS_CAPTURE: &str = "Its catalogue model needs a capture of its own app's traffic before \
                             Juicetray may query it.";
const BITMOUSE_CABLE: &str = "It is a BITMOUSE cable model; a BITMOUSE cable is never queried.";
/// The [Try] label of an off-catalogue BITMOUSE id: pressing it confirms a USB receiver.
const RECEIVER_TRY: &str = "It is a USB receiver — Try";
const UNSTABLE: &str = "Its interfaces kept changing, so they could not be read.";

/// The kind choices of a custom device: label and config value.
pub const KINDS: [(&str, &str); 5] = [
    ("Mouse", "mouse"),
    ("Headset", "headset"),
    ("Keyboard", "keyboard"),
    ("Gamepad", "gamepad"),
    ("Other", "other"),
];
/// The connection choices of a custom device: label and config value.
const CONNECTIONS: [(&str, &str); 2] = [("Receiver (USB dongle)", "receiver"), ("Cable", "cable")];

/// What the refresh worker found on a device's interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inspection {
    Gone,
    /// Its interface count still changes; asked again shortly.
    Settling,
    Ready(Interfaces),
    Failed(String),
}

/// A device's interfaces and the families they match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Interfaces {
    /// `families_for` and the time left of each family's query floor.
    pub families: Vec<(Family, Option<Duration>)>,
    /// The USB product string.
    pub product: Option<String>,
    pub shapes: Vec<HidShape>,
    pub serial: bool,
    /// The device's collections and serial ports as `juicetray scan` prints them (report sizes,
    /// descriptor hex, manufacturer / product); empty when they were not listed.
    pub scan_lines: Vec<String>,
}

/// The result of one try.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TryOutcome {
    Reading(Reading),
    NoReply,
    /// The device answered, but not with a trustworthy level.
    Invalid(String),
    /// Inside the family's query floor; nothing was sent.
    Waiting(Duration),
    Gone,
    /// Refused before the write: nothing was sent.
    Failed(String),
    /// The write was attempted, so the query went out (or may have), then the exchange failed.
    Interrupted(String),
}

/// What the owner adds, sent to the UI thread, which writes the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddRequest {
    /// A catalogue model: its id joins `allow`.
    Allow(Id),
    /// A device outside the catalogue: a `custom_devices` entry.
    Custom(CustomEntry),
}

/// Work the window does for the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Ask the worker for the device's interfaces (`Event::Inspected` answers).
    Inspect(Id),
    /// Ask the worker for one query (`Event::Tried` answers).
    TryAs(Id, Family),
    Add(AddRequest),
    /// Copy the shown details to the clipboard.
    Copy,
    OpenUrl(String),
}

/// The fields the window reads when the owner presses the main button.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    /// The picked list row.
    pub list: usize,
    pub kind: usize,
    pub connection: usize,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The main button (Continue, Next, Try, Add, Copy, Try again).
    Primary(Input),
    /// The second button (Not right, Send report).
    Alt,
    /// The third button (Report on GitHub), shown only for an unsupported device.
    Extra,
    /// Another list row was picked.
    Select(usize),
    Arrived(Id),
    Removed(Id),
    BluetoothArrived,
    /// A popup's [OK] or the window's start with a preselected id.
    Handover(Id),
    Inspected(Inspection),
    Tried(TryOutcome),
    /// The UI thread's answer to `Effect::Add`: the config was written, or why not.
    Saved(Result<(), String>),
    Tick,
}

/// The custom-device fields: the connection choices and the name to start from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub connections: Vec<&'static str>,
    pub name: String,
}

/// Everything the window shows; a hidden part is empty or `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub text: String,
    /// Device or family rows to pick from.
    pub list: Vec<String>,
    pub picked: usize,
    pub form: Option<Form>,
    /// The full report, read-only, for [Copy].
    pub details: Option<String>,
    pub primary: Option<String>,
    pub alt: Option<&'static str>,
    /// A third button after [Alt]; the window shifts [Primary] and [Alt] left to fit it.
    pub extra: Option<&'static str>,
    pub close: &'static str,
}

#[derive(Debug)]
enum Step {
    Warning,
    Detecting {
        since: Instant,
        arrivals: Arrivals,
        bluetooth: bool,
    },
    TimedOut,
    Pick(Vec<Id>),
    AlreadySupported(Id),
    Inspecting {
        id: Id,
        tries: u8,
        /// Set while waiting to ask again; unset while the worker's reply is due.
        again_at: Option<Instant>,
    },
    Choose {
        id: Id,
        /// Each family and when its query floor ends.
        choices: Vec<(Family, Option<Instant>)>,
        picked: usize,
    },
    Trying {
        id: Id,
        family: Family,
    },
    Found {
        id: Id,
        family: Family,
        reading: Reading,
        /// When the reading came in; [Add] sits where [Try] was, so it waits `ADD_GUARD`.
        since: Instant,
    },
    /// [Add] was pressed; the UI thread writes the config and answers with `Event::Saved`.
    Saving(Id),
    Added(Id),
    SaveFailed(Id, String),
    Unplugged(Id),
    Unsupported(Id),
}

/// What the report says about the current device.
#[derive(Debug, Default)]
struct Facts {
    interfaces: Option<Interfaces>,
    tried: Option<(Family, String)>,
    reason: String,
}

pub struct Flow {
    step: Step,
    allow: Vec<Id>,
    custom: Vec<Device>,
    facts: Facts,
    /// The app and OS line of the report.
    environment: String,
    identity: Identity,
}

impl Flow {
    /// A flow at the warning; a preselected id follows as `Event::Handover`.
    pub fn new(
        allow: Vec<Id>,
        custom: Vec<Device>,
        environment: String,
        identity: Identity,
    ) -> Self {
        Self {
            step: Step::Warning,
            allow,
            custom,
            facts: Facts::default(),
            environment,
            identity,
        }
    }

    /// Applies `event` at `now`; the effect, if any, is for the window to perform.
    pub fn on(&mut self, event: Event, now: Instant) -> Option<Effect> {
        match event {
            // shortcut: an id handed over mid-try is dropped (offered again on its next plug-in);
            // queue it if owners hit this.
            Event::Handover(_) if matches!(self.step, Step::Trying { .. }) => None,
            Event::Handover(id) => self.begin(id),
            Event::Removed(id) => {
                self.removed(id);
                None
            }
            Event::Arrived(id) => {
                if let Step::Detecting { arrivals, .. } = &mut self.step
                    && !is_recovery(id.0, id.1)
                {
                    arrivals.arrived(id, now);
                }
                None
            }
            Event::BluetoothArrived => {
                if let Step::Detecting { bluetooth, .. } = &mut self.step {
                    *bluetooth = true;
                }
                None
            }
            Event::Tick => self.tick(now),
            Event::Inspected(inspection) => self.inspected(inspection, now),
            Event::Tried(outcome) => {
                self.tried(outcome, now);
                None
            }
            // Only the window that pressed [Add] waits for the save; the result is shown as is.
            Event::Saved(result) => {
                if let Step::Saving(id) = self.step {
                    self.step = match result {
                        Ok(()) => Step::Added(id),
                        Err(why) => Step::SaveFailed(id, why),
                    };
                }
                None
            }
            Event::Select(index) => {
                if let Step::Choose {
                    choices, picked, ..
                } = &mut self.step
                    && index < choices.len()
                {
                    *picked = index;
                }
                None
            }
            Event::Primary(input) => self.primary(input, now),
            Event::Alt => self.alt(now),
            Event::Extra => self.extra(),
        }
    }

    /// Whether a worker (or config save) reply is due, so the window keeps polling for it.
    pub fn awaits_worker(&self) -> bool {
        matches!(
            self.step,
            Step::Inspecting { again_at: None, .. } | Step::Trying { .. } | Step::Saving(_)
        )
    }

    /// When the window must send `Event::Tick` next; `None` while only the owner acts.
    pub fn next_tick(&self, now: Instant) -> Option<Duration> {
        let until = |at: Instant| at.saturating_duration_since(now);
        match &self.step {
            Step::Detecting {
                since, arrivals, ..
            } => Some(until(arrivals.quiet_at().unwrap_or(*since + DETECT_LIMIT))),
            Step::Inspecting {
                again_at: Some(at), ..
            } => Some(until(*at)),
            Step::Inspecting { .. } | Step::Trying { .. } | Step::Saving(_) => Some(REPLY_POLL),
            Step::Choose {
                choices, picked, ..
            } => choices[*picked]
                .1
                .filter(|ready| *ready > now)
                .map(|ready| until(ready).min(SECOND)),
            _ => None,
        }
    }

    pub fn view(&self, now: Instant) -> View {
        let mut view = View {
            text: String::new(),
            list: Vec::new(),
            picked: 0,
            form: None,
            details: None,
            primary: None,
            alt: None,
            extra: None,
            close: "Close",
        };
        match &self.step {
            Step::Warning => {
                view.text = WARNING.to_owned();
                view.primary = Some("Continue".to_owned());
                view.close = "Cancel";
            }
            Step::Detecting { bluetooth, .. } => {
                view.text = DETECTING.to_owned();
                if *bluetooth {
                    view.text.push_str("\n\n");
                    view.text.push_str(BLUETOOTH_HINT);
                }
                view.close = "Cancel";
            }
            Step::TimedOut => {
                view.text = "No new device was found in 2 minutes. Unplug the device, plug it \
                             back in, then press Try again."
                    .to_owned();
                view.primary = Some("Try again".to_owned());
            }
            Step::Pick(ids) => {
                view.text = "Several new devices were found. Pick the one to add:".to_owned();
                view.list = ids.iter().map(|&id| self.labelled(id)).collect();
                view.primary = Some("Next".to_owned());
                view.close = "Cancel";
            }
            Step::AlreadySupported(id) => {
                view.text = format!(
                    "{} is already supported: Juicetray reads its battery on its own.",
                    self.labelled(*id)
                );
            }
            Step::Inspecting { .. } => {
                view.text = "Reading the device's interfaces…".to_owned();
                view.close = "Cancel";
            }
            Step::Choose {
                id,
                choices,
                picked,
            } => self.choose_view(&mut view, *id, choices, *picked, now),
            Step::Trying { .. } => {
                view.text = "Waiting for the reader…".to_owned();
                view.close = "Cancel";
            }
            Step::Found {
                id,
                family,
                reading,
                ..
            } => {
                let mut state = String::new();
                if reading.charging {
                    state.push_str(" (charging)");
                }
                if reading.asleep {
                    state.push_str(" (asleep)");
                }
                view.text = format!(
                    "Read {} %{state}. Does this match the device?",
                    reading.percent
                );
                if known(id.0, id.1).is_none() {
                    view.text
                        .push_str("\n\nPick its kind and connection, name it, then press Add.");
                    view.form = Some(Form {
                        connections: connections(*family)
                            .iter()
                            .map(|(label, _)| *label)
                            .collect(),
                        name: self.name(*id),
                    });
                }
                view.primary = Some("Add".to_owned());
                view.alt = Some("Not right");
                view.close = "Cancel";
            }
            Step::Saving(_) => view.text = "Saving…".to_owned(),
            Step::Added(id) => {
                view.text = format!(
                    "{} was added. Juicetray reads its battery from now on.",
                    self.name(*id)
                );
            }
            // A failed config write is shown, never reported as added.
            Step::SaveFailed(id, why) => {
                view.text = format!("{} was not added: saving failed: {why}", self.name(*id));
            }
            Step::Unplugged(id) => {
                view.text = format!(
                    "{} was unplugged. Plug it back in, then press Try again.",
                    self.name(*id)
                );
                view.primary = Some("Try again".to_owned());
            }
            Step::Unsupported(id) => {
                view.text = format!(
                    "Not supported yet. {}\n\nSend the report (no account needed) or report it on \
                     GitHub so support can be added, or copy the details.",
                    self.facts.reason
                );
                // Two report channels: the account-free form first, then a GitHub issue.
                view.details = Some(self.details(*id));
                view.primary = Some("Copy".to_owned());
                view.alt = Some("Send report");
                view.extra = Some("Report on GitHub");
            }
        }
        view
    }

    fn choose_view(
        &self,
        view: &mut View,
        id: Id,
        choices: &[(Family, Option<Instant>)],
        picked: usize,
        now: Instant,
    ) {
        let (family, ready) = choices[picked];
        view.text = if choices.len() == 1 {
            format!(
                "{} may answer the {} battery query.",
                self.labelled(id),
                family.name()
            )
        } else {
            view.list = choices
                .iter()
                .map(|(family, _)| family.name().to_owned())
                .collect();
            view.picked = picked;
            format!(
                "{} may answer one of these battery queries. Pick one:",
                self.labelled(id)
            )
        };
        view.text.push_str(
            "\n\nClose the device's own app, then press Try: Juicetray sends that one \
             read-only query, once.",
        );
        // Off the catalogue, nothing tells a BITMOUSE receiver from a cable mouse: the button's
        // own label is the owner's confirmation that it is a receiver.
        let label = if family == Family::AtkBitmouse && known(id.0, id.1).is_none() {
            view.text
                .push_str(" Never try a mouse on its cable: BITMOUSE goes to USB receivers only.");
            RECEIVER_TRY
        } else {
            "Try"
        };
        let wait = ready.map_or(Duration::ZERO, |at| at.saturating_duration_since(now));
        view.primary = Some(if wait.is_zero() {
            label.to_owned()
        } else {
            // Whole seconds, rounded up, so it never shows 0 while waiting.
            format!("{label} in {} s", wait.as_millis().div_ceil(1000))
        });
        view.close = "Cancel";
    }

    /// Starts on `id`: already supported, never touched, or inspected by the worker.
    fn begin(&mut self, id: Id) -> Option<Effect> {
        self.facts = Facts::default();
        if self.is_supported(id) {
            self.step = Step::AlreadySupported(id);
            return None;
        }
        if is_recovery(id.0, id.1) {
            self.unsupported(id, RECOVERY.to_owned());
            return None;
        }
        self.step = Step::Inspecting {
            id,
            tries: 0,
            again_at: None,
        };
        Some(Effect::Inspect(id))
    }

    fn detect(&mut self, now: Instant) {
        self.step = Step::Detecting {
            since: now,
            arrivals: Arrivals::default(),
            bluetooth: false,
        };
    }

    fn tick(&mut self, now: Instant) -> Option<Effect> {
        match &mut self.step {
            Step::Detecting {
                since, arrivals, ..
            } => {
                if arrivals.quiet_at().is_some_and(|at| now >= at) {
                    let due = arrivals.take_due(now);
                    return self.detected(due);
                }
                if arrivals.quiet_at().is_none() && now >= *since + DETECT_LIMIT {
                    self.step = Step::TimedOut;
                }
                None
            }
            Step::Inspecting { id, again_at, .. } if again_at.is_some_and(|at| now >= at) => {
                *again_at = None;
                Some(Effect::Inspect(*id))
            }
            _ => None,
        }
    }

    /// Settled new ids: one is inspected, several are picked from; ids the worker already
    /// queries only say so when nothing else came.
    fn detected(&mut self, ids: Vec<Id>) -> Option<Effect> {
        let (supported, new): (Vec<Id>, Vec<Id>) =
            ids.into_iter().partition(|&id| self.is_supported(id));
        match new.as_slice() {
            [] => {
                if let Some(&id) = supported.first() {
                    self.step = Step::AlreadySupported(id);
                }
                None
            }
            [id] => self.begin(*id),
            _ => {
                self.step = Step::Pick(new);
                None
            }
        }
    }

    /// A device left: forgotten while detecting, "unplugged" while it is being looked at.
    fn removed(&mut self, gone: Id) {
        match &mut self.step {
            Step::Detecting { arrivals, .. } => arrivals.forget(gone),
            Step::Inspecting { id, .. } | Step::Choose { id, .. } | Step::Trying { id, .. }
                if *id == gone =>
            {
                self.step = Step::Unplugged(gone);
            }
            _ => {}
        }
    }

    fn inspected(&mut self, inspection: Inspection, now: Instant) -> Option<Effect> {
        let Step::Inspecting { id, tries, .. } = self.step else {
            return None;
        };
        match inspection {
            Inspection::Gone => self.step = Step::Unplugged(id),
            Inspection::Settling if tries + 1 < INSPECT_TRIES => {
                self.step = Step::Inspecting {
                    id,
                    tries: tries + 1,
                    again_at: Some(now + INSPECT_RETRY),
                };
            }
            Inspection::Settling => self.unsupported(id, UNSTABLE.to_owned()),
            Inspection::Failed(why) => {
                self.unsupported(id, format!("Its interfaces could not be read: {why}"));
            }
            Inspection::Ready(interfaces) => {
                let offered = choices(id, &interfaces.families);
                self.facts.interfaces = Some(interfaces);
                if offered.is_empty() {
                    self.unsupported(id, no_choice_reason(id).to_owned());
                } else {
                    self.step = Step::Choose {
                        id,
                        choices: ready_times(&offered, now),
                        picked: 0,
                    };
                }
            }
        }
        None
    }

    fn tried(&mut self, outcome: TryOutcome, now: Instant) {
        let Step::Trying { id, family } = self.step else {
            return;
        };
        let name = family.name();
        // Unplugged and floor answers leave no report line; every other answer is recorded.
        let (result, reason) = match outcome {
            TryOutcome::Gone => {
                self.step = Step::Unplugged(id);
                return;
            }
            TryOutcome::Waiting(wait) => {
                let families = self
                    .facts
                    .interfaces
                    .as_ref()
                    .map(|interfaces| choices(id, &interfaces.families))
                    .unwrap_or_default();
                let mut timed = ready_times(&families, now);
                let picked = timed
                    .iter()
                    .position(|(choice, _)| *choice == family)
                    .unwrap_or_else(|| {
                        timed.push((family, None));
                        timed.len() - 1
                    });
                timed[picked].1 = Some(now + wait);
                self.step = Step::Choose {
                    id,
                    choices: timed,
                    picked,
                };
                return;
            }
            TryOutcome::Reading(reading) => {
                self.facts.tried = Some((family, format!("read {} %", reading.percent)));
                self.step = Step::Found {
                    id,
                    family,
                    reading,
                    since: now,
                };
                return;
            }
            TryOutcome::NoReply => (
                "no reply".to_owned(),
                format!("It did not answer the {name} battery query."),
            ),
            TryOutcome::Invalid(why) => (
                format!("invalid reply ({why})"),
                format!("Its answer to the {name} battery query is not a battery level ({why})."),
            ),
            TryOutcome::Failed(why) => (
                format!("not sent ({why})"),
                format!("The {name} battery query could not be sent: {why}"),
            ),
            // The write was attempted, so "not sent" would be untrue.
            TryOutcome::Interrupted(why) => (
                format!("sent, then failed ({why})"),
                format!(
                    "The {name} battery query may have been sent, but no answer was read: {why}"
                ),
            ),
        };
        self.facts.tried = Some((family, result));
        self.unsupported(id, reason);
    }

    fn primary(&mut self, input: Input, now: Instant) -> Option<Effect> {
        match &self.step {
            Step::Warning | Step::TimedOut => {
                self.detect(now);
                None
            }
            Step::Pick(ids) => {
                let id = ids.get(input.list).or(ids.first()).copied()?;
                self.begin(id)
            }
            Step::Choose {
                id,
                choices,
                picked,
            } => {
                let (family, ready) = choices[*picked];
                if ready.is_some_and(|at| now < at) {
                    return None;
                }
                let id = *id;
                self.step = Step::Trying { id, family };
                Some(Effect::TryAs(id, family))
            }
            // A press right after the reading is the [Try] press repeating, not a confirmation.
            Step::Found { since, .. } if now < *since + ADD_GUARD => None,
            Step::Found { id, family, .. } => {
                let id = *id;
                let request = add_request(id, *family, &input, &self.name(id));
                self.step = Step::Saving(id);
                Some(Effect::Add(request))
            }
            Step::Unplugged(id) => self.begin(*id),
            Step::Unsupported(_) => Some(Effect::Copy),
            _ => None,
        }
    }

    fn alt(&mut self, now: Instant) -> Option<Effect> {
        match self.step {
            // A wide [Try] overlaps [Not right], so a double-click on it must not dismiss the reading.
            Step::Found { since, .. } if now < since + ADD_GUARD => None,
            Step::Found { id, reading, .. } => {
                self.unsupported(
                    id,
                    format!(
                        "The level it gave ({} %) does not match the device.",
                        reading.percent
                    ),
                );
                None
            }
            // The redacted details fill the request form; its first line names the device.
            Step::Unsupported(id) => Some(Effect::OpenUrl(report::report_url(&self.details(id)))),
            _ => None,
        }
    }

    /// [Report on GitHub]: a new-issue page with the redacted details; other steps show no
    /// third button, so the event does nothing there.
    fn extra(&self) -> Option<Effect> {
        let Step::Unsupported(id) = self.step else {
            return None;
        };
        // The title carries the USB product string, so it is redacted like the body.
        let title = report::redact(
            &format!("Device report: {}", self.labelled(id)),
            &self.identity,
        );
        Some(Effect::OpenUrl(report::issue_url(
            &title,
            &self.details(id),
        )))
    }

    fn unsupported(&mut self, id: Id, reason: String) {
        self.facts.reason = reason;
        self.step = Step::Unsupported(id);
    }

    /// The worker already queries it: verified, in `allow` or custom.
    fn is_supported(&self, (vid, pid): Id) -> bool {
        lookup(vid, pid, &self.custom).is_some_and(|device| device.is_queried(&self.allow))
    }

    /// The catalogue name, else the USB product string, else `VID:PID`.
    fn name(&self, (vid, pid): Id) -> String {
        known(vid, pid)
            .map(|device| device.name.to_owned())
            .or_else(|| {
                self.facts
                    .interfaces
                    .as_ref()
                    .and_then(|interfaces| interfaces.product.as_deref())
                    .map(str::trim)
                    .filter(|product| !product.is_empty())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| id_text((vid, pid)))
    }

    /// The name with its id, unless the name is the id.
    fn labelled(&self, id: Id) -> String {
        let name = self.name(id);
        let id = id_text(id);
        if name == id {
            id
        } else {
            format!("{name} ({id})")
        }
    }

    /// The report text, redacted: what the device is, what matched, what the try gave.
    fn details(&self, id: Id) -> String {
        let (vid, pid) = id;
        // The environment leads, so a cut issue URL loses interface lines, not the app version.
        let mut lines = vec![
            self.environment.clone(),
            format!("Device: {}", self.labelled(id)),
        ];
        lines.push(match known(vid, pid) {
            Some(device) if device.verified => format!("Catalogue: {}, verified", device.name),
            Some(device) => format!("Catalogue: {}, not verified", device.name),
            None => "Catalogue: not listed".to_owned(),
        });
        let interfaces = self.facts.interfaces.as_ref();
        let families: Vec<&str> = interfaces
            .map(|found| {
                found
                    .families
                    .iter()
                    .map(|(family, _)| family.name())
                    .collect()
            })
            .unwrap_or_default();
        lines.push(format!(
            "Matching protocols: {}",
            if families.is_empty() {
                "none".to_owned()
            } else {
                families.join(", ")
            }
        ));
        lines.push(match &self.facts.tried {
            Some((family, result)) => format!("Tried: {}: {result}", family.name()),
            None => "Tried: nothing".to_owned(),
        });
        lines.push(format!("Result: {}", self.facts.reason));
        // The scan listing carries every fact a catalogue row needs; bare shapes are the fallback.
        if let Some(found) = interfaces {
            lines.push("Interfaces:".to_owned());
            if found.scan_lines.is_empty() {
                lines.extend(found.shapes.iter().map(shape_line));
                if found.serial {
                    lines.push("  serial port".to_owned());
                }
            } else {
                lines.extend(found.scan_lines.iter().cloned());
            }
        }
        report::redact(&lines.join("\n"), &self.identity)
    }
}

/// Why nothing is offered: a catalogue model its row blocks, else no matching interface.
fn no_choice_reason((vid, pid): Id) -> &'static str {
    match known(vid, pid) {
        Some(device) if device.needs_capture => NEEDS_CAPTURE,
        Some(device) if device.family == Family::AtkBitmouse && device.cable => BITMOUSE_CABLE,
        _ => NO_FAMILY,
    }
}

/// The families offered for `id`: a catalogue model only as its own family (the worker queries
/// it that way once allowed) and never when it needs a capture or is a BITMOUSE cable; ATK serial,
/// RongYuan F7, Driveall and Generic only exist for catalogue models, so other ids never get them.
pub fn choices(id: Id, families: &[(Family, Option<Duration>)]) -> Vec<(Family, Option<Duration>)> {
    let keep = |family: Family| match known(id.0, id.1) {
        Some(device) => {
            family == device.family
                && !device.needs_capture
                && !(device.family == Family::AtkBitmouse && device.cable)
        }
        None => !matches!(
            family,
            Family::AtkSerial | Family::RongYuanF7 | Family::Driveall | Family::Generic(_)
        ),
    };
    families
        .iter()
        .copied()
        .filter(|(family, _)| keep(*family))
        .collect()
}

/// The connection choices for `family`; BITMOUSE is read through a receiver only.
pub fn connections(family: Family) -> &'static [(&'static str, &'static str)] {
    if family == Family::AtkBitmouse {
        &CONNECTIONS[..1]
    } else {
        &CONNECTIONS
    }
}

/// What [Add] saves: a catalogue id joins `allow`; any other id becomes a custom device with the
/// picked kind and connection and the typed name (`default_name` when blank).
pub fn add_request(id: Id, family: Family, input: &Input, default_name: &str) -> AddRequest {
    if known(id.0, id.1).is_some() {
        return AddRequest::Allow(id);
    }
    let offered = connections(family);
    let (_, connection) = offered.get(input.connection).unwrap_or(&offered[0]);
    let (_, kind) = KINDS.get(input.kind).unwrap_or(&KINDS[0]);
    let typed = input.name.trim();
    AddRequest::Custom(CustomEntry {
        id: id_text(id),
        name: if typed.is_empty() {
            default_name
        } else {
            typed
        }
        .to_owned(),
        family: format!("{family:?}"),
        kind: (*kind).to_owned(),
        connection: (*connection).to_owned(),
    })
}

fn ready_times(
    families: &[(Family, Option<Duration>)],
    now: Instant,
) -> Vec<(Family, Option<Instant>)> {
    families
        .iter()
        .map(|&(family, wait)| (family, wait.map(|wait| now + wait)))
        .collect()
}

fn id_text((vid, pid): Id) -> String {
    format!("{vid:04X}:{pid:04X}")
}

/// One interface line of the report: interface, usage page:usage and report sizes.
fn shape_line(shape: &HidShape) -> String {
    let interface = shape.interface.map_or("?".to_owned(), |n| n.to_string());
    let reports = if shape.reports.is_empty() {
        "not read".to_owned()
    } else {
        reports_text(&shape.reports, None)
    };
    format!(
        "  if {interface} {:04X}:{:04X} {reports}",
        shape.usage_page, shape.usage
    )
}

#[cfg(test)]
mod tests {
    use protocol::{ReportKind, ReportSize};

    use super::*;
    use crate::device::tests::custom;

    /// MCHOSE V9 Pro 2, a verified catalogue row.
    const VERIFIED: Id = (0x3837, 0x6011);
    const UNKNOWN: Id = (0x3554, 0x7777);
    const OTHER: Id = (0x373B, 0x8888);

    fn new_flow() -> Flow {
        Flow::new(
            Vec::new(),
            Vec::new(),
            "Juicetray v0.0.0, Windows 10.0.19045".to_owned(),
            Identity {
                profiles: vec![r"C:\Users\Jane".to_owned()],
                computer: Some("JANE-PC".to_owned()),
            },
        )
    }

    /// An unverified catalogue row that needs no capture and is not a BITMOUSE cable.
    fn unverified() -> (Id, Family) {
        protocol::KNOWN
            .iter()
            .find(|device| {
                !device.verified
                    && !device.needs_capture
                    && !is_recovery(device.vid, device.pid)
                    && !(device.family == Family::AtkBitmouse && device.cable)
            })
            .map(|device| ((device.vid, device.pid), device.family))
            .expect("an unverified catalogue row")
    }

    fn compx_shape() -> HidShape {
        HidShape {
            interface: Some(1),
            usage_page: 0xFF02,
            usage: 0x0002,
            reports: [ReportKind::Input, ReportKind::Output]
                .into_iter()
                .map(|kind| ReportSize {
                    kind,
                    id: 0x08,
                    bytes: 17,
                })
                .collect(),
        }
    }

    fn ready(families: &[(Family, Option<Duration>)]) -> Inspection {
        Inspection::Ready(Interfaces {
            families: families.to_vec(),
            product: Some("Gaming Mouse".to_owned()),
            shapes: vec![compx_shape()],
            serial: false,
            scan_lines: Vec::new(),
        })
    }

    fn reading(percent: u8) -> Reading {
        Reading {
            percent,
            charging: false,
            asleep: false,
        }
    }

    /// A flow at Choose for `id` with `families`, started by a handover.
    fn choosing(id: Id, families: &[(Family, Option<Duration>)], now: Instant) -> Flow {
        let mut flow = new_flow();
        assert_eq!(flow.on(Event::Handover(id), now), Some(Effect::Inspect(id)));
        assert_eq!(flow.on(Event::Inspected(ready(families)), now), None);
        flow
    }

    fn press(flow: &mut Flow, now: Instant) -> Option<Effect> {
        flow.on(Event::Primary(Input::default()), now)
    }

    #[test]
    fn menu_start_warns_then_detects_one_new_device() {
        let start = Instant::now();
        let mut flow = new_flow();
        let view = flow.view(start);
        assert!(
            view.text
                .starts_with("This device is not officially supported yet.")
        );
        assert!(
            view.text
                .ends_with("https://github.com/ittmchien/juicetray/issues")
        );
        assert_eq!(view.primary.as_deref(), Some("Continue"));
        assert_eq!(view.close, "Cancel");
        // No third button here, so its event does nothing.
        assert_eq!(view.extra, None);
        assert_eq!(flow.on(Event::Extra, start), None);

        assert_eq!(press(&mut flow, start), None);
        assert!(flow.view(start).text.starts_with("Detecting…"));
        assert_eq!(flow.next_tick(start), Some(DETECT_LIMIT));

        // Replug: a removal then arrivals of the same id; it settles 2 s after the last one.
        flow.on(Event::Removed(UNKNOWN), start);
        flow.on(Event::Arrived(UNKNOWN), start);
        let later = start + Duration::from_secs(1);
        flow.on(Event::Arrived(UNKNOWN), later);
        assert_eq!(flow.next_tick(start), Some(Duration::from_secs(3)));
        assert_eq!(flow.on(Event::Tick, start + Duration::from_secs(2)), None);
        assert_eq!(
            flow.on(Event::Tick, later + crate::plug::SETTLE),
            Some(Effect::Inspect(UNKNOWN))
        );
        assert!(flow.awaits_worker());
        assert_eq!(flow.next_tick(start), Some(REPLY_POLL));
    }

    #[test]
    fn detection_picks_among_several_and_skips_supported_and_recovery_ids() {
        let start = Instant::now();
        let mut flow = new_flow();
        press(&mut flow, start);
        flow.on(Event::Arrived(VERIFIED), start);
        flow.on(Event::Arrived(UNKNOWN), start);
        flow.on(Event::Arrived(OTHER), start + Duration::from_millis(400));
        flow.on(Event::Arrived((0x3554, 0xF2FF)), start);
        // Nothing acts until every waiting id is quiet.
        assert_eq!(flow.on(Event::Tick, start + crate::plug::SETTLE), None);
        assert!(flow.view(start).text.starts_with("Detecting…"));
        let quiet = start + Duration::from_millis(400) + crate::plug::SETTLE;
        assert_eq!(flow.on(Event::Tick, quiet), None);
        let view = flow.view(quiet);
        assert_eq!(view.list, ["3554:7777", "373B:8888"]);
        assert_eq!(view.primary.as_deref(), Some("Next"));
        let second = Input {
            list: 1,
            ..Input::default()
        };
        assert_eq!(
            flow.on(Event::Primary(second), quiet),
            Some(Effect::Inspect(OTHER))
        );
    }

    #[test]
    fn detection_reports_an_already_supported_device_and_times_out() {
        let start = Instant::now();
        let mut flow = new_flow();
        press(&mut flow, start);
        flow.on(Event::Arrived(VERIFIED), start);
        assert_eq!(flow.on(Event::Tick, start + crate::plug::SETTLE), None);
        assert!(
            flow.view(start)
                .text
                .contains("MCHOSE V9 Pro 2 (3837:6011) is already supported")
        );

        let mut flow = new_flow();
        press(&mut flow, start);
        flow.on(Event::BluetoothArrived, start);
        assert!(flow.view(start).text.contains("Bluetooth"));
        // A device that leaves before it settles is forgotten.
        flow.on(Event::Arrived(UNKNOWN), start);
        flow.on(Event::Removed(UNKNOWN), start);
        assert_eq!(flow.next_tick(start), Some(DETECT_LIMIT));
        flow.on(Event::Tick, start + DETECT_LIMIT - Duration::from_millis(1));
        assert!(flow.view(start).text.starts_with("Detecting…"));
        flow.on(Event::Tick, start + DETECT_LIMIT);
        let view = flow.view(start);
        assert!(view.text.starts_with("No new device was found"));
        assert_eq!(flow.next_tick(start), None);
        // Try again restarts the 2 minutes.
        let again = start + DETECT_LIMIT;
        press(&mut flow, again);
        assert_eq!(flow.next_tick(again), Some(DETECT_LIMIT));
    }

    #[test]
    fn preselected_ids_skip_warning_and_detection() {
        let now = Instant::now();
        let mut flow = new_flow();
        assert_eq!(flow.on(Event::Handover(VERIFIED), now), None);
        assert!(flow.view(now).text.contains("already supported"));
        // A custom device or an allowed catalogue row is supported too.
        let (row, _) = unverified();
        let mut flow = Flow::new(
            vec![row],
            vec![custom(0x1234, 0x5678, Family::Compx)],
            String::new(),
            Identity::default(),
        );
        for id in [row, (0x1234, 0x5678)] {
            assert_eq!(flow.on(Event::Handover(id), now), None);
            assert!(
                flow.view(now).text.contains("already supported"),
                "{id:04X?}"
            );
        }
        // A recovery id is never inspected.
        let mut flow = new_flow();
        assert_eq!(flow.on(Event::Handover((0x3554, 0xF2FF)), now), None);
        assert!(flow.view(now).text.contains("recovery"));
    }

    #[test]
    fn settling_interfaces_are_asked_again_then_given_up() {
        let start = Instant::now();
        let mut flow = new_flow();
        flow.on(Event::Handover(UNKNOWN), start);
        let mut now = start;
        for _ in 1..INSPECT_TRIES {
            assert_eq!(flow.on(Event::Inspected(Inspection::Settling), now), None);
            assert!(!flow.awaits_worker());
            assert_eq!(flow.next_tick(now), Some(INSPECT_RETRY));
            assert_eq!(flow.on(Event::Tick, now), None);
            now += INSPECT_RETRY;
            assert_eq!(flow.on(Event::Tick, now), Some(Effect::Inspect(UNKNOWN)));
        }
        flow.on(Event::Inspected(Inspection::Settling), now);
        let view = flow.view(now);
        assert!(view.text.contains(UNSTABLE), "{}", view.text);
        assert_eq!(view.primary.as_deref(), Some("Copy"));
        assert_eq!(view.alt, Some("Send report"));
        assert_eq!(view.extra, Some("Report on GitHub"));
    }

    #[test]
    fn inspection_outcomes() {
        let now = Instant::now();
        let mut flow = new_flow();
        flow.on(Event::Handover(UNKNOWN), now);
        flow.on(Event::Inspected(Inspection::Gone), now);
        let view = flow.view(now);
        assert!(view.text.starts_with("3554:7777 was unplugged"));
        // Try again re-checks presence.
        assert_eq!(press(&mut flow, now), Some(Effect::Inspect(UNKNOWN)));
        flow.on(
            Event::Inspected(Inspection::Failed("hid unavailable".into())),
            now,
        );
        assert!(flow.view(now).text.contains("hid unavailable"));

        let flow = choosing(UNKNOWN, &[], now);
        let view = flow.view(now);
        assert!(view.text.contains(NO_FAMILY));
        let details = view.details.unwrap();
        assert!(
            details.contains("Device: Gaming Mouse (3554:7777)"),
            "{details}"
        );
        assert!(details.contains("Matching protocols: none"), "{details}");
        assert!(
            details.contains("if 1 FF02:0002 in 08:17 out 08:17"),
            "{details}"
        );
    }

    #[test]
    fn choices_follow_the_catalogue() {
        let both = [(Family::Compx, None), (Family::Report13, None)];
        assert_eq!(choices(UNKNOWN, &both), both);
        assert!(choices(UNKNOWN, &[(Family::AtkSerial, None)]).is_empty());
        assert!(choices(UNKNOWN, &[(Family::RongYuanF7, None)]).is_empty());
        assert!(choices(UNKNOWN, &[(Family::Driveall, None)]).is_empty());
        static SPEC: protocol::GenericSpec = protocol::GenericSpec {
            usage_page: 0xFF60,
            usage: 0x0061,
            report: protocol::GenericReport::Output,
            report_bytes: 64,
            request: &[0x00, 0x01],
            reply_prefix: &[0x00, 0x55, 0x10],
            level_byte: 26,
            charging: None,
            scale: 100,
        };
        assert!(choices(UNKNOWN, &[(Family::Generic(&SPEC), None)]).is_empty());
        // A catalogue model is tried as its own family only.
        let (row, family) = unverified();
        let other = if family == Family::Compx {
            Family::Report13
        } else {
            Family::Compx
        };
        assert_eq!(
            choices(row, &[(other, None), (family, None)]),
            [(family, None)]
        );
        assert!(choices(row, &[(other, None)]).is_empty());
        for device in protocol::KNOWN {
            let blocked =
                device.needs_capture || (device.family == Family::AtkBitmouse && device.cable);
            if blocked {
                let id = (device.vid, device.pid);
                assert!(
                    choices(id, &[(device.family, None)]).is_empty(),
                    "{id:04X?}"
                );
            }
        }
    }

    #[test]
    fn try_waits_out_the_floor_then_sends_once() {
        let start = Instant::now();
        let floor = Duration::from_millis(9500);
        let mut flow = choosing(
            UNKNOWN,
            &[(Family::Compx, Some(floor)), (Family::Report13, None)],
            start,
        );
        let view = flow.view(start);
        assert_eq!(view.list, ["Compx report 8", "VGN report 0x13"]);
        assert_eq!(view.primary.as_deref(), Some("Try in 10 s"));
        assert_eq!(flow.next_tick(start), Some(SECOND));
        // A press inside the floor sends nothing.
        assert_eq!(press(&mut flow, start), None);
        // The other family has no floor.
        flow.on(Event::Select(1), start);
        assert_eq!(flow.view(start).primary.as_deref(), Some("Try"));
        assert_eq!(flow.next_tick(start), None);
        flow.on(Event::Select(0), start);
        flow.on(Event::Select(7), start);
        let late = start + floor;
        assert_eq!(flow.view(late).primary.as_deref(), Some("Try"));
        assert_eq!(
            press(&mut flow, late),
            Some(Effect::TryAs(UNKNOWN, Family::Compx))
        );
        assert!(flow.awaits_worker());
        assert_eq!(flow.view(late).text, "Waiting for the reader…");
        // A second press while trying sends nothing more.
        assert_eq!(press(&mut flow, late), None);

        // A floor the worker found meanwhile goes back to the countdown.
        flow.on(
            Event::Tried(TryOutcome::Waiting(Duration::from_secs(3))),
            late,
        );
        let view = flow.view(late);
        assert_eq!(view.picked, 0);
        assert_eq!(view.primary.as_deref(), Some("Try in 3 s"));
    }

    #[test]
    fn single_family_has_no_picker() {
        let now = Instant::now();
        let flow = choosing(UNKNOWN, &[(Family::Compx, None)], now);
        let view = flow.view(now);
        assert!(view.list.is_empty());
        assert!(
            view.text.starts_with(
                "Gaming Mouse (3554:7777) may answer the Compx report 8 battery query."
            )
        );
    }

    #[test]
    fn try_outcomes_lead_to_add_or_the_report() {
        let now = Instant::now();
        let tried = |outcome: TryOutcome| {
            let mut flow = choosing(UNKNOWN, &[(Family::Compx, None)], now);
            press(&mut flow, now);
            flow.on(Event::Tried(outcome), now);
            flow
        };
        let flow = tried(TryOutcome::Reading(reading(52)));
        let view = flow.view(now);
        assert!(
            view.text
                .starts_with("Read 52 %. Does this match the device?")
        );
        assert_eq!(view.primary.as_deref(), Some("Add"));
        assert_eq!(view.alt, Some("Not right"));
        let form = view.form.unwrap();
        assert_eq!(form.name, "Gaming Mouse");
        assert_eq!(form.connections.len(), 2);

        // A refusal before the write says "not sent"; a failure after it never does.
        for (outcome, text, tried_line) in [
            (TryOutcome::NoReply, "did not answer", "no reply"),
            (
                TryOutcome::Invalid("level 0xFF".into()),
                "level 0xFF",
                "invalid reply",
            ),
            (
                TryOutcome::Failed("busy".into()),
                "could not be sent: busy",
                "not sent (busy)",
            ),
            (
                TryOutcome::Interrupted("read failed".into()),
                "may have been sent, but no answer was read: read failed",
                "sent, then failed (read failed)",
            ),
        ] {
            let flow = tried(outcome);
            let view = flow.view(now);
            assert!(view.text.starts_with("Not supported yet."), "{}", view.text);
            assert!(view.text.contains(text), "{}", view.text);
            let details = view.details.unwrap();
            assert!(
                details.contains(&format!("Tried: Compx report 8: {tried_line}")),
                "{details}"
            );
        }
        let flow = tried(TryOutcome::Gone);
        assert!(flow.view(now).text.contains("was unplugged"));

        // Unplugged mid-try: the late reply is ignored.
        let mut flow = choosing(UNKNOWN, &[(Family::Compx, None)], now);
        press(&mut flow, now);
        flow.on(Event::Removed(OTHER), now);
        assert!(flow.awaits_worker());
        flow.on(Event::Removed(UNKNOWN), now);
        assert!(!flow.awaits_worker());
        flow.on(Event::Tried(TryOutcome::Reading(reading(52))), now);
        assert!(flow.view(now).text.contains("was unplugged"));
    }

    #[test]
    fn add_saves_allow_for_catalogue_rows_and_custom_entries_otherwise() {
        let now = Instant::now();
        let mut flow = choosing(UNKNOWN, &[(Family::Compx, None)], now);
        press(&mut flow, now);
        flow.on(Event::Tried(TryOutcome::Reading(reading(52))), now);
        let input = Input {
            list: 0,
            kind: 2,
            connection: 1,
            name: "  My board ".to_owned(),
        };
        let checked = now + ADD_GUARD;
        assert_eq!(
            flow.on(Event::Primary(input), checked),
            Some(Effect::Add(AddRequest::Custom(CustomEntry {
                id: "3554:7777".into(),
                name: "My board".into(),
                family: "Compx".into(),
                kind: "keyboard".into(),
                connection: "cable".into(),
            })))
        );
        // "Added" only once the UI thread says the config was written.
        assert_eq!(flow.view(checked).text, "Saving…");
        assert!(flow.awaits_worker());
        assert_eq!(flow.next_tick(checked), Some(REPLY_POLL));
        flow.on(Event::Saved(Ok(())), checked);
        assert!(!flow.awaits_worker());
        assert!(
            flow.view(checked)
                .text
                .starts_with("Gaming Mouse was added.")
        );

        let (row, family) = unverified();
        let mut flow = choosing(row, &[(family, None)], now);
        press(&mut flow, now);
        flow.on(Event::Tried(TryOutcome::Reading(reading(80))), now);
        assert_eq!(flow.view(now).form, None);
        assert_eq!(
            press(&mut flow, checked),
            Some(Effect::Add(AddRequest::Allow(row)))
        );
        // A failed save is shown, never reported as added.
        flow.on(
            Event::Saved(Err("config.toml is not valid TOML".into())),
            checked,
        );
        let text = flow.view(checked).text;
        assert!(text.contains("was not added"), "{text}");
        assert!(text.contains("config.toml is not valid TOML"), "{text}");

        // BITMOUSE: receiver only, and a blank name takes the default.
        let blank = Input {
            connection: 1,
            name: " ".into(),
            ..Input::default()
        };
        assert_eq!(connections(Family::AtkBitmouse).len(), 1);
        assert_eq!(
            add_request(OTHER, Family::AtkBitmouse, &blank, "373B:8888"),
            AddRequest::Custom(CustomEntry {
                id: "373B:8888".into(),
                name: "373B:8888".into(),
                family: "AtkBitmouse".into(),
                kind: "mouse".into(),
                connection: "receiver".into(),
            })
        );
        // Every custom family it writes is one the config accepts.
        for family in [
            Family::Compx,
            Family::Report13,
            Family::Mchose,
            Family::AtkBitmouse,
            Family::VgnKc2,
        ] {
            let AddRequest::Custom(entry) = add_request(OTHER, family, &blank, "x") else {
                panic!("custom");
            };
            let config = crate::config::Config {
                custom_devices: vec![entry],
                ..crate::config::Config::default()
            };
            assert_eq!(config.custom().len(), 1, "{family:?}");
        }
    }

    #[test]
    fn not_right_reports_and_the_report_is_redacted() {
        let now = Instant::now();
        let mut flow = choosing(UNKNOWN, &[(Family::Compx, None)], now);
        press(&mut flow, now);
        flow.on(Event::Tried(TryOutcome::Reading(reading(52))), now);
        // Only an unsupported device has a third button.
        assert_eq!(flow.view(now).extra, None);
        assert_eq!(flow.on(Event::Extra, now + ADD_GUARD), None);
        assert_eq!(flow.view(now).primary.as_deref(), Some("Add"));
        // [Not right] waits out the same guard as [Add].
        assert_eq!(flow.on(Event::Alt, now), None);
        assert_eq!(flow.view(now).primary.as_deref(), Some("Add"));
        let now = now + ADD_GUARD;
        assert_eq!(flow.on(Event::Alt, now), None);
        let view = flow.view(now);
        assert!(view.text.contains("(52 %) does not match"), "{}", view.text);
        assert_eq!(view.primary.as_deref(), Some("Copy"));
        assert_eq!(view.alt, Some("Send report"));
        assert_eq!(view.extra, Some("Report on GitHub"));
        assert_eq!(press(&mut flow, now), Some(Effect::Copy));
        let Some(Effect::OpenUrl(url)) = flow.on(Event::Extra, now) else {
            panic!("issue url");
        };
        assert!(
            url.starts_with(
                "https://github.com/ittmchien/juicetray/issues/new?title=Device%20report%3A%20"
            ),
            "{url}"
        );
        assert!(url.len() <= report::MAX_URL_LEN);
        let Some(Effect::OpenUrl(url)) = flow.on(Event::Alt, now) else {
            panic!("report url");
        };
        assert!(url.starts_with("https://docs.google.com/forms/"), "{url}");
        assert!(url.contains("Device%3A%20"), "{url}");
        assert!(url.len() <= report::MAX_URL_LEN);

        let mut flow = new_flow();
        flow.on(Event::Handover(UNKNOWN), now);
        flow.on(
            Event::Inspected(Inspection::Failed(r"C:\Users\Jane\x on JANE-PC".into())),
            now,
        );
        let details = flow.view(now).details.unwrap();
        assert!(details.contains(r"%USERPROFILE%\x on <pc>"), "{details}");
        // The environment leads, so URL truncation never drops it.
        assert!(details.starts_with("Juicetray v0.0.0, Windows 10.0.19045\nDevice: "));

        // The product string in the sent report is redacted too.
        let mut flow = new_flow();
        flow.on(Event::Handover(UNKNOWN), now);
        flow.on(
            Event::Inspected(Inspection::Ready(Interfaces {
                product: Some("Mouse of JANE-PC".into()),
                ..Interfaces::default()
            })),
            now,
        );
        let Some(Effect::OpenUrl(url)) = flow.on(Event::Alt, now) else {
            panic!("report url");
        };
        assert!(url.starts_with("https://docs.google.com/forms/"), "{url}");
        assert!(!url.contains("JANE"), "{url}");
        assert!(url.contains("%3Cpc%3E"), "{url}");
        // The GitHub issue's title carries the product string, redacted as well.
        let Some(Effect::OpenUrl(url)) = flow.on(Event::Extra, now) else {
            panic!("issue url");
        };
        let title = url.split("&body=").next().unwrap();
        assert!(!url.contains("JANE"), "{url}");
        assert!(title.contains("%3Cpc%3E"), "{title}");
    }

    #[test]
    fn details_list_the_scan_lines_redacted() {
        let now = Instant::now();
        let mut flow = new_flow();
        flow.on(Event::Handover(UNKNOWN), now);
        let scan_lines = vec![
            "3554:7777  unknown device".to_owned(),
            "  manufacturer \"Jane's\" · product \"Pad of JANE-PC\"".to_owned(),
            "  HID if 1 FF02:0002 vendor · Usb · in 08:17 out 08:17".to_owned(),
            "    descriptor (3 B): 06 02 ff".to_owned(),
        ];
        flow.on(
            Event::Inspected(Inspection::Ready(Interfaces {
                shapes: vec![compx_shape()],
                serial: true,
                scan_lines,
                ..Interfaces::default()
            })),
            now,
        );
        let details = flow.view(now).details.unwrap();
        assert!(
            details.contains("Interfaces:\n3554:7777  unknown device"),
            "{details}"
        );
        assert!(details.contains("descriptor (3 B): 06 02 ff"), "{details}");
        assert!(details.contains("product \"Pad of <pc>\""), "{details}");
        // The scan lines replace the bare shape and serial lines.
        assert!(!details.contains("  if 1 FF02:0002"), "{details}");
        assert!(!details.contains("  serial port"), "{details}");
    }

    #[test]
    fn add_ignores_presses_right_after_the_reading() {
        let now = Instant::now();
        let mut flow = choosing(UNKNOWN, &[(Family::Compx, None)], now);
        assert_eq!(
            press(&mut flow, now),
            Some(Effect::TryAs(UNKNOWN, Family::Compx))
        );
        flow.on(Event::Tried(TryOutcome::Reading(reading(52))), now);
        // A double-click or held Enter on [Try] lands on [Add] at the same spot.
        assert_eq!(press(&mut flow, now), None);
        assert_eq!(press(&mut flow, now + ADD_GUARD / 2), None);
        assert_eq!(
            flow.view(now).primary.as_deref(),
            Some("Add"),
            "still waiting for the owner"
        );
        assert!(matches!(
            press(&mut flow, now + ADD_GUARD),
            Some(Effect::Add(_))
        ));
    }

    #[test]
    fn off_catalogue_bitmouse_try_confirms_a_receiver() {
        let now = Instant::now();
        let flow = choosing(UNKNOWN, &[(Family::AtkBitmouse, None)], now);
        let view = flow.view(now);
        assert_eq!(view.primary.as_deref(), Some("It is a USB receiver — Try"));
        assert!(
            view.text.contains("Never try a mouse on its cable"),
            "{}",
            view.text
        );
        // The floor countdown keeps the confirmation.
        let mut flow = choosing(
            UNKNOWN,
            &[(Family::AtkBitmouse, Some(Duration::from_secs(3)))],
            now,
        );
        assert_eq!(
            flow.view(now).primary.as_deref(),
            Some("It is a USB receiver — Try in 3 s")
        );
        let late = now + Duration::from_secs(3);
        assert_eq!(
            press(&mut flow, late),
            Some(Effect::TryAs(UNKNOWN, Family::AtkBitmouse))
        );
        // Any other family is plain Try.
        let view = choosing(UNKNOWN, &[(Family::Compx, None)], now).view(now);
        assert_eq!(view.primary.as_deref(), Some("Try"));
        assert!(!view.text.contains("cable"), "{}", view.text);
    }

    #[test]
    fn blocked_catalogue_rows_say_why() {
        let now = Instant::now();
        let cable = protocol::KNOWN
            .iter()
            .find(|device| {
                device.family == Family::AtkBitmouse
                    && device.cable
                    && !device.verified
                    && !is_recovery(device.vid, device.pid)
            })
            .expect("an unverified BITMOUSE cable row");
        let id = (cable.vid, cable.pid);
        let view = choosing(id, &[(Family::AtkBitmouse, None)], now).view(now);
        assert!(view.text.contains(BITMOUSE_CABLE), "{}", view.text);
        assert_eq!(no_choice_reason(UNKNOWN), NO_FAMILY);
        // No catalogue row needs a capture today; any that does gets its own reason.
        for device in protocol::KNOWN.iter().filter(|device| device.needs_capture) {
            assert_eq!(no_choice_reason((device.vid, device.pid)), NEEDS_CAPTURE);
        }
    }

    #[test]
    fn handover_restarts_unless_a_try_is_running() {
        let now = Instant::now();
        let mut flow = new_flow();
        press(&mut flow, now);
        assert_eq!(
            flow.on(Event::Handover(UNKNOWN), now),
            Some(Effect::Inspect(UNKNOWN))
        );
        let mut flow = choosing(UNKNOWN, &[(Family::Compx, None)], now);
        press(&mut flow, now);
        assert_eq!(flow.on(Event::Handover(OTHER), now), None);
        assert!(flow.awaits_worker());
        flow.on(Event::Tried(TryOutcome::NoReply), now);
        assert_eq!(
            flow.on(Event::Handover(OTHER), now),
            Some(Effect::Inspect(OTHER))
        );
    }
}
