//! Tray library entry, shared by the combined `StretchLock` binary.

#[cfg(windows)]
pub mod app;
#[cfg(windows)]
pub mod autostart;
pub mod icon;
pub mod ipc;
// Pure serde/validation logic; compiled on every target so its tests run
// outside Windows too. The UI layers that consume it are windows-only.
pub mod settings;
#[cfg(windows)]
pub mod tray;
#[cfg(windows)]
pub mod ui;

/// Run the tray UI until the user quits (never stops the daemon).
#[cfg(windows)]
pub fn run() -> i32 {
    use stretchlock_core::logging;
    let store = stretchlock_core::ConfigStore::default();
    let config = store.load();
    logging::init(&config.log_level, Some(&store.dir));
    log::info!("tray starting");
    app::run(config);
    0
}

#[cfg(not(windows))]
pub fn run() -> i32 {
    eprintln!("StretchLock tray only runs on Windows");
    1
}
