//! HKCU Run-key autostart management (Software\Microsoft\Windows\CurrentVersion\Run).

#![cfg(windows)]

use windows::core::w;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteKeyValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_READ, KEY_WRITE, REG_SZ,
};

const RUN_KEY: windows::core::PCWSTR =
    w!(r"Software\Microsoft\Windows\CurrentVersion\Run");

/// Create (`enable = true`) or remove (`enable = false`) a REG_SZ value in
/// the per-user Run key. `exe_path` defaults to the current executable; the
/// value name distinguishes entries, e.g. "StretchLockDaemon" or
/// "StretchLockTray".
pub fn set_run_key(name: &str, enable: bool, exe_path: Option<&str>) -> std::io::Result<()> {
    unsafe {
        let access = if enable { KEY_WRITE } else { KEY_READ };
        let mut hkey = HKEY::default();
        let status = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, access, &mut hkey);
        if status != ERROR_SUCCESS {
            return Err(win32_io_error("RegOpenKeyExW", status));
        }
        let result = if enable {
            let exe = match exe_path {
                Some(path) => path.to_owned(),
                None => match std::env::current_exe() {
                    Ok(path) => path.display().to_string(),
                    Err(err) => return Err(std::io::Error::other(err)),
                },
            };
            // REG_SZ payload includes the trailing NUL.
            let mut wide: Vec<u16> = exe.encode_utf16().collect();
            wide.push(0);
            let bytes: &[u8] = std::slice::from_raw_parts(
                wide.as_ptr().cast(),
                wide.len() * std::mem::size_of::<u16>(),
            );
            let name_w = to_wide(name);
            let status = RegSetValueExW(
                hkey,
                windows::core::PCWSTR(name_w.as_ptr()),
                0,
                REG_SZ,
                Some(bytes),
            );
            if status == ERROR_SUCCESS {
                log::info!("Autostart enabled: {} -> {}", name, exe);
                Ok(())
            } else {
                Err(win32_io_error("RegSetValueExW", status))
            }
        } else {
            let name_w = to_wide(name);
            let status = RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                windows::core::PCWSTR(name_w.as_ptr()),
            );
            if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
                log::info!("Autostart disabled: {}", name);
                Ok(())
            } else {
                Err(win32_io_error("RegDeleteKeyValueW", status))
            }
        };
        let _ = RegCloseKey(hkey);
        result
    }
}

fn win32_io_error(what: &str, status: WIN32_ERROR) -> std::io::Error {
    log::warn!("{} failed: {}", what, status.0);
    std::io::Error::from_raw_os_error(status.0 as i32)
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
