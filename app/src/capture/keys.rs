//! The key filter's watcher (Windows): one shared `keys` thread that keeps only the times of
//! keyboard events from any keyboard, never their input, so no key code is copied or stored.

// ponytail: misses keys typed into elevated windows (unverified) and may look keylogger-like to
// AV; RIDEV_REMOVE also drops tao's unused keyboard registration (app.rs uses no DeviceEvent).

use std::collections::VecDeque;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::{
    RAWINPUTDEVICE, RAWINPUTDEVICE_FLAGS, RIDEV_INPUTSINK, RIDEV_REMOVE, RegisterRawInputDevices,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE,
    MSG, PostThreadMessageW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_INPUT, WM_QUIT,
};
use windows::core::w;

use super::{KEY_WINDOW, KEYBOARD_PAGE, KEYBOARD_USAGES, LISTEN};
use crate::log;

/// Most key times kept; the oldest goes first.
const MAX_TIMES: usize = 4096;

/// The watcher's users and its thread (id for `WM_QUIT`, handle to join).
struct Watcher {
    users: usize,
    running: Option<(u32, JoinHandle<()>)>,
}

static WATCHER: Mutex<Watcher> = Mutex::new(Watcher {
    users: 0,
    running: None,
});
/// Keyboard event times, pruned to `LISTEN + KEY_WINDOW` and at most `MAX_TIMES` entries.
static TIMES: Mutex<VecDeque<Instant>> = Mutex::new(VecDeque::new());

/// A capture's hold on the watcher; dropping the last one stops it.
pub struct Lease(());

/// Holds the watcher, starting it for the first user; a watcher that cannot start keeps nothing.
pub fn acquire() -> Result<Lease, String> {
    let mut watcher = WATCHER.lock().unwrap_or_else(PoisonError::into_inner);
    if watcher.users == 0 {
        watcher.running = Some(spawn()?);
    }
    watcher.users += 1;
    Ok(Lease(()))
}

/// Keyboard events after `after`, oldest first.
pub fn times_after(after: Instant) -> Vec<Instant> {
    TIMES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .copied()
        .filter(|at| *at > after)
        .collect()
}

impl Drop for Lease {
    /// The last user stops and joins the thread before `WATCHER` unlocks, so a new watcher never
    /// registers before the old one is removed (the thread never takes `WATCHER`).
    fn drop(&mut self) {
        let mut watcher = WATCHER.lock().unwrap_or_else(PoisonError::into_inner);
        watcher.users = watcher.users.saturating_sub(1);
        if watcher.users > 0 {
            return;
        }
        let Some((thread_id, handle)) = watcher.running.take() else {
            return;
        };
        let quit = if handle.is_finished() {
            Ok(())
        } else {
            // SAFETY: posting only queues a message; `handle` keeps the thread handle open, so
            // Windows cannot reuse this thread id for another thread.
            unsafe { PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
        };
        match quit {
            Ok(()) => stopped(handle),
            // It may have ended since the check: then it joins as above.
            Err(_) if handle.is_finished() => stopped(handle),
            // A live thread that never gets WM_QUIT would hang the join: it is left running.
            Err(error) => log::line(&format!("key watcher not stopped ({error}); left running")),
        }
    }
}

/// Joins the ended watcher; nothing records any more, so old key times go with it.
fn stopped(handle: JoinHandle<()>) {
    if handle.join().is_err() {
        log::line("key watcher panicked");
    }
    TIMES.lock().unwrap_or_else(PoisonError::into_inner).clear();
}

/// Starts the `keys` thread and waits for its start result; a thread that failed is joined.
fn spawn() -> Result<(u32, JoinHandle<()>), String> {
    let (started, result) = mpsc::channel();
    let handle = thread::Builder::new()
        .name("keys".into())
        .spawn(move || watch(&started))
        .map_err(|error| format!("no key watcher thread: {error}"))?;
    let outcome = match result.recv() {
        Ok(Ok(thread_id)) => return Ok((thread_id, handle)),
        Ok(Err(why)) => why,
        // The thread died before it sent its result.
        Err(_) => "the key watcher stopped while starting".to_owned(),
    };
    if handle.join().is_err() {
        log::line("key watcher panicked while starting");
    }
    Err(outcome)
}

/// The thread: everything holding the `HWND` stays here (`HWND` is not `Send`); only the thread
/// id crosses threads.
fn watch(started: &Sender<Result<u32, String>>) {
    let window = match open_window() {
        Ok(window) => window,
        Err(why) => {
            // `acquire` waits on the receiver, so this send only fails if it is gone too.
            let _ = started.send(Err(why));
            return;
        }
    };
    // SAFETY: no arguments; it only reads the calling thread's id.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _ = started.send(Ok(thread_id));
    pump();
    unregister();
    destroy(window);
}

/// A message-only window (never top-level, so broadcasts and `EnumWindows` never see it) that
/// receives keyboard and keypad Raw Input in the background; any failure undoes what was done.
fn open_window() -> Result<HWND, String> {
    // SAFETY: a message-only window of the system STATIC class; no menu, instance or creation
    // data.
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!(""),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
    }
    .map_err(|error| format!("no key watcher window: {error}"))?;
    let devices = devices(RIDEV_INPUTSINK, window);
    // SAFETY: `devices` is a live array whose element size is passed; `window` is this thread's.
    let registered =
        unsafe { RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32) };
    if let Err(error) = registered {
        unregister();
        destroy(window);
        return Err(format!("keyboard events unavailable: {error}"));
    }
    Ok(window)
}

/// The keyboard and keypad entries with `flags`, sent to `target`.
fn devices(flags: RAWINPUTDEVICE_FLAGS, target: HWND) -> [RAWINPUTDEVICE; 2] {
    KEYBOARD_USAGES.map(|usage| RAWINPUTDEVICE {
        usUsagePage: KEYBOARD_PAGE,
        usUsage: usage,
        dwFlags: flags,
        hwndTarget: target,
    })
}

/// Removes both registrations with the null target `RIDEV_REMOVE` needs; a failure is only
/// logged, since the window goes away next.
fn unregister() {
    let devices = devices(RIDEV_REMOVE, HWND::default());
    // SAFETY: as in `open_window`; a null target is what RIDEV_REMOVE requires.
    let removed = unsafe { RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32) };
    if let Err(error) = removed {
        log::line(&format!(
            "key watcher: removing keyboard events failed: {error}"
        ));
    }
}

fn destroy(window: HWND) {
    // SAFETY: `window` was created on this thread and is destroyed once.
    if let Err(error) = unsafe { DestroyWindow(window) } {
        log::line(&format!("key watcher window destroy failed: {error}"));
    }
}

/// Until `WM_QUIT`: a `WM_INPUT` keeps its time and goes unread to `DefWindowProcW`; every other
/// message is dispatched.
fn pump() {
    let mut message = MSG::default();
    // SAFETY: `message` outlives each call; only this thread's queue is read. 0 is WM_QUIT and
    // -1 an error, both end the loop.
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
        if message.message == WM_INPUT {
            record(Instant::now());
            // SAFETY: the message was just taken from this thread's queue for its own window.
            unsafe {
                DefWindowProcW(
                    message.hwnd,
                    message.message,
                    message.wParam,
                    message.lParam,
                )
            };
        } else {
            // SAFETY: as above.
            unsafe { DispatchMessageW(&message) };
        }
    }
}

/// Keeps one event time; older than a capture can use or past the cap, the oldest goes.
fn record(at: Instant) {
    let mut times = TIMES.lock().unwrap_or_else(PoisonError::into_inner);
    while times
        .front()
        .is_some_and(|first| at.saturating_duration_since(*first) > LISTEN + KEY_WINDOW)
    {
        times.pop_front();
    }
    if times.len() >= MAX_TIMES {
        times.pop_front();
    }
    times.push_back(at);
}
