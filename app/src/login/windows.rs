use anyhow::Result;
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegGetValueW, RegSetValueExW,
};
use windows::core::{PCWSTR, w};

use super::{LoginState, run_value};

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const VALUE_NAME: PCWSTR = w!("Juicetray");

pub fn login_state() -> LoginState {
    // SAFETY: both strings are NUL-terminated literals; no output buffer is requested.
    let found = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            VALUE_NAME,
            RRF_RT_REG_SZ,
            None,
            None,
            None,
        )
    };
    if found.is_ok() {
        LoginState::Enabled
    } else {
        LoginState::Disabled
    }
}

/// Writes or deletes the per-user Run value that points at this executable.
pub fn set_start_at_login(enabled: bool) -> Result<()> {
    if enabled {
        return write_run_value(&run_value(&std::env::current_exe()?));
    }
    // SAFETY: both strings are NUL-terminated literals.
    let deleted = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE_NAME) };
    // A missing value already means disabled.
    if deleted == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    Ok(deleted.ok()?)
}

fn write_run_value(value: &str) -> Result<()> {
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes: Vec<u8> = wide.iter().flat_map(|unit| unit.to_le_bytes()).collect();
    let mut key = HKEY::default();
    // SAFETY: `key` receives an open handle that is closed below; the buffers outlive the calls.
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        let written = RegSetValueExW(key, VALUE_NAME, None, REG_SZ, Some(&bytes));
        // The write result matters more than a failed close of a just-written key.
        let _ = RegCloseKey(key);
        Ok(written.ok()?)
    }
}
