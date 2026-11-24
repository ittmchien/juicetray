//! When the app may touch devices: a laptop on battery pauses the auto-refresh timer. The rules
//! are pure; only the start-up probe reads the OS.

#[cfg(any(windows, test))]
use std::time::{Duration, Instant};

/// Opening the menu refreshes only when the last refresh finished at least this long ago.
#[cfg(any(windows, test))]
pub const MENU_REFRESH_MIN_GAP: Duration = Duration::from_secs(10);

/// `BatteryFlag` bit that marks a machine without a system battery.
#[cfg(any(windows, test))]
const NO_BATTERY_FLAG: u8 = 128;
/// `BatteryFlag` / `ACLineStatus` value for "unknown".
#[cfg(any(windows, test))]
const UNKNOWN: u8 = 255;

/// Where the machine gets its power; only a laptop that is unplugged counts as `Battery`.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PowerSource {
    /// No system battery (a desktop), or the OS could not say: never pauses anything.
    #[default]
    NoBattery,
    Ac,
    Battery,
}

/// Everything that decides whether the auto-refresh timer may read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PowerState {
    pub source: PowerSource,
    /// The owner allows auto-refresh on battery.
    pub refresh_on_battery: bool,
}

/// Whether the auto-refresh timer may read: a laptop on battery waits for AC unless the owner
/// allowed battery refresh. Requested reads (start, Refresh now, menu open, plug-in) always run.
pub fn timer_may_read(state: PowerState) -> bool {
    state.source != PowerSource::Battery || state.refresh_on_battery
}

/// Whether going from `before` to `after` ends a pause and so earns one immediate read: the
/// laptop is back on AC. Turning the battery option on only restarts the timer.
pub fn reads_on_change(before: PowerState, after: PowerState) -> bool {
    before.source == PowerSource::Battery && after.source != PowerSource::Battery
}

/// The power source from `GetSystemPowerStatus` fields: `ACLineStatus` 0 = unplugged, 1 =
/// plugged in, 255 = unknown; `BatteryFlag` bit 128 = no battery, 255 = unknown.
#[cfg(any(windows, test))]
pub fn power_source(ac_line_status: u8, battery_flag: u8) -> PowerSource {
    if battery_flag == UNKNOWN || battery_flag & NO_BATTERY_FLAG != 0 {
        return PowerSource::NoBattery;
    }
    match ac_line_status {
        0 => PowerSource::Battery,
        _ => PowerSource::Ac,
    }
}

/// Whether the last refresh is old enough for a menu open to refresh again.
#[cfg(any(windows, test))]
pub fn menu_refresh_due(last_refresh: Option<Instant>, now: Instant) -> bool {
    last_refresh.is_none_or(|last| now.saturating_duration_since(last) >= MENU_REFRESH_MIN_GAP)
}

/// The current power source; a failed query counts as no battery, which pauses nothing.
#[cfg(windows)]
pub fn current_power_source() -> PowerSource {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: the out-pointer refers to a local.
    match unsafe { GetSystemPowerStatus(&mut status) } {
        Ok(()) => power_source(status.ACLineStatus, status.BatteryFlag),
        Err(_) => PowerSource::NoBattery,
    }
}

/// Only Windows reads the battery.
#[cfg(not(windows))]
pub fn current_power_source() -> PowerSource {
    PowerSource::NoBattery
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(source: PowerSource, refresh_on_battery: bool) -> PowerState {
        PowerState {
            source,
            refresh_on_battery,
        }
    }

    #[test]
    fn battery_blocks_the_timer_unless_allowed() {
        assert!(!timer_may_read(state(PowerSource::Battery, false)));
        assert!(timer_may_read(state(PowerSource::Battery, true)));
    }

    #[test]
    fn ac_and_machines_without_a_battery_never_pause() {
        for source in [PowerSource::Ac, PowerSource::NoBattery] {
            assert!(timer_may_read(state(source, false)));
        }
    }

    #[test]
    fn leaving_a_pause_reads_once() {
        let idle = state(PowerSource::Ac, false);
        assert!(reads_on_change(state(PowerSource::Battery, false), idle));
        // Staying put, entering battery and the battery option itself read nothing.
        assert!(!reads_on_change(idle, idle));
        assert!(!reads_on_change(idle, state(PowerSource::Battery, false)));
        assert!(!reads_on_change(
            state(PowerSource::Battery, false),
            state(PowerSource::Battery, true)
        ));
    }

    #[test]
    fn power_source_from_system_power_status() {
        // Unplugged laptop, charging laptop, desktop (no battery), unknown values.
        assert_eq!(power_source(0, 1), PowerSource::Battery);
        assert_eq!(power_source(1, 9), PowerSource::Ac);
        assert_eq!(power_source(1, 128), PowerSource::NoBattery);
        assert_eq!(power_source(0, 128), PowerSource::NoBattery);
        assert_eq!(power_source(255, 255), PowerSource::NoBattery);
        assert_eq!(power_source(255, 1), PowerSource::Ac);
    }

    #[test]
    fn menu_refresh_waits_for_the_gap() {
        let start = Instant::now();
        assert!(menu_refresh_due(None, start));
        assert!(!menu_refresh_due(Some(start), start));
        assert!(!menu_refresh_due(
            Some(start),
            start + MENU_REFRESH_MIN_GAP - Duration::from_millis(1)
        ));
        assert!(menu_refresh_due(Some(start), start + MENU_REFRESH_MIN_GAP));
        // A clock that reads earlier than the stamp never underflows.
        assert!(!menu_refresh_due(
            Some(start + Duration::from_secs(5)),
            start
        ));
    }
}
