//! Thin wrapper: the combined `StretchLock --daemon` entry lives in lib.rs.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() {
    let code = stretchlock_daemon::run(std::env::args().skip(1));
    std::process::exit(code);
}
