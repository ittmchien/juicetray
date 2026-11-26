//! Update checks against this project's GitHub Releases: once a day (and on request) from
//! short-lived threads. Windows can replace its own exe with the release's (checked against the
//! release's SHA256SUMS.txt); macOS upgrades a Homebrew install with `brew upgrade` and otherwise
//! opens the download page.

#[cfg(windows)]
mod apply;
pub mod check;
mod common;
#[cfg(any(windows, test))]
mod install;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use self::common::show_outcome;
pub use self::common::{spawn_check, spawn_scheduler};
#[cfg(target_os = "macos")]
pub use self::macos::{offer, offer_move_to_applications};
#[cfg(windows)]
pub use self::windows::{shell_open, spawn_cleanup, spawn_offer};
// The remote catalogue lookup shares the GitHub-only HTTPS GET.
#[cfg(windows)]
pub(crate) use self::windows::get_within;

/// The step an "Update now" install is on, carrying the version it installs; the menu shows it
/// as a disabled top line. Steps only: neither brew nor the download reports a percentage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStage {
    /// `brew upgrade` is running.
    #[cfg(target_os = "macos")]
    Homebrew(String),
    /// The release's exe and checksum list are downloading.
    #[cfg(windows)]
    Downloading(String),
    /// The new copy replaces the running one; the app then exits and restarts at once.
    Installing(String),
}

impl UpdateStage {
    /// The menu line for this step.
    pub fn text(&self) -> String {
        match self {
            #[cfg(target_os = "macos")]
            Self::Homebrew(version) => format!("Updating to v{version} via Homebrew…"),
            #[cfg(windows)]
            Self::Downloading(version) => format!("Downloading v{version}…"),
            Self::Installing(version) => format!("Installing v{version}…"),
        }
    }
}

/// Where releases are listed; also the fallback download page.
pub const RELEASES_URL: &str = "https://github.com/ittmchien/juicetray/releases";

/// First argument of the process an update starts; it waits for this one to release the
/// single-instance lock.
#[cfg(any(windows, target_os = "macos"))]
pub const UPDATED_FLAG: &str = "--updated-from";

/// Opens `url` in the default browser from a short-lived thread, so a slow shell never stalls
/// the caller.
#[cfg(not(windows))]
pub fn open_url(url: &str) {
    let url = url.to_owned();
    let spawned = std::thread::Builder::new()
        .name("open-url".into())
        .spawn(move || {
            if let Err(error) = open_in_browser(&url) {
                eprintln!("opening {url} failed: {error}");
            }
        });
    if let Err(error) = spawned {
        eprintln!("open-url thread failed: {error}");
    }
}

#[cfg(target_os = "macos")]
fn open_in_browser(url: &str) -> Result<(), String> {
    let status = std::process::Command::new("open")
        .arg(url)
        .status()
        .map_err(|error| error.to_string())?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("open exited with {status}"))
}
