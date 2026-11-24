//! Combined entry point. One executable, two roles:
//!   StretchLock --daemon  -> service process (timer, lock overlay, IPC server)
//!   StretchLock           -> tray UI process; starts the daemon if absent
//! Two processes keep tray crashes isolated from the service while shipping
//! a single file.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--daemon") => {
            let code = stretchlock_daemon::run(args);
            std::process::exit(code);
        }
        Some("--help") | Some("-h") => {
            println!("StretchLock {}", env!("CARGO_PKG_VERSION"));
            println!("Usage: StretchLock [--daemon] [--log-level DEBUG|INFO|WARNING|ERROR|OFF]");
            println!("       StretchLock            start the tray UI (spawns --daemon if needed)");
            println!("       StretchLock --daemon   start the service process");
            std::process::exit(0);
        }
        Some(other) => {
            eprintln!("Unknown argument: {other}");
            eprintln!("Run StretchLock --help for usage.");
            std::process::exit(2);
        }
        None => {
            ensure_daemon();
            let code = stretchlock_tray::run();
            std::process::exit(code);
        }
    }
}

/// Spawn `current_exe() --daemon` detached when the service is not running.
#[cfg(windows)]
fn ensure_daemon() {
    if daemon_reachable() {
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            log::warn!("cannot locate current exe: {err}");
            return;
        }
    };
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    match std::process::Command::new(&exe)
        .arg("--daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
    {
        Ok(child) => log::info!("spawned daemon pid {}", child.id()),
        Err(err) => {
            log::warn!("failed to spawn daemon: {err}");
            return;
        }
    }
    // Give the named pipe a moment to come up.
    for _ in 0..20 {
        if daemon_reachable() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[cfg(windows)]
fn daemon_reachable() -> bool {
    use std::fs::OpenOptions;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(r"\\.\pipe\stretchlock")
        .is_ok()
}

#[cfg(not(windows))]
fn daemon_reachable() -> bool {
    false
}

#[cfg(not(windows))]
fn ensure_daemon() {}
