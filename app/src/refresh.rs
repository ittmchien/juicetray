use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use tao::event_loop::EventLoopProxy;
use ui_model::DeviceView;

use crate::app::UserEvent;
use crate::config::Config;
use crate::log;
use crate::pause::{self, PowerState};
use crate::receivers::{CheckNotes, check_receiver, read_receivers};

pub enum Command {
    RefreshNow,
    SetInterval(Option<u32>),
    /// One battery query for the receiver row with this id, asked from the menu.
    CheckReceiver(String),
    /// The power source or the battery option changed; a pause ending earns one read.
    SetPower(PowerState),
}

/// What a `UserEvent::Devices` result covers.
pub enum ReadScope {
    /// A whole read cycle; carries the config file's `allow` as read for it, so the app never
    /// re-reads the file.
    Full { allow: Vec<String> },
    /// One receiver check; the rest are the worker's last rows.
    Single,
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

/// Blocks between reads (no timer thread, no polling); a changed interval or a check restarts
/// the wait without a read, a paused timer waits for a command only, and the loop ends when the
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
    // The last receiver rows, so a check replaces only its own row.
    let mut receivers = Vec::new();
    // The last failed check per row id, shown by later reads until a check succeeds.
    let mut notes = CheckNotes::new();
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
            // The file is read once per cycle, so hand edits to `allow` apply without a restart.
            let config = Config::load();
            {
                let _span = log::span("receivers read");
                receivers = read_receivers(&config.allowed(), &notes, SystemTime::now());
            }
            let scope = ReadScope::Full {
                allow: config.allow,
            };
            if !send_devices(proxy, &bluetooth_devices, &receivers, scope) {
                log::trace(format_args!("worker exit: event loop gone"));
                return;
            }
        }
        read_now = true;
        // While the timer may not read, the wait has no timeout and only a command wakes it.
        let timer = interval.filter(|_| pause::timer_may_read(power));
        let command = wait(commands, timer);
        log::trace(format_args!("worker woke: {}", wake_reason(&command)));
        match command {
            Ok(Command::RefreshNow) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Ok(Command::SetInterval(seconds)) => {
                interval = seconds;
                read_now = false;
            }
            Ok(Command::SetPower(next)) => {
                log::trace(format_args!("power {power:?} -> {next:?}"));
                read_now = pause::reads_on_change(power, next);
                power = next;
            }
            Ok(Command::CheckReceiver(id)) => {
                read_now = false;
                let _span = log::span_fmt(format_args!("check receiver {id}"));
                // An unknown or unplugged id is ignored.
                let Some(checked) =
                    check_receiver(&id, &Config::load().allowed(), SystemTime::now())
                else {
                    continue;
                };
                checked.remember(&mut notes);
                // The app thread owns config writes; it adds the device to `allow` first.
                let allowed = checked.confirm.is_none_or(|(vid, pid)| {
                    proxy.send_event(UserEvent::Allowed(vid, pid)).is_ok()
                });
                replace_row(&mut receivers, checked.row);
                if !allowed
                    || !send_devices(proxy, &bluetooth_devices, &receivers, ReadScope::Single)
                {
                    log::trace(format_args!("worker exit: event loop gone"));
                    return;
                }
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
        Ok(Command::CheckReceiver(_)) => "CheckReceiver",
        Ok(Command::SetPower(_)) => "SetPower",
        Err(RecvTimeoutError::Timeout) => "timeout",
        Err(RecvTimeoutError::Disconnected) => "disconnected",
    }
}

fn replace_row(rows: &mut Vec<DeviceView>, row: DeviceView) {
    match rows.iter_mut().find(|shown| shown.id == row.id) {
        Some(shown) => *shown = row,
        None => rows.push(row),
    }
}

/// False when the event loop is gone.
fn send_devices(
    proxy: &EventLoopProxy<UserEvent>,
    bluetooth_devices: &[DeviceView],
    receivers: &[DeviceView],
    scope: ReadScope,
) -> bool {
    let devices = bluetooth_devices.iter().chain(receivers).cloned().collect();
    let _span = log::span("send devices to the event loop");
    proxy.send_event(UserEvent::Devices(devices, scope)).is_ok()
}

/// Auto-refresh Off waits for a command forever.
fn wait(commands: &Receiver<Command>, interval: Option<u32>) -> Result<Command, RecvTimeoutError> {
    match interval {
        Some(seconds) => commands.recv_timeout(Duration::from_secs(u64::from(seconds))),
        None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
    }
}
