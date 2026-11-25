//! Polls Bluetooth connections (and, on macOS, the power source) to notice changes: macOS on a
//! main-run-loop timer, Windows on a background thread.

use std::collections::BTreeSet;
#[cfg(target_os = "macos")]
use std::{
    cell::{Cell, RefCell},
    ptr::NonNull,
};

#[cfg(target_os = "macos")]
use block2::RcBlock;
#[cfg(target_os = "macos")]
use objc2_foundation::NSTimer;
use tao::event_loop::EventLoopProxy;

use crate::app::UserEvent;
#[cfg(target_os = "macos")]
use crate::pause;

#[cfg(target_os = "macos")]
const POLL_SECONDS: f64 = 3.0;
// A longer tick than macOS: the SetupAPI walk costs more, and the worker's settle read adds 3 s.
#[cfg(windows)]
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Sends `UserEvent::BluetoothChanged` when the connected set differs from the last poll, and
/// `UserEvent::PowerChanged` when the power source does (one cheap IOKit read per tick).
// ponytail: polls the connected set (~1 ms per tick); swap for a reliable OS notification if one
// appears. Power rides the same tick, so there is no run-loop or C callback code.
#[cfg(target_os = "macos")]
pub fn watch(proxy: EventLoopProxy<UserEvent>) {
    let last = RefCell::new(None);
    let last_power = Cell::new(pause::current_power_source());
    let poll = RcBlock::new(move |_: NonNull<NSTimer>| {
        let source = pause::current_power_source();
        if last_power.replace(source) != source {
            // A closed event loop means the app is exiting.
            let _ = proxy.send_event(UserEvent::PowerChanged);
        }
        let now = bluetooth::connected_ids();
        if changed(&mut last.borrow_mut(), now) {
            // A closed event loop means the app is exiting.
            let _ = proxy.send_event(UserEvent::BluetoothChanged);
        }
    });
    // SAFETY: the timer is scheduled on the main run loop and fires there, so the non-Send
    // block never leaves the main thread; the run loop retains the timer for the process lifetime.
    let timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(POLL_SECONDS, true, &poll) };
    // Lets macOS coalesce this wake-up with others to save power.
    timer.setTolerance(1.0);
}

/// Sends `UserEvent::BluetoothChanged` when the connected set differs from the last poll.
// ponytail: polls the SetupAPI connected flag every 5 s; a WinRT DeviceWatcher would be
// event-driven, swap it in if idle wake-ups matter.
#[cfg(windows)]
pub fn watch(proxy: EventLoopProxy<UserEvent>) {
    let spawned = std::thread::Builder::new()
        .name("bluetooth-poll".into())
        .spawn(move || {
            crate::efficiency::enter_background_mode();
            let mut last = None;
            // A failing read logs once per streak, not every tick.
            let mut failing = false;
            loop {
                std::thread::sleep(POLL_INTERVAL);
                match bluetooth::connected_ids() {
                    Ok(now) => {
                        failing = false;
                        // A closed event loop means the app is exiting.
                        if changed(&mut last, now)
                            && proxy.send_event(UserEvent::BluetoothChanged).is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        if !failing {
                            crate::log::line(&format!("bluetooth poll failed: {error:#}"));
                        }
                        failing = true;
                    }
                }
            }
        });
    if let Err(error) = spawned {
        crate::log::line(&format!("bluetooth poll thread not started: {error}"));
    }
}

/// Stores `now` as the baseline; true when an earlier baseline exists and differs.
fn changed(last: &mut Option<BTreeSet<String>>, now: BTreeSet<String>) -> bool {
    let differs = last.as_ref().is_some_and(|before| *before != now);
    *last = Some(now);
    differs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn first_poll_is_baseline_then_differences_report() {
        let mut last = None;
        assert!(!changed(&mut last, set(&["a"])));
        assert!(!changed(&mut last, set(&["a"])));
        assert!(changed(&mut last, set(&["a", "b"])));
        assert!(changed(&mut last, set(&[])));
    }
}
