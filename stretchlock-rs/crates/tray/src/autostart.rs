//! Tray autostart entry: `HKEY_CURRENT_USER\Software\Microsoft\Windows\
//! CurrentVersion\Run`, value name `StretchLockTray` pointing at this exe.
//!
//! The daemon's own autostart key is maintained by the daemon; only the
//! tray key lives here.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "StretchLockTray";

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

fn quoted_exe_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.to_string_lossy().replace('/', "\\");
    if path.contains('"') {
        log::warn!("executable path contains a quote; not writing autostart value");
        return None;
    }
    Some(format!("\"{path}\""))
}

fn open_run_key(access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Result<HKEY, WIN32_ERROR> {
    let mut key = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(wide(RUN_KEY).as_ptr()),
            0,
            access,
            &mut key,
        )
    };
    if status.is_ok() { Ok(key) } else { Err(status) }
}

/// Current autostart checkbox state: the Run value exists.
pub fn enabled() -> bool {
    let key = match open_run_key(KEY_QUERY_VALUE) {
        Ok(key) => key,
        Err(_) => return false,
    };
    let name = wide(VALUE_NAME);
    let mut kind = REG_SZ;
    let mut size = 0u32;
    let status = unsafe {
        RegGetValueW(
            key,
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            Some(&mut kind),
            None,
            Some(&mut size),
        )
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    status == ERROR_SUCCESS
}

pub fn set_enabled(enable: bool) -> Result<(), String> {
    if !enable {
        let key = match open_run_key(KEY_SET_VALUE) {
            Ok(key) => key,
            Err(status) if status == ERROR_FILE_NOT_FOUND => return Ok(()),
            Err(status) => return Err(format!("open registry key failed: {status:?}")),
        };
        let status = unsafe { RegDeleteValueW(key, PCWSTR(wide(VALUE_NAME).as_ptr())) };
        unsafe {
            let _ = RegCloseKey(key);
        }
        if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
            log::info!("tray autostart removed");
            return Ok(());
        }
        return Err(format!("RegDeleteValueW failed: {status:?}"));
    }

    let Some(command) = quoted_exe_command() else {
        return Err("cannot determine tray executable path".into());
    };
    let key = match open_run_key(KEY_SET_VALUE) {
        Ok(key) => key,
        Err(status) if status == ERROR_FILE_NOT_FOUND => {
            // The Run key normally always exists; create it if not.
            let mut created = HKEY::default();
            let create = unsafe {
                RegCreateKeyW(
                    HKEY_CURRENT_USER,
                    PCWSTR(wide(RUN_KEY).as_ptr()),
                    &mut created,
                )
            };
            if create.is_ok() {
                created
            } else {
                return Err(format!("create registry key failed: {create:?}"));
            }
        }
        Err(status) => return Err(format!("open registry key failed: {status:?}")),
    };
    // REG_SZ data includes the terminating NUL.
    let mut data = wide(&command);
    data.pop();
    data.push(0);
    let status = unsafe {
        RegSetValueExW(
            key,
            PCWSTR(wide(VALUE_NAME).as_ptr()),
            0,
            REG_SZ,
            Some(std::slice::from_raw_parts(
                data.as_ptr().cast::<u8>(),
                data.len() * 2,
            )),
        )
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    if status == ERROR_SUCCESS {
        log::info!("tray autostart set to {command}");
        Ok(())
    } else {
        Err(format!("RegSetValueExW failed: {status:?}"))
    }
}
