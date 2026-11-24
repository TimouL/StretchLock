//! Thin wrapper: the combined default (tray) entry lives in lib.rs.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() {
    let code = stretchlock_tray::run();
    std::process::exit(code);
}
