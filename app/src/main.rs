#![cfg_attr(windows, windows_subsystem = "windows")]

// The Add device window runs on Windows; its steps are also tested elsewhere.
#[cfg(any(windows, test))]
mod add_device;
mod alerts;
mod app;
mod config;
// Windows only: the context menu runs on its own thread, so a stuck menu never stops the app.
#[cfg(windows)]
mod context_menu;
mod device;
mod dialog;
mod efficiency;
mod instance;
mod interval;
mod licenses;
mod listen;
mod log;
mod login;
mod menu;
mod pause;
// Plug-in detection and its popup run on Windows; their rules are also tested elsewhere.
#[cfg(any(windows, test))]
mod plug;
#[cfg(any(windows, test))]
mod popup;
mod probe;
mod query;
mod query_log;
mod receivers;
mod refresh;
// GitHub reports come from Windows windows; the URL and redaction rules are also tested elsewhere.
#[cfg(any(windows, test))]
mod report;
mod scan;
mod svg_cache;
// System symbols are pre-rendered through AppKit, so the cache is macOS only.
#[cfg(target_os = "macos")]
mod symbol_cache;
mod symbols;
mod taskbar;
mod theme;
mod ui_font;
mod update;
// Vendor apps that pause their brand's devices while open.
mod vendor_apps;
// Windows-only hang dump; the stall bookkeeping is also tested on other hosts.
#[cfg(any(windows, test))]
mod watchdog;

use std::process::ExitCode;
use std::time::SystemTime;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("scan") => run_scan(),
        Some("probe") => {
            attach_parent_console();
            probe::run_probe(std::env::args().skip(2).collect())
        }
        Some("listen") => {
            attach_parent_console();
            listen::run_listen(std::env::args().skip(2).collect())
        }
        Some("licenses") => {
            attach_parent_console();
            println!("{}", licenses::licenses_text());
            ExitCode::SUCCESS
        }
        _ => app::run(),
    }
}

/// A GUI-subsystem exe has no console; borrow the parent shell's so output is visible.
fn attach_parent_console() {
    #[cfg(windows)]
    {
        use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        // Fails when launched without a parent console; then there is nowhere to print.
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
}

/// Prints every section it can; fails only when Bluetooth or the USB listing itself failed.
fn run_scan() -> ExitCode {
    attach_parent_console();

    let bluetooth_ok = match bluetooth::read_devices() {
        Ok(mut devices) => {
            ui_model::sort_by_level(&mut devices);
            println!("{}", scan::scan_report(&devices, SystemTime::now()));
            true
        }
        Err(error) => {
            eprintln!("scan failed: {error:#}");
            false
        }
    };

    let inventory = door::inventory(&[]);
    println!(
        "\n{}",
        scan::usb_report(
            &inventory.hid,
            &inventory.serial,
            &inventory.errors,
            &config::Config::load().custom()
        )
    );
    // Layout data for diagnosing the taskbar strip on a machine nobody can inspect.
    #[cfg(windows)]
    println!("\n{}", taskbar::describe_environment());

    if bluetooth_ok && inventory.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
