use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use tao::event_loop::EventLoopProxy;

use crate::app::UserEvent;
use crate::config::Config;
use crate::receivers::read_receivers;

pub enum Command {
    RefreshNow,
    SetInterval(Option<u32>),
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

/// Blocks between reads (no timer thread, no polling); a changed interval restarts the wait
/// without a read, and the loop ends when the app drops the sender or the event loop is gone.
fn run(proxy: &EventLoopProxy<UserEvent>, commands: &Receiver<Command>, mut interval: Option<u32>) {
    let mut read_now = true;
    // The last Bluetooth list that was read; a failed Bluetooth read sends it again.
    let mut bluetooth_devices = Vec::new();
    loop {
        if read_now {
            match bluetooth::read_devices() {
                Ok(devices) => bluetooth_devices = devices,
                Err(error) => eprintln!("bluetooth read failed: {error:#}"),
            }
            // `allow` is re-read every time, so hand edits apply without a restart.
            let receivers = read_receivers(&Config::load().allowed(), SystemTime::now());
            let devices = bluetooth_devices.iter().cloned().chain(receivers).collect();
            if proxy.send_event(UserEvent::Devices(devices)).is_err() {
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
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Auto-refresh Off waits for a command forever.
fn wait(commands: &Receiver<Command>, interval: Option<u32>) -> Result<Command, RecvTimeoutError> {
    match interval {
        Some(seconds) => commands.recv_timeout(Duration::from_secs(u64::from(seconds))),
        None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
    }
}
