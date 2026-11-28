//! The platform-neutral half of the update flow: the launch and daily schedule, the check thread
//! and a manual check's answer. Fetching and offering are the platform module's.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use tao::event_loop::EventLoopProxy;

#[cfg(target_os = "macos")]
use super::macos as platform;
#[cfg(windows)]
use super::windows as platform;

use super::check::{self, UpdateCheck};
use crate::app::UserEvent;
use crate::dialog;
use crate::log;

pub(super) const API_URL: &str = "https://api.github.com/repos/ittmchien/juicetray/releases/latest";
pub(super) const DIALOG_TITLE: &str = "Check for updates";
pub(super) const BUSY_MESSAGE: &str = "An update check or install is already in progress.";

/// Time from launch to the first automatic check, so start-up stays quiet.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// One check at a time; a click or tick meanwhile is dropped.
static CHECKING: AtomicBool = AtomicBool::new(false);

/// Sends `UserEvent::UpdateDue` a minute after every launch (so a login start checks too), then
/// once a day from one sleeping thread; the sleep clock stops while the machine sleeps.
pub fn spawn_scheduler(proxy: EventLoopProxy<UserEvent>) {
    let spawned = thread::Builder::new()
        .name("update-schedule".into())
        .spawn(move || {
            let mut wait = FIRST_CHECK_DELAY;
            loop {
                thread::sleep(wait);
                // A closed event loop means the app is exiting.
                if proxy.send_event(UserEvent::UpdateDue).is_err() {
                    return;
                }
                wait = CHECK_INTERVAL;
            }
        });
    if let Err(error) = spawned {
        eprintln!("update scheduler thread failed: {error}");
    }
}

/// Runs one check on its own thread and reports it as `UserEvent::UpdateChecked`. A manual
/// check's answer shows on that thread on Windows, on the main thread (NSAlert) on macOS.
pub fn spawn_check(proxy: EventLoopProxy<UserEvent>, manual: bool) {
    if CHECKING.swap(true, Ordering::Relaxed) {
        // A click that did nothing looks broken; the scheduler's tick stays silent.
        if manual {
            platform::busy_notice();
        }
        return;
    }
    let spawned = thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let _span = log::span("update check");
            let outcome = platform::fetch_latest().and_then(|(status, body)| {
                check::evaluate(status, &body, env!("CARGO_PKG_VERSION"))
            });
            // A closed event loop means the app is exiting.
            let _ = proxy.send_event(UserEvent::UpdateChecked {
                manual,
                outcome: outcome.clone(),
            });
            #[cfg(windows)]
            if manual {
                show_outcome(&outcome, &proxy);
            }
            CHECKING.store(false, Ordering::Relaxed);
        });
    if let Err(error) = spawned {
        CHECKING.store(false, Ordering::Relaxed);
        eprintln!("update check thread failed: {error}");
    }
}

/// The manual check's answer; blocks the calling thread until the owner closes the dialog.
pub fn show_outcome(outcome: &Result<UpdateCheck, String>, proxy: &EventLoopProxy<UserEvent>) {
    match outcome {
        Ok(UpdateCheck::Available(info)) => platform::offer(info, proxy),
        Ok(UpdateCheck::UpToDate) => dialog::notice(
            DIALOG_TITLE,
            &format!("You're up to date (v{}).", env!("CARGO_PKG_VERSION")),
        ),
        Err(error) => dialog::notice(
            DIALOG_TITLE,
            &format!("Could not check for updates: {error}"),
        ),
    }
}
