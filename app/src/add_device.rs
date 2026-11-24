//! Entry point of the "Add device" window (own thread, single instance). This build has no
//! window yet: `open` shows a short notice instead, and popups wait while it is open.

use std::sync::atomic::{AtomicBool, Ordering};

use tao::event_loop::EventLoopProxy;

use crate::app::UserEvent;
use crate::plug::Id;
use crate::{dialog, log};

const TITLE: &str = "Add device";
const NOT_READY: &str = "Adding a device is not available in this version yet. Juicetray asks \
                         again the next time this device is plugged in.";

/// Whether the window (or its stand-in notice) is on screen.
static OPEN: AtomicBool = AtomicBool::new(false);

/// Whether new-device popups must wait: the open window detects devices itself.
pub fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

/// Opens the window with `id` preselected on an `add device` thread, which sends
/// `UserEvent::AddDeviceClosed` when it closes; a second call while open does nothing.
pub fn open(proxy: EventLoopProxy<UserEvent>, id: Id) {
    if OPEN.swap(true, Ordering::Relaxed) {
        return;
    }
    log::trace(format_args!("add device {:04X}:{:04X}", id.0, id.1));
    let spawned = std::thread::Builder::new()
        .name("add device".into())
        .spawn(move || {
            dialog::notice(TITLE, NOT_READY);
            OPEN.store(false, Ordering::Relaxed);
            // A closed event loop means the app is exiting.
            let _ = proxy.send_event(UserEvent::AddDeviceClosed);
        });
    if let Err(error) = spawned {
        OPEN.store(false, Ordering::Relaxed);
        log::line(&format!("add device window unavailable: {error}"));
    }
}
