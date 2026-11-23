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

/// A named mutex lives as long as the process; the second copy sees it already exists.
#[cfg(windows)]
pub fn another_instance_running() -> bool {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::w;

    // SAFETY: the name is a NUL-terminated literal; the handle is intentionally never closed.
    unsafe {
        match CreateMutexW(None, false, w!("Local\\io.github.ittmchien.juicetray")) {
            Ok(_) => GetLastError() == ERROR_ALREADY_EXISTS,
            // Without a mutex there is no guard; starting is better than refusing to run.
            Err(_) => false,
        }
    }
}
