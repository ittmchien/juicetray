#![cfg_attr(windows, windows_subsystem = "windows")]

mod alerts;
mod app;
mod config;
mod dialog;
mod instance;
mod interval;
mod login;
mod menu;
mod probe;
mod query_log;
mod refresh;
mod scan;
mod svg_cache;
mod taskbar;
mod theme;
mod ui_font;

use std::process::ExitCode;
use std::time::SystemTime;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("scan") => run_scan(),
        Some("probe") => {
            attach_parent_console();
            probe::run_probe(std::env::args().nth(2))
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
            ui_model::sort_weakest_first(&mut devices);
            println!("{}", scan::scan_report(&devices, SystemTime::now()));
            true
        }
        Err(error) => {
            eprintln!("scan failed: {error:#}");
            false
        }
    };

    let inventory = door::inventory(None);
    println!(
        "\n{}",
        scan::usb_report(&inventory.hid, &inventory.serial, &inventory.errors)
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
