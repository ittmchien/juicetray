//! Start at login; the OS is the only source of truth, nothing is kept in the config.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use self::windows::{login_state, set_start_at_login};
#[cfg(target_os = "macos")]
pub use macos::{login_state, set_start_at_login};

/// What the OS reports for this app's login item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginState {
    Enabled,
    Disabled,
    /// macOS registered the item but waits for the user to approve it in System Settings.
    #[cfg_attr(windows, allow(dead_code))] // only macOS reports this state
    RequiresApproval,
    /// No login item can exist: not running from an app bundle, or the OS does not know the app.
    #[cfg_attr(windows, allow(dead_code))] // only macOS reports this state
    Unavailable,
}

/// The Run-key value: the executable path in double quotes, no arguments.
#[cfg(any(windows, test))]
fn run_value(exe: &std::path::Path) -> String {
    format!("\"{}\"", exe.display())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn run_value_quotes_path() {
        let exe = Path::new(r"C:\Program Files\Juice Tray\juicetray.exe");
        assert_eq!(
            run_value(exe),
            r#""C:\Program Files\Juice Tray\juicetray.exe""#
        );
    }
}
