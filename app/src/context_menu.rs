//! Windows context menu on its own short-lived `menu` thread. `TrackPopupMenu` can block forever
//! inside a cross-thread `SendMessage` to another process's hung window; only this thread waits.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use tray_icon::menu::ContextMenu;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, PostMessageW,
    TranslateMessage, WM_NULL, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::w;

use crate::log;
use crate::menu::{self, MenuEntry};

/// A menu open this long counts as stuck: a new click abandons it and the watchdog names hung
/// windows. A real user closes a menu by clicking elsewhere, which ends it well before this.
pub const STUCK_AFTER: Duration = Duration::from_secs(10);

/// The open menu's sequence number and opening time; `None` while no menu is open.
static OPEN: Mutex<Option<(u64, Instant)>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The open menu's sequence number and how long it has been open.
pub fn open_for() -> Option<(u64, Duration)> {
    let open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);
    open.map(|(id, since)| (id, since.elapsed()))
}

/// Opens `entries` at the cursor on a new `menu` thread. A click while a menu is open is ignored,
/// unless that menu is stuck: then it is abandoned and ends whenever Windows releases it.
pub fn show(entries: Vec<MenuEntry>) {
    let id = {
        let mut open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, since)) = *open {
            let age = since.elapsed();
            if age < STUCK_AFTER {
                log::trace(format_args!("menu still open; click ignored"));
                return;
            }
            log::line(&format!(
                "menu stuck for {} ms; abandoning it",
                age.as_millis()
            ));
        }
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        *open = Some((id, Instant::now()));
        id
    };
    let spawned = thread::Builder::new().name("menu".into()).spawn(move || {
        track(&entries);
        close(id);
    });
    if let Err(error) = spawned {
        log::line(&format!("menu thread unavailable: {error}"));
        close(id);
    }
}

/// Clears the open state, unless a newer menu has replaced this (abandoned) one.
fn close(id: u64) {
    let mut open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);
    if matches!(*open, Some((open_id, _)) if open_id == id) {
        *open = None;
    }
}

/// Builds the menu on this thread (muda menus are not `Send`) and tracks it on a hidden owner
/// window of this thread; muda's `TPM_RETURNCMD` path sends the clicked item as a `MenuEvent`.
fn track(entries: &[MenuEntry]) {
    let _span = log::span("context menu");
    // A hidden top-level tool window of the system STATIC class: no class to register, not
    // attached to Explorer's input, and off the taskbar and Alt+Tab.
    // SAFETY: a top-level window of a system class; no parent, menu or creation data.
    let owner = match unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("STATIC"),
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )
    } {
        Ok(owner) => owner,
        Err(error) => {
            log::line(&format!("menu owner window failed: {error}"));
            return;
        }
    };
    let menu = menu::build_menu(entries);
    // Modal: returns when the menu closes; muda makes `owner` foreground first (KB 135788).
    // SAFETY: `owner` is a live window of this thread until `DestroyWindow` below.
    unsafe { menu.show_context_menu_for_hwnd(owner.0 as isize, None) };
    finish(owner);
}

/// KB 135788: a message to the owner after `TrackPopupMenu` ends the menu's task switch; it is
/// pumped here before the window and thread go away.
fn finish(owner: HWND) {
    // SAFETY: `owner` is a live window of this thread; posting only queues a message.
    if let Err(error) = unsafe { PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0)) } {
        log::line(&format!("posting WM_NULL after the menu failed: {error}"));
    }
    let mut message = MSG::default();
    // SAFETY: `message` outlives each call; only this thread's own queue is read.
    while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
        // SAFETY: `message` was just filled by PeekMessageW.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    // SAFETY: `owner` was created on this thread and is destroyed once.
    if let Err(error) = unsafe { DestroyWindow(owner) } {
        log::line(&format!("menu owner window destroy failed: {error}"));
    }
}
