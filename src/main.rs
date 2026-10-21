#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(any(windows, target_os = "macos"))]
mod hotkeys;
#[cfg(windows)]
mod win;

#[cfg(target_os = "macos")]
mod mac;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = dost::cli::run(&args) {
        return code;
    }
    start();
    ExitCode::SUCCESS
}

#[cfg(windows)]
fn start() {
    win::run();
}

#[cfg(target_os = "macos")]
fn start() {
    mac::run();
}

#[cfg(not(any(windows, target_os = "macos")))]
fn start() {
    eprintln!("Dost runs on Windows and macOS. To preview the art: cargo run --example preview");
}
