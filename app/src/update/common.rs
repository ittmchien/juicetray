//! The platform-neutral half of the update flow: the once-a-day schedule, the check thread and a
//! manual check's answer. Fetching and offering are the platform module's.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

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

/// Sends `UserEvent::UpdateDue` at most once a day from one sleeping thread, counting from the
/// last check stored on disk; the sleep clock stops while the machine sleeps (awake time only).
pub fn spawn_scheduler(proxy: EventLoopProxy<UserEvent>) {
    let spawned = thread::Builder::new()
        .name("update-schedule".into())
        .spawn(move || {
            let mut wait = check::next_check_wait(
                last_check(),
                SystemTime::now(),
                CHECK_INTERVAL,
                FIRST_CHECK_DELAY,
            );
            loop {
                thread::sleep(wait);
                // A closed event loop means the app is exiting.
                if proxy.send_event(UserEvent::UpdateDue).is_err() {
                    return;
                }
                save_last_check(SystemTime::now());
                wait = CHECK_INTERVAL;
            }
        });
    if let Err(error) = spawned {
        eprintln!("update scheduler thread failed: {error}");
    }
}

/// File holding the time of the last automatic check, in whole seconds since the Unix epoch.
fn last_check_path() -> Option<std::path::PathBuf> {
    Some(
        crate::config::project_dirs()?
            .data_local_dir()
            .join("last-update-check"),
    )
}

/// The stored last-check time; a missing or unreadable file means "never".
fn last_check() -> Option<SystemTime> {
    let text = std::fs::read_to_string(last_check_path()?).ok()?;
    let secs: u64 = text.trim().parse().ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

/// Stores the last-check time; failing only means the next start may check a day early.
fn save_last_check(at: SystemTime) {
    let Some(path) = last_check_path() else {
        return;
    };
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let saved = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, secs.to_string()));
    if let Err(error) = saved {
        log::line(&format!(
            "saving the last update check failed: {}",
            error.kind()
        ));
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
