//! Windows side of the update check: one HTTPS GET through WinHTTP, the threads that run it,
//! and the browser launch. Nothing here runs on the UI, taskbar or device-refresh threads.

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use tao::event_loop::EventLoopProxy;
use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

use super::check::{self, UpdateCheck};
use crate::app::UserEvent;
use crate::{dialog, log};

const API_HOST: PCWSTR = w!("api.github.com");
const API_PATH: PCWSTR = w!("/repos/ittmchien/juicetray/releases/latest");
const ACCEPT_HEADER: &str = "Accept: application/vnd.github+json\r\n";
/// Applies to resolving, connecting, sending and each receive.
const TIMEOUT_MS: i32 = 10_000;
/// A release reply is a few KB; the tag and page link come long before this cap.
const MAX_BODY_BYTES: usize = 512 * 1024;
const DIALOG_TITLE: &str = "Check for updates";

/// Time from launch to the first automatic check, so start-up stays quiet.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// One check (and its dialog) at a time; a click or tick meanwhile is dropped.
static CHECKING: AtomicBool = AtomicBool::new(false);

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
                show_outcome(&outcome);
            }
            CHECKING.store(false, Ordering::Relaxed);
        });
    if let Err(error) = spawned {
        CHECKING.store(false, Ordering::Relaxed);
        eprintln!("update check thread failed: {error}");
    }
}

/// The manual check's answer; blocks this check thread until the owner closes the dialog.
fn show_outcome(outcome: &Result<UpdateCheck, String>) {
    match outcome {
        Ok(UpdateCheck::Available(info)) => {
            let message = format!("Juicetray v{} is available.", info.version);
            if dialog::confirm(DIALOG_TITLE, &message, "Open download page", "Later")
                && let Err(error) = shell_open(&info.url)
            {
                eprintln!("opening {} failed: {error}", info.url);
            }
        }
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

/// GETs the latest-release document: its HTTP status and, for a 200, its body.
fn fetch_latest() -> Result<(u16, String), String> {
    let step =
        |step: &'static str| move |error: windows::core::Error| format!("{step} failed: {error}");
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
    // SAFETY: `session` is live and the host is a static NUL-terminated string.
    let connection = Handle::new(
        unsafe { WinHttpConnect(session.0, API_HOST, INTERNET_DEFAULT_HTTPS_PORT, 0) },
        "connecting",
    )?;
    // SAFETY: `connection` is live; the static strings are NUL-terminated; null version,
    // referrer and accept types pick the defaults.
    let request = Handle::new(
        unsafe {
            WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                API_PATH,
                PCWSTR::null(),
                PCWSTR::null(),
                null(),
                WINHTTP_FLAG_SECURE,
            )
        },
        "opening the request",
    )?;
    let headers: Vec<u16> = ACCEPT_HEADER.encode_utf16().collect();
    // SAFETY: `request` is live; `headers` outlives the call and there is no request body.
    unsafe { WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0) }
        .map_err(step("sending the request"))?;
    // SAFETY: `request` is live; the reserved pointer must be null.
    unsafe { WinHttpReceiveResponse(request.0, null_mut()) }
        .map_err(step("waiting for the reply"))?;

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
        return Ok((status, String::new()));
    }

    let mut body = Vec::new();
    let mut chunk = [0_u8; 8192];
    while body.len() < MAX_BODY_BYTES {
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
    }
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
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
