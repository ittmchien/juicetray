//! Windows side of the update check: HTTPS GETs through WinHTTP, the threads that run them, the
//! update dialogs and the browser launch. Nothing here runs on the UI, taskbar or device-refresh
//! threads.

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use tao::event_loop::EventLoopProxy;
use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_OPTION_URL, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle,
    WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpQueryOption,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

use super::apply::{self, Failure};
use super::check::{self, UpdateCheck, UpdateInfo};
use super::install::is_github_url;
use crate::app::UserEvent;
use crate::dialog::{self, Answer};
use crate::log;

const API_URL: &str = "https://api.github.com/repos/ittmchien/juicetray/releases/latest";
const ACCEPT_HEADER: &str = "Accept: application/vnd.github+json\r\n";
/// Applies to resolving, connecting, sending and each receive.
const TIMEOUT_MS: i32 = 10_000;
/// A release reply is a few KB; the tag and page link come long before this cap.
const MAX_BODY_BYTES: usize = 512 * 1024;
const DIALOG_TITLE: &str = "Check for updates";
const BUSY_MESSAGE: &str = "An update check or install is already in progress.";

/// Time from launch to the first automatic check, so start-up stays quiet.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// One check (and its dialog) at a time; a click or tick meanwhile is dropped.
static CHECKING: AtomicBool = AtomicBool::new(false);
/// One offer (its dialog and, on "Update now", the install) at a time.
static OFFERING: AtomicBool = AtomicBool::new(false);

/// How many times, half a second apart, the start-up cleanup retries deleting the old exe.
const CLEANUP_TRIES: u32 = 20;
const CLEANUP_RETRY_DELAY: Duration = Duration::from_millis(500);

/// Deletes the `<exe>.old` an update left behind, from a short-lived thread: the previous
/// process may still be exiting or a scanner may hold the file, so it retries and then gives
/// up quietly (the next start tries again).
pub fn spawn_cleanup() {
    let spawned = thread::Builder::new()
        .name("update-cleanup".into())
        .spawn(|| {
            let Ok(exe) = std::env::current_exe() else {
                return;
            };
            let backup = super::install::backup_path(&exe);
            for _ in 0..CLEANUP_TRIES {
                match std::fs::remove_file(&backup) {
                    Ok(()) => return,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                    Err(_) => thread::sleep(CLEANUP_RETRY_DELAY),
                }
            }
        });
    if let Err(error) = spawned {
        eprintln!("update cleanup thread failed: {error}");
    }
}

/// Sends `UserEvent::UpdateDue` once after `FIRST_CHECK_DELAY` and then every `CHECK_INTERVAL`
/// from one sleeping thread: no timer or polling on the event loop, and the UI thread decides
/// whether a check may run (setting, power).
/// shortcut: the sleep clock stops while the PC is suspended, so a day counts awake time only.
pub fn spawn_scheduler(proxy: EventLoopProxy<UserEvent>) {
    let spawned = thread::Builder::new()
        .name("update-schedule".into())
        .spawn(move || {
            thread::sleep(FIRST_CHECK_DELAY);
            // A closed event loop means the app is exiting.
            while proxy.send_event(UserEvent::UpdateDue).is_ok() {
                thread::sleep(CHECK_INTERVAL);
            }
        });
    if let Err(error) = spawned {
        eprintln!("update scheduler thread failed: {error}");
    }
}

/// Runs one check on its own thread and reports it as `UserEvent::UpdateChecked`; a manual
/// check then shows its answer in a dialog on that same thread.
pub fn spawn_check(proxy: EventLoopProxy<UserEvent>, manual: bool) {
    if CHECKING.swap(true, Ordering::Relaxed) {
        // A click that did nothing looks broken; the scheduler's tick stays silent.
        if manual {
            spawn_busy_notice();
        }
        return;
    }
    let spawned = thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let _span = log::span("update check");
            let outcome = fetch_latest().and_then(|(status, body)| {
                check::evaluate(status, &body, env!("CARGO_PKG_VERSION"))
            });
            // A closed event loop means the app is exiting.
            let _ = proxy.send_event(UserEvent::UpdateChecked {
                manual,
                outcome: outcome.clone(),
            });
            if manual {
                show_outcome(&outcome, &proxy);
            }
            CHECKING.store(false, Ordering::Relaxed);
        });
    if let Err(error) = spawned {
        CHECKING.store(false, Ordering::Relaxed);
        eprintln!("update check thread failed: {error}");
    }
}

/// Tells the owner a check or offer is already under way, from its own thread because the
/// notice blocks until closed and the caller is the UI thread.
fn spawn_busy_notice() {
    let spawned = thread::Builder::new()
        .name("update-busy".into())
        .spawn(|| dialog::notice(DIALOG_TITLE, BUSY_MESSAGE));
    if let Err(error) = spawned {
        eprintln!("update notice thread failed: {error}");
    }
}

/// Offers `info` from its own thread: the menu line's click, so the UI thread never waits on
/// a dialog or a download.
pub fn spawn_offer(proxy: EventLoopProxy<UserEvent>, info: UpdateInfo) {
    let spawned = thread::Builder::new()
        .name("update-offer".into())
        .spawn(move || offer(&info, &proxy));
    if let Err(error) = spawned {
        eprintln!("update offer thread failed: {error}");
    }
}

/// The manual check's answer; blocks this check thread until the owner closes the dialog.
fn show_outcome(outcome: &Result<UpdateCheck, String>, proxy: &EventLoopProxy<UserEvent>) {
    match outcome {
        Ok(UpdateCheck::Available(info)) => offer(info, proxy),
        Ok(UpdateCheck::UpToDate) => dialog::notice(
            DIALOG_TITLE,
            &format!("You're up to date (v{}).", env!("CARGO_PKG_VERSION")),
        ),
        Err(error) => dialog::notice(
            DIALOG_TITLE,
            &format!("Could not check for updates: {error}"),
        ),
    }
}

/// Asks what to do about a newer release and does it; blocks the calling thread until the
/// owner answers (and, for "Update now", until the install ends). A second offer meanwhile only
/// gets a notice. Without both release assets the only action is the download page.
fn offer(info: &UpdateInfo, proxy: &EventLoopProxy<UserEvent>) {
    if OFFERING.swap(true, Ordering::Relaxed) {
        dialog::notice(DIALOG_TITLE, BUSY_MESSAGE);
        return;
    }
    let message = format!("Juicetray v{} is available.", info.version);
    match &info.assets {
        Some(assets) => {
            let answer = dialog::confirm_or_alt(
                DIALOG_TITLE,
                &message,
                "Update now",
                "Open download page",
                "Later",
            );
            match answer {
                Answer::Ok => update_now(info, assets, proxy),
                Answer::Alt => open_page(&info.url),
                Answer::Cancel => {}
            }
        }
        None => {
            if dialog::confirm(DIALOG_TITLE, &message, "Open download page", "Later") {
                open_page(&info.url);
            }
        }
    }
    OFFERING.store(false, Ordering::Relaxed);
}

/// Installs the release and asks the UI thread to exit once the new exe runs; a failure is
/// shown in a dialog and leaves the current exe in place.
fn update_now(info: &UpdateInfo, assets: &check::UpdateAssets, proxy: &EventLoopProxy<UserEvent>) {
    let _span = log::span("update install");
    match apply::install(assets, env!("CARGO_PKG_VERSION")) {
        // A closed event loop means the app is exiting anyway.
        Ok(()) => {
            let _ = proxy.send_event(UserEvent::UpdateRestart);
        }
        Err(Failure::ReadOnlyFolder(reason)) => {
            log::line(&format!("update: folder not writable: {reason}"));
            offer_download_page(
                info,
                "Juicetray can't replace itself in the folder it runs from.                  Open the download page to update by hand?",
            );
        }
        Err(Failure::Other(reason)) => {
            log::line(&format!("update failed: {reason}"));
            offer_download_page(
                info,
                &format!(
                    "The update failed and the installed version is unchanged: {reason}.                      Open the download page to update by hand?"
                ),
            );
        }
        Err(Failure::RestoreFailed(reason)) => {
            log::line(&format!("update failed, restore failed: {reason}"));
            offer_download_page(
                info,
                &format!(
                    "The update failed and Juicetray could not put the previous version back:                      {reason}. Open the download page to reinstall?"
                ),
            );
        }
    }
}

/// Shows `message` with the download page as the one action.
fn offer_download_page(info: &UpdateInfo, message: &str) {
    if dialog::confirm(DIALOG_TITLE, message, "Open download page", "Later") {
        open_page(&info.url);
    }
}

fn open_page(url: &str) {
    if let Err(error) = shell_open(url) {
        eprintln!("opening {url} failed: {error}");
    }
}

/// Closes a WinHTTP handle when dropped.
struct Handle(*mut c_void);

impl Handle {
    /// WinHTTP reports a failed open with a null handle and the error on the thread.
    fn new(raw: *mut c_void, step: &str) -> Result<Self, String> {
        if raw.is_null() {
            Err(format!(
                "{step} failed: {}",
                windows::core::Error::from_thread()
            ))
        } else {
            Ok(Self(raw))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful WinHTTP open and is closed once, children first.
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

/// GETs the latest-release document: its HTTP status and, for a 200, its body. A body past the
/// cap is cut short, which only loses the assets (the tag and page link come first).
fn fetch_latest() -> Result<(u16, String), String> {
    let reply = get(API_URL, Some(ACCEPT_HEADER), MAX_BODY_BYTES)?;
    Ok((
        reply.status,
        String::from_utf8_lossy(&reply.body).into_owned(),
    ))
}

/// An HTTPS reply: its status and, for a 200, the body read up to the caller's cap.
pub(super) struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    /// More bytes were on offer than the cap allowed.
    pub truncated: bool,
}

/// GETs `url` (HTTPS only), following redirects. The final URL must still be on a GitHub host,
/// or the reply is refused; a non-200 status comes back with an empty body.
pub(super) fn get(url: &str, accept: Option<&str>, max_bytes: usize) -> Result<Reply, String> {
    let step =
        |step: &'static str| move |error: windows::core::Error| format!("{step} failed: {error}");
    if !is_github_url(url) {
        return Err(format!("refusing to fetch {url}"));
    }
    // `is_github_url` guarantees the scheme and a plain host name.
    let rest = &url["https://".len()..];
    let host_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, path) = rest.split_at(host_len);
    let path = HSTRING::from(if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    });
    let host = HSTRING::from(host);
    let agent = HSTRING::from(format!("juicetray/{}", env!("CARGO_PKG_VERSION")));
    // Without proxy settings of its own, WinHTTP follows the system's (including auto-detect).
    // SAFETY: the agent is a NUL-terminated string that outlives the call; no proxy names.
    let session = Handle::new(
        unsafe {
            WinHttpOpen(
                &agent,
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            )
        },
        "WinHTTP open",
    )?;
    // SAFETY: `session` is a live session handle.
    unsafe { WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS) }
        .map_err(step("setting the timeouts"))?;
    // SAFETY: `session` is live and `host` is a NUL-terminated string that outlives the call.
    let connection = Handle::new(
        unsafe { WinHttpConnect(session.0, &host, INTERNET_DEFAULT_HTTPS_PORT, 0) },
        "connecting",
    )?;
    // SAFETY: `connection` is live; the verb and `path` are NUL-terminated strings that outlive
    // the call; null version, referrer and accept types pick the defaults.
    let request = Handle::new(
        unsafe {
            WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                &path,
                PCWSTR::null(),
                PCWSTR::null(),
                null(),
                WINHTTP_FLAG_SECURE,
            )
        },
        "opening the request",
    )?;
    let headers: Option<Vec<u16>> = accept.map(|header| header.encode_utf16().collect());
    // SAFETY: `request` is live; `headers` outlives the call and there is no request body.
    unsafe { WinHttpSendRequest(request.0, headers.as_deref(), None, 0, 0, 0) }
        .map_err(step("sending the request"))?;
    // SAFETY: `request` is live; the reserved pointer must be null.
    unsafe { WinHttpReceiveResponse(request.0, null_mut()) }
        .map_err(step("waiting for the reply"))?;

    // WinHTTP follows redirects on its own (never from HTTPS down to HTTP); the page it ended
    // on must still be GitHub's.
    let mut final_url = vec![0_u16; 8192];
    let mut url_bytes = (final_url.len() * size_of::<u16>()) as u32;
    // SAFETY: the buffer holds `url_bytes` bytes; for this option the call writes a string.
    unsafe {
        WinHttpQueryOption(
            request.0,
            WINHTTP_OPTION_URL,
            Some(final_url.as_mut_ptr().cast()),
            &raw mut url_bytes,
        )
    }
    .map_err(step("reading the final URL"))?;
    let final_url = String::from_utf16_lossy(&final_url[..url_bytes as usize / size_of::<u16>()]);
    if !is_github_url(final_url.trim_end_matches('\0')) {
        return Err("the download was redirected away from GitHub".to_owned());
    }

    let mut status = 0_u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: with the NUMBER flag the buffer is one `u32` and `size` says so; no header name,
    // no index.
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &raw mut size,
            null_mut(),
        )
    }
    .map_err(step("reading the status"))?;
    let status = u16::try_from(status).map_err(|_| format!("odd HTTP status {status}"))?;
    if status != 200 {
        return Ok(Reply {
            status,
            body: Vec::new(),
            truncated: false,
        });
    }

    let mut body = Vec::new();
    let mut chunk = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let mut read = 0_u32;
        // SAFETY: `chunk` is valid for its length and `read` for one `u32`.
        unsafe {
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &raw mut read,
            )
        }
        .map_err(step("reading the reply"))?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read as usize]);
        if body.len() > max_bytes {
            body.truncate(max_bytes);
            truncated = true;
            break;
        }
    }
    Ok(Reply {
        status,
        body,
        truncated,
    })
}

/// Opens `url` with the shell's default handler (the browser).
pub fn shell_open(url: &str) -> Result<(), String> {
    let url = HSTRING::from(url);
    // SAFETY: the verb and `url` are NUL-terminated strings that outlive the call; no owner
    // window, parameters or directory.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &url,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // The legacy return value is a pseudo-handle above 32 on success, an error code otherwise.
    let code = result.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("the shell returned {code}"))
    }
}
