use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use tao::event_loop::EventLoopProxy;
use ui_model::DeviceView;

use crate::app::UserEvent;
use crate::config::Config;
use crate::receivers::{CheckNotes, check_receiver, read_receivers};

pub enum Command {
    RefreshNow,
    SetInterval(Option<u32>),
    /// One battery query for the receiver row with this id, asked from the menu.
    CheckReceiver(String),
}

/// Starts the worker; it reads at once, then on `RefreshNow` and every `interval` seconds.
pub fn spawn_worker(
    proxy: EventLoopProxy<UserEvent>,
    interval: Option<u32>,
) -> Result<Sender<Command>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("refresh".into())
        .spawn(move || run(&proxy, &receiver, interval))
        .context("start refresh thread")?;
    Ok(sender)
}

/// Blocks between reads (no timer thread, no polling); a changed interval or a check restarts
/// the wait without a read, and the loop ends when the app drops the sender or the event loop
/// is gone.
fn run(proxy: &EventLoopProxy<UserEvent>, commands: &Receiver<Command>, mut interval: Option<u32>) {
    let mut read_now = true;
    // The last Bluetooth list that was read; a failed Bluetooth read sends it again.
    let mut bluetooth_devices = Vec::new();
    // The last receiver rows, so a check replaces only its own row.
    let mut receivers = Vec::new();
    // The last failed check per row id, shown by later reads until a check succeeds.
    let mut notes = CheckNotes::new();
    loop {
        if read_now {
            match bluetooth::read_devices() {
                Ok(devices) => bluetooth_devices = devices,
                Err(error) => eprintln!("bluetooth read failed: {error:#}"),
            }
            // `allow` is re-read every time, so hand edits apply without a restart.
            receivers = read_receivers(&Config::load().allowed(), &notes, SystemTime::now());
            if !send_devices(proxy, &bluetooth_devices, &receivers) {
                return;
            }
        }
        read_now = true;
        match wait(commands, interval) {
            Ok(Command::RefreshNow) | Err(RecvTimeoutError::Timeout) => {}
            Ok(Command::SetInterval(seconds)) => {
                interval = seconds;
                read_now = false;
            }
            Ok(Command::CheckReceiver(id)) => {
                read_now = false;
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
                if !allowed || !send_devices(proxy, &bluetooth_devices, &receivers) {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
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
) -> bool {
    let devices = bluetooth_devices.iter().chain(receivers).cloned().collect();
    proxy.send_event(UserEvent::Devices(devices)).is_ok()
}

/// Auto-refresh Off waits for a command forever.
fn wait(commands: &Receiver<Command>, interval: Option<u32>) -> Result<Command, RecvTimeoutError> {
    match interval {
        Some(seconds) => commands.recv_timeout(Duration::from_secs(u64::from(seconds))),
        None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
    }
}
