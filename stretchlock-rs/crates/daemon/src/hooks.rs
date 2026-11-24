//! Low-level mouse hook that swallows all mouse input during a break.
//!
//! Ported from `platform/windows.py`. The hook thread must pump messages
//! (GetMessageW) for a low-level hook to stay responsive; dropping the
//! guard unhooks and posts WM_QUIT to that thread.

#![cfg(windows)]

use std::thread;
use std::time::Duration;

use parking_lot::Mutex;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetKeyState};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_QUIT,
    WM_SYSKEYDOWN,
};

/// RAII holder for the blocking mouse hook. `start_mouse_block` installs
/// the hook on a dedicated thread; dropping the guard unhooks it and stops
/// the message pump.
pub struct HookGuard {
    hook: HHOOK,
    pump_thread_id: u32,
}

// HHOOK is an opaque handle; moving it across threads is safe and
// UnhookWindowsHookEx is not thread-affine.
unsafe impl Send for HookGuard {}

unsafe extern "system" fn mouse_hook_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code < 0 {
        // Must forward messages the system does not want us to handle.
        CallNextHookEx(None, n_code, w_param, l_param)
    } else {
        // Swallow every mouse event while the break is active.
        LRESULT(1)
    }
}

/// Start blocking mouse input. Returns `None` (with a logged warning) when
/// the hook cannot be installed; the overlay alone then handles blocking,
/// mirroring the Python adapter's behaviour.
pub fn start_mouse_block() -> Option<HookGuard> {
    // `windows::core::Error` is not Send, so report the raw handle plus an
    // optional formatted error string across the channel instead.
    let (sender, receiver) = std::sync::mpsc::channel::<(isize, u32, Option<String>)>();
    let spawned = thread::Builder::new()
        .name("stretchlock-mouse-hook".into())
        .spawn(move || {
            let thread_id = unsafe { GetCurrentThreadId() };
            let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), None, 0) };
            let report = match &hook {
                Ok(handle) => (handle.0 as isize, thread_id, None),
                Err(err) => (0, thread_id, Some(err.to_string())),
            };
            let _ = sender.send(report);
            if hook.is_err() {
                return;
            }
            let mut msg = MSG::default();
            unsafe {
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
            }
        });
    match spawned {
        Ok(_) => {}
        Err(err) => {
            log::warn!("Failed to spawn mouse hook thread: {}", err);
            return None;
        }
    }
    let (hook_value, pump_thread_id, failure) = match receiver.recv_timeout(Duration::from_secs(2))
    {
        Ok(report) => report,
        Err(err) => {
            log::warn!("Mouse hook thread did not report installation: {}", err);
            return None;
        }
    };
    if let Some(err) = failure {
        log::warn!(
            "SetWindowsHookExW returned NULL -- mouse hook installation failed: {}",
            err
        );
        return None;
    }
    log::info!("Mouse input blocked via low-level hook");
    Some(HookGuard {
        hook: HHOOK(hook_value as _),
        pump_thread_id,
    })
}

impl Drop for HookGuard {
    fn drop(&mut self) {
        unsafe {
            if UnhookWindowsHookEx(self.hook).is_ok() {
                log::info!("Mouse input unblocked");
            } else {
                log::warn!("Failed to remove mouse hook");
            }
            let _ = PostThreadMessageW(self.pump_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

// ---------------------------------------------------------------------------
// Keyboard hook: during a break, swallow all keys except those building the
// passphrase buffer. Enter submits it for verification; the app polls the
// shared state every second.
// ---------------------------------------------------------------------------

static PASSPHRASE_BUFFER: Mutex<PassphraseState> = Mutex::new(PassphraseState::new());

/// Wakes the daemon loop immediately after Enter submits a passphrase, so
/// verification never waits for the next one-second tick.
static WAKE_TX: Mutex<Option<std::sync::mpsc::Sender<()>>> = Mutex::new(None);

pub fn set_wake_sender(tx: std::sync::mpsc::Sender<()>) {
    *WAKE_TX.lock() = Some(tx);
}

fn notify_passphrase_submitted() {
    if let Some(tx) = WAKE_TX.lock().as_ref() {
        let _ = tx.send(());
    }
}

struct PassphraseState {
    chars: Vec<u8>,
    submitted: bool,
}

impl PassphraseState {
    const MAX_LEN: usize = 128;
    const fn new() -> Self {
        PassphraseState {
            chars: Vec::new(),
            submitted: false,
        }
    }
}

/// Take the passphrase typed since the last call, if Enter was pressed.
pub fn take_submitted_passphrase() -> Option<String> {
    let mut state = PASSPHRASE_BUFFER.lock();
    if state.submitted {
        state.submitted = false;
        let typed = String::from_utf8_lossy(&state.chars).into_owned();
        state.chars.clear();
        Some(typed)
    } else {
        None
    }
}

/// Number of characters typed so far, for the overlay's masked field.
pub fn passphrase_len() -> usize {
    PASSPHRASE_BUFFER.lock().chars.len()
}

/// Clear the buffer when a break ends so the next one starts fresh.
pub fn reset_passphrase_buffer() {
    let mut state = PASSPHRASE_BUFFER.lock();
    state.chars.clear();
    state.submitted = false;
}

unsafe extern "system" fn keyboard_hook_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code < 0 {
        return CallNextHookEx(None, n_code, w_param, l_param);
    }
    let msg_code = w_param.0 as u32;
    if msg_code != WM_KEYDOWN && msg_code != WM_SYSKEYDOWN {
        // Swallow every key-up as well; they carry no data we need.
        return LRESULT(1);
    }
    let info = &*(l_param.0 as *const KBDLLHOOKSTRUCT);
    let vk = info.vkCode as u32;

    // Ctrl/Alt/Win (both flavours) held: always swallow, never buffer.
    let ctrl = GetAsyncKeyState(0x11i32 /*VK_CONTROL*/) as u16 & 0x8000 != 0;
    let alt = GetAsyncKeyState(0x12i32 /*VK_MENU*/) as u16 & 0x8000 != 0;
    let win = GetAsyncKeyState(0x5Bi32 /*VK_LWIN*/) as u16 & 0x8000 != 0
        || GetAsyncKeyState(0x5Ci32 /*VK_RWIN*/) as u16 & 0x8000 != 0;
    if ctrl || alt || win {
        return LRESULT(1);
    }
    // CapsLock toggle state: toggled = uppercase letters.
    let caps = GetKeyState(0x14i32 /*VK_CAPITAL*/) as u16 & 1 != 0;

    match vk {
        // Enter submits what has been typed so far.
        0x0D /*VK_RETURN*/ => {
            let mut state = PASSPHRASE_BUFFER.lock();
            state.submitted = !state.chars.is_empty();
            if !state.submitted {
                state.chars.clear();
            }
            drop(state);
            crate::overlay::set_passphrase_len(0);
            notify_passphrase_submitted();
            LRESULT(1)
        }
        0x08 /*VK_BACK*/ => {
            let len = {
                let mut state = PASSPHRASE_BUFFER.lock();
                state.chars.pop();
                state.chars.len()
            };
            crate::overlay::set_passphrase_len(len);
            LRESULT(1)
        }
        0x1B /*VK_ESCAPE*/ | 0x09 /*VK_TAB*/ | 0x20 /*VK_SPACE*/ => LRESULT(1),
        0x30..=0x5A /*0-9 A-Z*/ => {
            let mut state = PASSPHRASE_BUFFER.lock();
            if state.chars.len() < PassphraseState::MAX_LEN && !state.submitted {
                let shift = GetAsyncKeyState(0x10i32 /*VK_SHIFT*/) as u16 & 0x8000 != 0;
                let ch = match vk {
                    // Digits: shifted variants per US layout.
                    0x30 => if shift { b')' } else { b'0' },
                    0x31 => if shift { b'!' } else { b'1' },
                    0x32 => if shift { b'@' } else { b'2' },
                    0x33 => if shift { b'#' } else { b'3' },
                    0x34 => if shift { b'$' } else { b'4' },
                    0x35 => if shift { b'%' } else { b'5' },
                    0x36 => if shift { b'^' } else { b'6' },
                    0x37 => if shift { b'&' } else { b'7' },
                    0x38 => if shift { b'*' } else { b'8' },
                    0x39 => if shift { b'(' } else { b'9' },
                    // OEM keys on the main block.
                    0xBA => if shift { b':' } else { b';' },
                    0xBB => if shift { b'+' } else { b'=' },
                    0xBC => if shift { b'<' } else { b',' },
                    0xBD => if shift { b'_' } else { b'-' },
                    0xBE => if shift { b'>' } else { b'.' },
                    0xBF => if shift { b'?' } else { b'/' },
                    0xC0 => if shift { b'~' } else { b'`' },
                    0xDB => if shift { b'{' } else { b'[' },
                    0xDC => if shift { b'|' } else { b'\\' },
                    0xDD => if shift { b'}' } else { b']' },
                    0xDE => if shift { b'"' } else { b'\'' },
                    // Letters: uppercase when shift XOR caps.
                    _ if (0x41..=0x5A).contains(&vk) => {
                        if shift ^ caps { vk as u8 } else { vk as u8 + 32 }
                    }
                    _ => b'?',
                };
                if ch != b'?' || (0x30..=0x5A).contains(&vk) || (0xBA..=0xDE).contains(&vk) {
                    state.chars.push(ch);
                }
                let len = state.chars.len();
                drop(state);
                // Instant feedback: the overlay repaints the masked field now
                // instead of waiting for the next one-second tick.
                crate::overlay::set_passphrase_len(len);
            }
            LRESULT(1)
        }
        _ => LRESULT(1),
    }
}

/// RAII holder for the keyboard hook, mirroring `HookGuard`.
pub struct KeyboardGuard {
    hook: HHOOK,
    pump_thread_id: u32,
}

unsafe impl Send for KeyboardGuard {}

/// Start capturing the passphrase buffer and swallowing all other keys.
pub fn start_keyboard_capture() -> Option<KeyboardGuard> {
    let (sender, receiver) = std::sync::mpsc::channel::<(isize, u32, Option<String>)>();
    let spawned = thread::Builder::new()
        .name("stretchlock-keyboard-hook".into())
        .spawn(move || {
            let thread_id = unsafe { GetCurrentThreadId() };
            reset_passphrase_buffer();
            let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), None, 0) };
            let report = match &hook {
                Ok(handle) => (handle.0 as isize, thread_id, None),
                Err(err) => (0, thread_id, Some(err.to_string())),
            };
            let _ = sender.send(report);
            if hook.is_err() {
                return;
            }
            let mut msg = MSG::default();
            unsafe {
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
            }
        });
    match spawned {
        Ok(_) => {}
        Err(err) => {
            log::warn!("Failed to spawn keyboard hook thread: {}", err);
            return None;
        }
    }
    let (hook_value, pump_thread_id, failure) = match receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(report) => report,
        Err(err) => {
            log::warn!("Keyboard hook thread did not report installation: {}", err);
            return None;
        }
    };
    if let Some(err) = failure {
        log::warn!(
            "SetWindowsHookExW keyboard failed: {}",
            err
        );
        return None;
    }
    log::info!("Keyboard capture active (passphrase buffer armed)");
    Some(KeyboardGuard {
        hook: HHOOK(hook_value as _),
        pump_thread_id,
    })
}

impl Drop for KeyboardGuard {
    fn drop(&mut self) {
        unsafe {
            if UnhookWindowsHookEx(self.hook).is_ok() {
                log::info!("Keyboard capture removed");
            } else {
                log::warn!("Failed to remove keyboard hook");
            }
            let _ = PostThreadMessageW(self.pump_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        reset_passphrase_buffer();
    }
}
