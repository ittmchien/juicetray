//! Update checks against this project's GitHub Releases. Windows asks GitHub's API once a day
//! (and on request) from short-lived threads and can replace its own exe with the release's
//! (checked against the release's SHA256SUMS.txt); macOS sends nothing and its menu item opens
//! the releases page.

#[cfg(windows)]
mod apply;
#[cfg(any(windows, test))]
pub mod check;
#[cfg(any(windows, test))]
mod install;
#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use self::windows::{spawn_check, spawn_cleanup, spawn_offer, spawn_scheduler};

/// Where releases are listed; also the fallback download page.
pub const RELEASES_URL: &str = "https://github.com/ittmchien/juicetray/releases";

/// First argument of the process an update starts; it waits for this one to release the
/// single-instance lock.
#[cfg(windows)]
pub const UPDATED_FLAG: &str = "--updated-from";

/// Whether this build can check on its own, which decides if the menu offers the switch.
pub const AUTO_CHECK_AVAILABLE: bool = cfg!(windows);

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
