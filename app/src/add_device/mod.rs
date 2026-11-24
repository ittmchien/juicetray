//! The "Add device" window (Windows): menu "Add unsupported device…" or a new-device popup's
//! [OK] opens it on its own thread, one at a time. The steps are the pure `flow`; the device
//! work runs on the refresh worker (`inspect`, `try_as`); the UI thread writes the config.

mod flow;
#[cfg(windows)]
mod window;
#[cfg(windows)]
mod worker;

#[cfg(windows)]
pub use flow::{AddRequest, Inspection, TryOutcome};
#[cfg(windows)]
pub use window::{is_open, open};
#[cfg(windows)]
pub use worker::{inspect, try_as};
