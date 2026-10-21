#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod win;

#[cfg(target_os = "macos")]
mod mac;

#[cfg(windows)]
fn main() {
    win::run();
}

#[cfg(target_os = "macos")]
fn main() {
    mac::run();
}

#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!("Dost runs on Windows and macOS. To preview the art: cargo run --example preview");
}
