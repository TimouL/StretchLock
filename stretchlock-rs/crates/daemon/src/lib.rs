//! Daemon library entry, shared by the combined `StretchLock` binary.

use stretchlock_core::config::ConfigStore;
use stretchlock_core::stats::Stats;

pub mod app;
pub mod timer;

#[cfg(windows)]
pub mod autostart;
#[cfg(windows)]
pub mod hooks;
#[cfg(windows)]
pub mod ipc_server;
#[cfg(windows)]
pub mod mascot;
pub mod overlay;

/// Run the daemon until IPC StopService exits the process.
pub fn run(mut args: impl Iterator<Item = String>) -> i32 {
    let mut log_level: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--log-level" => {
                log_level = args.next();
            }
            other => {
                eprintln!("Unknown argument: {}", other);
                eprintln!("Usage: StretchLock --daemon [--log-level DEBUG|INFO|WARNING|ERROR|OFF]");
                return 2;
            }
        }
    }

    let store = ConfigStore::default();
    let mut config = store.load();
    if let Some(level) = log_level {
        config.log_level = level;
        config.validate();
    }
    stretchlock_core::logging::init(&config.log_level, Some(&store.dir));
    log::info!(
        "StretchLock daemon starting (work {} min, break {} min, enabled {})",
        config.work_interval_minutes,
        config.break_duration_minutes,
        config.enabled
    );

    #[cfg(windows)]
    {
        overlay::enable_per_monitor_dpi();
        if !acquire_single_instance_lock() {
            log::warn!("Another StretchLock daemon is already running; exiting");
            return 0;
        }
        maintain_autostart(&config);
    }

    let stats = Stats::load(&store.dir);
    let application = std::sync::Arc::new(app::App::new(store, config, stats));

    #[cfg(windows)]
    {
        ipc_server::spawn(application.clone());
        overlay::start();
    }

    // Event-driven loop: the overlay's resume button and the keyboard's
    // passphrase submission wake us immediately; the 1-second timeout drives
    // the timer tick.
    let (wake_tx, wake_rx) = std::sync::mpsc::channel::<()>();
    #[cfg(windows)]
    {
        overlay::set_resume_sender(wake_tx.clone());
        hooks::set_wake_sender(wake_tx);
    }
    loop {
        match wake_rx.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(()) => {
                application.check_passphrase_override();
                application.on_resume();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => application.tick_once(),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    0
}

/// Keep the per-user Run key in sync with `autostart_daemon`.
#[cfg(windows)]
fn maintain_autostart(config: &stretchlock_core::config::Config) {
    const VALUE_NAME: &str = "StretchLockDaemon";
    // Combined binary: the Run entry must select the daemon role explicitly.
    // set_run_key writes the string verbatim, so quote the path here for
    // paths containing spaces.
    let exe_path = std::env::current_exe().ok().and_then(|path| {
        let path = path.display().to_string().replace('/', "\\");
        path.contains('"')
            .then(|| {
                log::warn!("executable path contains a quote; not writing autostart value");
                None::<String>
            })
            .unwrap_or_else(|| Some(format!("\"{path}\" --daemon")))
    });
    if let Err(err) =
        autostart::set_run_key(VALUE_NAME, config.autostart_daemon, exe_path.as_deref())
    {
        log::warn!("Could not update autostart entry: {}", err);
    }
}

#[cfg(windows)]
fn acquire_single_instance_lock() -> bool {
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::w;

    unsafe {
        let handle = match CreateMutexW(None, false, w!("StretchLockDaemonMutex")) {
            Ok(handle) => handle,
            Err(err) => {
                log::warn!("CreateMutexW failed: {}", err);
                // Fail open: blocking startup over a mutex is worse.
                return true;
            }
        };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            return false;
        }
        // Keep the handle open for the process lifetime so no second
        // instance can win the race while this one runs.
        true
    }
}
