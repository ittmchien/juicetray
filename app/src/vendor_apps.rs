//! Vendor apps that talk to their own brand's devices. While one is open the worker sends nothing
//! to that brand's devices (no open, no query): a second HID or serial client can hang the app.

use crate::log;

/// USB vendor id and the vendor app's UI process name. Only the UI exe counts: the background
/// helpers (`MCHOSE HUB Agent/Watcher/Notifier.exe`, `atk-hub-service.exe`) run while it is closed.
const VENDOR_APPS: [(u16, &str); 4] = [
    (0x3837, "MCHOSE HUB.exe"),
    (0x373B, "ATK HUB.exe"),
    (0x3554, "ATK HUB.exe"),
    (0x391D, "VGN VHUB.exe"),
];

/// The shown name of the open vendor app that owns `vid`'s devices; `running` comes from `running`.
pub fn paused_by(vid: u16, running: &[&str]) -> Option<&'static str> {
    VENDOR_APPS
        .iter()
        .find(|&&(id, exe)| id == vid && running.contains(&exe))
        .map(|&(_, exe)| exe.trim_end_matches(".exe"))
}

/// The vendor apps of `vids` that are running now; the process list is read only when one of
/// `vids` has an app. A failed read counts as none open (logged), as before this check existed.
pub fn running(vids: &[u16]) -> Vec<&'static str> {
    let wanted: Vec<&'static str> = VENDOR_APPS
        .iter()
        .filter(|(id, _)| vids.contains(id))
        .map(|&(_, exe)| exe)
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let _span = log::span("vendor app check");
    match process_names() {
        Ok(names) => open_apps(&wanted, &names),
        Err(error) => {
            log::line(&format!("vendor app check failed: {error:#}"));
            Vec::new()
        }
    }
}

/// The `wanted` exe names found among `names` (Windows file names are case-insensitive).
fn open_apps(wanted: &[&'static str], names: &[String]) -> Vec<&'static str> {
    wanted
        .iter()
        .copied()
        .filter(|exe| names.iter().any(|name| name.eq_ignore_ascii_case(exe)))
        .collect()
}

/// Every process's exe file name, from one Toolhelp snapshot.
#[cfg(windows)]
fn process_names() -> anyhow::Result<Vec<String>> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: plain flags; the handle is closed below on the only path after it opens.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut names = Vec::new();
    // SAFETY: `entry` has its size set and outlives the call.
    let mut found = unsafe { Process32FirstW(snapshot, &mut entry) };
    while found.is_ok() {
        let file = &entry.szExeFile;
        let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
        names.push(String::from_utf16_lossy(&file[..len]));
        // SAFETY: as above; the snapshot is still open.
        found = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    // SAFETY: the snapshot handle came from CreateToolhelp32Snapshot and is closed once.
    let _ = unsafe { CloseHandle(snapshot) };
    Ok(names)
}

/// The vendor apps are Windows-only, so elsewhere none is ever open.
#[cfg(not(windows))]
fn process_names() -> anyhow::Result<Vec<String>> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_brand_pauses_only_for_its_own_ui_app() {
        let mchose = ["MCHOSE HUB.exe"];
        assert_eq!(paused_by(0x3837, &mchose), Some("MCHOSE HUB"));
        assert_eq!(paused_by(0x373B, &mchose), None);
        let atk = ["ATK HUB.exe"];
        assert_eq!(paused_by(0x373B, &atk), Some("ATK HUB"));
        assert_eq!(paused_by(0x3554, &atk), Some("ATK HUB"));
        assert_eq!(paused_by(0x3837, &atk), None);
        assert_eq!(paused_by(0x391D, &["VGN VHUB.exe"]), Some("VGN VHUB"));
        // A brand without a listed app is never paused.
        assert_eq!(paused_by(0x046D, &["ATK HUB.exe", "MCHOSE HUB.exe"]), None);
        assert_eq!(paused_by(0x3837, &[]), None);
    }

    #[test]
    fn background_helpers_do_not_count_as_open() {
        let wanted = ["MCHOSE HUB.exe", "ATK HUB.exe"];
        let helpers: Vec<String> = [
            "MCHOSE HUB Agent.exe",
            "MCHOSE HUB Watcher.exe",
            "MCHOSE HUB Notifier.exe",
            "atk-hub-service.exe",
        ]
        .map(String::from)
        .to_vec();
        assert!(open_apps(&wanted, &helpers).is_empty());
        // The UI exe counts whatever its case.
        let open = vec!["mchose hub.EXE".to_owned()];
        assert_eq!(open_apps(&wanted, &open), ["MCHOSE HUB.exe"]);
    }

    #[test]
    fn no_vendor_vid_reads_no_process_list() {
        assert!(running(&[0x046D, 0x1234]).is_empty());
        assert!(running(&[]).is_empty());
    }
}
