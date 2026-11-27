//! Report byte lengths of one HID collection from `HidP_GetCaps` (Windows; nothing elsewhere).
//! Only preparsed data is read on a zero-access handle, as hidapi does; no report I/O.

#[cfg(windows)]
use windows::Win32::Devices::HumanInterfaceDevice::{
    HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FreePreparsedData, HidD_GetPreparsedData, HidP_GetCaps,
    PHIDP_PREPARSED_DATA,
};
#[cfg(windows)]
use windows::Win32::Foundation::E_FAIL;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
#[cfg(windows)]
use windows::core::{Error, HSTRING, Owned};

/// Access mask 0: the handle can neither read nor write reports.
#[cfg(windows)]
const NO_ACCESS: u32 = 0;

/// The caps text for the collection at `path`: lengths, or why they are unknown.
#[cfg(windows)]
pub fn line(path: &str) -> Option<String> {
    Some(match read(path) {
        Ok(caps) => caps_text(
            caps.InputReportByteLength,
            caps.OutputReportByteLength,
            caps.FeatureReportByteLength,
        ),
        Err(error) => unknown_text(&error.to_string()),
    })
}

/// Other hosts print no caps.
#[cfg(not(windows))]
pub fn line(_path: &str) -> Option<String> {
    None
}

/// Byte lengths include the report id byte, as Windows reports them.
#[cfg(any(windows, test))]
fn caps_text(input: u16, output: u16, feature: u16) -> String {
    format!("caps in {input} out {output} feat {feature}")
}

/// Printed instead of the lengths when the collection's caps cannot be read.
#[cfg(any(windows, test))]
fn unknown_text(error: &str) -> String {
    format!("caps unknown ({error})")
}

/// Preparsed data freed on drop.
#[cfg(windows)]
struct Preparsed(PHIDP_PREPARSED_DATA);

#[cfg(windows)]
impl Drop for Preparsed {
    fn drop(&mut self) {
        // SAFETY: the data came from a successful HidD_GetPreparsedData and is freed only here.
        unsafe { HidD_FreePreparsedData(self.0) };
    }
}

/// Opens the collection with no access, reads its caps and releases every handle on each path.
#[cfg(windows)]
fn read(path: &str) -> windows::core::Result<HIDP_CAPS> {
    // SAFETY: the HSTRING path is NUL-terminated; `Owned` closes the handle this call returns.
    let handle = unsafe {
        Owned::new(CreateFileW(
            &HSTRING::from(path),
            NO_ACCESS,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES::default(),
            None,
        )?)
    };
    let mut data = PHIDP_PREPARSED_DATA::default();
    // SAFETY: `handle` is open; on success `data` is owned by the guard below.
    if !unsafe { HidD_GetPreparsedData(*handle, &mut data) } {
        // HidD_GetPreparsedData is not documented to set the last error, so the text is fixed.
        return Err(Error::new(E_FAIL, "no preparsed data"));
    }
    let data = Preparsed(data);
    let mut caps = HIDP_CAPS::default();
    // SAFETY: `data` is live preparsed data and `caps` a writable HIDP_CAPS.
    let status = unsafe { HidP_GetCaps(data.0, &mut caps) };
    if status != HIDP_STATUS_SUCCESS {
        return Err(status.into());
    }
    Ok(caps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_text_lists_the_three_lengths() {
        assert_eq!(caps_text(33, 33, 0), "caps in 33 out 33 feat 0");
    }

    #[test]
    fn unknown_text_carries_the_error() {
        assert_eq!(
            unknown_text("Access is denied."),
            "caps unknown (Access is denied.)"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn other_hosts_print_nothing() {
        assert_eq!(line("anything"), None);
    }
}
