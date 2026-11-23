#![cfg_attr(windows, windows_subsystem = "windows")]

mod alerts;
mod app;
mod config;
mod instance;
mod login;
mod menu;
mod refresh;
mod scan;
mod taskbar;
mod theme;

use std::process::ExitCode;
use std::time::SystemTime;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("scan") => run_scan(),
        _ => app::run(),
    }
}

fn run_scan() -> ExitCode {
    // A GUI-subsystem exe has no console; borrow the parent shell's so output is visible.
    #[cfg(windows)]
    {
        use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        // Fails when launched without a parent console; then there is nowhere to print.
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }

    match bluetooth::read_devices() {
        Ok(mut devices) => {
            ui_model::sort_weakest_first(&mut devices);
            println!("{}", scan::scan_report(&devices, SystemTime::now()));
            // Layout data for diagnosing the taskbar strip on a machine nobody can inspect.
            #[cfg(windows)]
            println!("\n{}", taskbar::describe_environment());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("scan failed: {error:#}");
            ExitCode::FAILURE
        }
    }
}
