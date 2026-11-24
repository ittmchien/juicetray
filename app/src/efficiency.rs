//! Efficiency mode for the calling background thread: Windows may run it slower and on efficient
//! cores. Only worker threads call it; the UI, taskbar and menu threads stay at full speed.

/// Asks Windows to power-throttle the calling thread; a failure is logged and changes nothing.
/// No-op on other systems.
#[cfg(windows)]
pub fn enter_background_mode() {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadInformation, THREAD_POWER_THROTTLING_CURRENT_VERSION,
        THREAD_POWER_THROTTLING_EXECUTION_SPEED, THREAD_POWER_THROTTLING_STATE,
        ThreadPowerThrottling,
    };

    let state = THREAD_POWER_THROTTLING_STATE {
        Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
    };
    // SAFETY: the pseudo handle of the calling thread is always valid; `state` is the struct the
    // ThreadPowerThrottling class takes, with its own size, and outlives the call.
    let set = unsafe {
        SetThreadInformation(
            GetCurrentThread(),
            ThreadPowerThrottling,
            (&raw const state).cast(),
            size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
        )
    };
    if let Err(error) = set {
        crate::log::line(&format!("efficiency mode unavailable: {error}"));
    }
}

#[cfg(not(windows))]
pub fn enter_background_mode() {}
