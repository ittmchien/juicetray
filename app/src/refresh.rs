use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use tao::event_loop::EventLoopProxy;
use ui_model::DeviceView;

use crate::app::UserEvent;
use crate::config::Config;
use crate::log;
use crate::pause::{self, PowerState};
use crate::receivers::read_receivers;

pub enum Command {
    RefreshNow,
    SetInterval(Option<u32>),
    /// The power source or the battery option changed; a pause ending earns one read.
    SetPower(PowerState),
    /// Looks for devices to offer in the new-device popup among these plugged-in ids, from the
    /// device lists alone; answers with `UserEvent::Offers`. Devices present at start are never
    /// offered, so the ids always come from a plug-in.
    #[cfg(windows)]
    FindOffers(Vec<crate::plug::Id>),
    /// The Add window asks for a device's interfaces and matching families; a closed reply
    /// channel (the window closed) is ignored.
    #[cfg(windows)]
    Inspect {
        id: crate::plug::Id,
        reply: Sender<crate::add_device::Inspection>,
    },
    /// The Add window's one try: a single battery query as `family`; same reply rule.
    #[cfg(windows)]
    TryAs {
        id: crate::plug::Id,
        family: protocol::Family,
        reply: Sender<crate::add_device::TryOutcome>,
    },
}

/// Starts the worker; it reads at once, then on `RefreshNow` and
/// every `interval` seconds while `power` lets the timer run.
pub fn spawn_worker(
    proxy: EventLoopProxy<UserEvent>,
    interval: Option<u32>,
    power: PowerState,
) -> Result<Sender<Command>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("refresh".into())
        .spawn(move || run(&proxy, &receiver, interval, power))
        .context("start refresh thread")?;
    Ok(sender)
}

/// Blocks between reads (no timer thread, no polling); a changed interval restarts the wait
/// without a read, a paused timer waits for a command only, and the loop ends when the
/// app drops the sender or the event loop is gone.
fn run(
    proxy: &EventLoopProxy<UserEvent>,
    commands: &Receiver<Command>,
    mut interval: Option<u32>,
    mut power: PowerState,
) {
    crate::efficiency::enter_background_mode();
    let mut read_now = true;
    // The last Bluetooth list that was read; a failed Bluetooth read sends it again.
    let mut bluetooth_devices = Vec::new();
    // The timer counts from here, so commands that read nothing never push the next read back.
    let mut timer_from = Instant::now();
    // Shapes the Add window read, kept until their device leaves.
    #[cfg(windows)]
    let mut shapes = door::ShapeCache::default();
    log::trace(format_args!("worker start {power:?}"));
    loop {
        if read_now {
            let _read = log::span("read cycle");
            {
                let _span = log::span("bluetooth read");
                match bluetooth::read_devices() {
                    Ok(devices) => bluetooth_devices = devices,
                    Err(error) => {
                        eprintln!("bluetooth read failed: {error:#}");
                        log::line(&format!("bluetooth read failed: {error:#}"));
                    }
                }
            }
            // The file is read once per cycle, so hand edits to the device keys apply without a
            // restart; the app takes them from this read.
            let config = Config::load();
            let receivers = {
                let _span = log::span("receivers read");
                read_receivers(&config.allowed(), &config.custom(), SystemTime::now())
            };
            if !send_devices(proxy, &bluetooth_devices, &receivers, config) {
                log::trace(format_args!("worker exit: event loop gone"));
                return;
            }
            timer_from = Instant::now();
        }
        read_now = true;
        // While the timer may not read, the wait has no timeout and only a command wakes it.
        let timer = interval.filter(|_| pause::timer_may_read(power));
        let command = wait(commands, timer, timer_from);
        log::trace(format_args!("worker woke: {}", wake_reason(&command)));
        match command {
            Ok(Command::RefreshNow) => {}
            Err(RecvTimeoutError::Timeout) => {}
            // A new interval or power state restarts the timer.
            Ok(Command::SetInterval(seconds)) => {
                interval = seconds;
                read_now = false;
                timer_from = Instant::now();
            }
            Ok(Command::SetPower(next)) => {
                log::trace(format_args!("power {power:?} -> {next:?}"));
                read_now = pause::reads_on_change(power, next);
                power = next;
                timer_from = Instant::now();
            }
            #[cfg(windows)]
            Ok(Command::FindOffers(ids)) => {
                read_now = false;
                let offers = crate::plug::find_offers(&ids, &Config::load());
                if !offers.is_empty() && proxy.send_event(UserEvent::Offers(offers)).is_err() {
                    log::trace(format_args!("worker exit: event loop gone"));
                    return;
                }
            }
            #[cfg(windows)]
            Ok(Command::Inspect { id, reply }) => {
                read_now = false;
                let _ = reply.send(crate::add_device::inspect(id, &mut shapes));
            }
            #[cfg(windows)]
            Ok(Command::TryAs { id, family, reply }) => {
                read_now = false;
                let _ = reply.send(crate::add_device::try_as(id, family));
            }
            Err(RecvTimeoutError::Disconnected) => {
                log::trace(format_args!("worker exit: app dropped the sender"));
                return;
            }
        }
    }
}

/// What ended the wait, for the trace.
fn wake_reason(command: &Result<Command, RecvTimeoutError>) -> &'static str {
    match command {
        Ok(Command::RefreshNow) => "RefreshNow",
        Ok(Command::SetInterval(_)) => "SetInterval",
        Ok(Command::SetPower(_)) => "SetPower",
        #[cfg(windows)]
        Ok(Command::FindOffers(_)) => "FindOffers",
        #[cfg(windows)]
        Ok(Command::Inspect { .. }) => "Inspect",
        #[cfg(windows)]
        Ok(Command::TryAs { .. }) => "TryAs",
        Err(RecvTimeoutError::Timeout) => "timeout",
        Err(RecvTimeoutError::Disconnected) => "disconnected",
    }
}

/// False when the event loop is gone.
fn send_devices(
    proxy: &EventLoopProxy<UserEvent>,
    bluetooth_devices: &[DeviceView],
    receivers: &[DeviceView],
    config: Config,
) -> bool {
    let devices = bluetooth_devices.iter().chain(receivers).cloned().collect();
    let _span = log::span("send devices to the event loop");
    proxy
        .send_event(UserEvent::Devices(devices, Box::new(config)))
        .is_ok()
}

/// Waits until `interval` after `from` (a fixed deadline, whatever wakes it meanwhile); auto-refresh
/// Off waits for a command forever.
fn wait(
    commands: &Receiver<Command>,
    interval: Option<u32>,
    from: Instant,
) -> Result<Command, RecvTimeoutError> {
    match interval {
        Some(seconds) => commands
            .recv_timeout(Duration::from_secs(u64::from(seconds)).saturating_sub(from.elapsed())),
        None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
    }
}
