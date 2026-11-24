//! Single-instance guard and app-bundle detection.

/// True when this process runs from a `.app` bundle (it then has a bundle identifier).
#[cfg(target_os = "macos")]
pub fn is_bundled() -> bool {
    bundle_identifier().is_some()
}

#[cfg(target_os = "macos")]
fn bundle_identifier() -> Option<objc2::rc::Retained<objc2_foundation::NSString>> {
    objc2_foundation::NSBundle::mainBundle().bundleIdentifier()
}

/// Only macOS has app bundles.
#[cfg(not(target_os = "macos"))]
pub fn is_bundled() -> bool {
    false
}

/// Another running app with this bundle id means a second copy was launched.
#[cfg(target_os = "macos")]
pub fn another_instance_running() -> bool {
    use objc2_app_kit::NSRunningApplication;

    let Some(identifier) = bundle_identifier() else {
        return false;
    };
    let own_pid = std::process::id();
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&identifier)
        .iter()
        .any(|app| u32::try_from(app.processIdentifier()).is_ok_and(|pid| pid != own_pid))
}

/// How long an update's restart waits for the exiting process to release the lock; the old
/// process may take a while to exit (it joins the taskbar thread).
#[cfg(windows)]
const RESTART_WAIT: std::time::Duration = std::time::Duration::from_secs(60);
#[cfg(windows)]
const RESTART_POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// A named mutex lives as long as the process; the second copy sees it already exists. The
/// process an update starts runs while its predecessor is still exiting, so it keeps retrying
/// (releasing its own handle each time, or it would keep the mutex alive) for `RESTART_WAIT`.
#[cfg(windows)]
pub fn another_instance_running() -> bool {
    use std::time::Instant;
    use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::w;

    let started_by_update = std::env::args().nth(1).as_deref() == Some(crate::update::UPDATED_FLAG);
    let give_up_at = Instant::now()
        + if started_by_update {
            RESTART_WAIT
        } else {
            Default::default()
        };
    loop {
        // SAFETY: the name is a NUL-terminated literal.
        let mutex =
            unsafe { CreateMutexW(None, false, w!("Local\\io.github.ittmchien.juicetray")) };
        // Without a mutex there is no guard; starting is better than refusing to run.
        let Ok(handle) = mutex else {
            return false;
        };
        // SAFETY: plain thread-local error query, valid right after the call above.
        if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
            // The handle is never closed, so the lock lasts as long as the process.
            return false;
        }
        // SAFETY: `handle` is the handle just opened to the other process's mutex.
        let _ = unsafe { CloseHandle(handle) };
        if Instant::now() >= give_up_at {
            return true;
        }
        std::thread::sleep(RESTART_POLL);
    }
}
