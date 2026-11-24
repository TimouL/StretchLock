//! Full-screen break overlay, modelled on the Python original: the desktop is
//! captured once when the break starts, blurred, and painted edge to edge on
//! every monitor; a frosted card in the centre shows the countdown, the break
//! stats and the masked passphrase input.
//!
//! Painting is plain GDI on an opaque window, so what the user sees is exactly
//! the blurred capture we drew — no layered-window blending involved.

#![cfg(windows)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use parking_lot::Mutex;

use chrono::{Datelike, Local, Timelike};

use windows::core::{w, Interface};
use windows::Win32::Foundation::{BOOL, COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
    EnumDisplayMonitors, GetDC, GetDeviceCaps, GetMonitorInfoW, InvalidateRect, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, DT_CENTER, DT_SINGLELINE, DT_VCENTER, HBITMAP, HDC,
    LOGPIXELSY, MONITORINFO, SRCCOPY, TRANSPARENT,
};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    GetSystemMetrics, GetWindowRect, LoadCursorW, PostMessageW, PostThreadMessageW, RegisterClassW,
    SetTimer, SetWindowPos, ShowWindow, SystemParametersInfoW, TranslateMessage, UpdateLayeredWindow,
    CS_HREDRAW, CS_VREDRAW, HCURSOR, HWND_TOPMOST, IDC_ARROW, MSG, SPI_GETWORKAREA,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE,
    SW_SHOWNOACTIVATE, ULW_ALPHA, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP, WM_APP, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_ERASEBKGND, WM_PAINT, WM_TIMER, WM_LBUTTONDOWN,
};

const OVERLAY_CLASS: windows::core::PCWSTR = w!("StretchLockOverlay");
const WM_APP_ACTIVATE: u32 = WM_APP + 1;
const WM_APP_DEACTIVATE: u32 = WM_APP + 2;
const WM_APP_UPDATE: u32 = WM_APP + 3;
const WM_APP_WIDGET_UPDATE: u32 = WM_APP + 4;
const WM_APP_WIDGET_HIDE: u32 = WM_APP + 5;

const WIDGET_CLASS: windows::core::PCWSTR = w!("StretchLockCountdown");

/// What the overlay shows. Split into slots so each part gets its own font.
#[derive(Default, Clone)]
pub struct OverlayContent {
    pub title: String,
    pub countdown: String,
    pub stats: String,
    pub hint: String,
    pub message: String,
    pub passphrase_len: usize,
    pub animate: bool,
    pub fraction: f32,
    /// Break finished, awaiting the resume button.
    pub done: bool,
}

static CONTENT: Mutex<OverlayContent> = Mutex::new(OverlayContent {
    title: String::new(),
    countdown: String::new(),
    stats: String::new(),
    hint: String::new(),
    message: String::new(),
    passphrase_len: 0,
    animate: true,
    fraction: 1.0,
    done: false,
});

/// Blurred, darkened desktop capture shared by every overlay window.
#[derive(Clone)]
struct Capture {
    pixels: Vec<u32>,
    width: i32,
    height: i32,
    origin_x: i32,
    origin_y: i32,
}

static CAPTURE: Mutex<Option<Capture>> = Mutex::new(None);

/// Off-screen back buffer: every frame is composed here and blitted to the
/// window in one go, which removes the flicker of direct painting.
// GDI handles are only touched on the overlay UI thread.
/// Returns a DC backed by a DIB of at least `width`×`height`, recreating it
/// when the window size changes.
/// Run `f` over the back-buffer pixels (index = y*width + x).
/// Content rectangle for a given client size, shared by painting and the
/// animation invalidation so both agree. The redesign spreads content
/// across most of the screen (a clock/date column on wide layouts, the
/// centred column otherwise), so the refresh region covers nearly the
/// whole client area instead of the old centred card — otherwise the
/// clock column would never repaint.
fn card_rect(client: &RECT) -> RECT {
    let width = (client.right - client.left).max(1);
    let height = (client.bottom - client.top).max(1);
    RECT {
        left: client.left + (width as f32 * 0.03) as i32,
        top: client.top + (height as f32 * 0.02) as i32,
        right: client.left + (width as f32 * 0.97) as i32,
        bottom: client.top + (height as f32 * 0.95) as i32,
    }
}

/// Event channel from the overlay's resume button to the daemon loop.
static RESUME_TX: Mutex<Option<std::sync::mpsc::Sender<()>>> = Mutex::new(None);

pub fn set_resume_sender(tx: std::sync::mpsc::Sender<()>) {
    *RESUME_TX.lock() = Some(tx);
}

fn notify_resume() {
    if let Some(tx) = RESUME_TX.lock().as_ref() {
        let _ = tx.send(());
    }
}

/// The "start using the computer" button, shown once the break finishes.
fn resume_button_rect(client: &RECT, scale: f32) -> RECT {
    let width = (client.right - client.left).max(1);
    let height = (client.bottom - client.top).max(1);
    let short = (width.min(height)) as f32;
    let bw = ((short * 0.28) as i32).max((280.0 * scale) as i32);
    let bh = (52.0 * scale) as i32;
    let cx = client.left + width / 2;
    let cy = client.top + (height as f32 * 0.74) as i32;
    RECT {
        left: cx - bw / 2,
        top: cy - bh / 2,
        right: cx + bw / 2,
        bottom: cy + bh / 2,
    }
}

static WINDOWS: Mutex<Vec<isize>> = Mutex::new(Vec::new());
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Displayed ring fraction; eases toward the target every frame so the ring
/// shrinks smoothly instead of jumping once per second.

struct WindowTarget {
    target: ID2D1HwndRenderTarget,
    capture_bitmap: Option<ID2D1Bitmap>,
    capture_version: usize,
}

static D2D_FACTORY: Mutex<Option<ID2D1Factory>> = Mutex::new(None);
static DWRITE_FACTORY: Mutex<Option<IDWriteFactory>> = Mutex::new(None);
static WINDOW_TARGETS: Mutex<Option<HashMap<isize, WindowTarget>>> = Mutex::new(None);
static WIDGET_HWND: Mutex<Option<isize>> = Mutex::new(None);
static WIDGET_TEXT: Mutex<String> = Mutex::new(String::new());
static CAPTURE_VERSION: AtomicUsize = AtomicUsize::new(0);

static UI_THREAD_ID: Mutex<Option<u32>> = Mutex::new(None);
const ANIM_TIMER_ID: usize = 0xA11;
const ANIM_INTERVAL_MS: u32 = 40;
static ANIM_EPOCH: parking_lot::Mutex<Option<std::time::Instant>> = parking_lot::Mutex::new(None);

/// Milliseconds since the overlay was activated, used to drive the mascot.
fn animation_ms() -> u64 {
    ANIM_EPOCH
        .lock()
        .map(|start| start.elapsed().as_millis() as u64)
        .unwrap_or(0)
}

/// Spawn the overlay UI thread. Idempotent.
pub fn start() {
    let mut thread_id = UI_THREAD_ID.lock();
    if thread_id.is_some() {
        return;
    }
    match std::thread::Builder::new()
        .name("stretchlock-overlay-ui".into())
        .spawn(ui_thread_main)
    {
        Ok(_) => *thread_id = Some(0),
        Err(err) => log::warn!("Failed to spawn overlay UI thread: {}", err),
    }
}

/// Opt the process into per-monitor DPI awareness. Call once at startup.
pub fn enable_per_monitor_dpi() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn activate() {
    // Capture before the overlay is shown so we never photograph ourselves.
    unsafe { capture_desktop() };
    CAPTURE_VERSION.fetch_add(1, Ordering::SeqCst);
    *ANIM_EPOCH.lock() = Some(std::time::Instant::now());
    ACTIVE.store(true, Ordering::SeqCst);
    broadcast(WM_APP_ACTIVATE);
}

pub fn deactivate() {
    ACTIVE.store(false, Ordering::SeqCst);
    broadcast(WM_APP_DEACTIVATE);
    *CAPTURE.lock() = None;
}

/// Update only the masked passphrase length. Called from the keyboard hook so
/// typed characters show up immediately instead of waiting for the 1 Hz tick.
pub fn set_passphrase_len(len: usize) {
    {
        let mut content = CONTENT.lock();
        if content.passphrase_len == len {
            return;
        }
        content.passphrase_len = len;
    }
    broadcast(WM_APP_UPDATE);
}

/// Update the text slots shown on every overlay.
#[allow(clippy::too_many_arguments)]
pub fn update(
    title: &str,
    countdown: &str,
    stats: &str,
    hint: &str,
    message: &str,
    passphrase_len: usize,
    animate: bool,
    fraction: f32,
    done: bool,
) {
    {
        let mut content = CONTENT.lock();
        content.title = title.to_owned();
        content.countdown = countdown.to_owned();
        content.stats = stats.to_owned();
        content.hint = hint.to_owned();
        content.message = message.to_owned();
        content.passphrase_len = passphrase_len;
        content.animate = animate;
        content.fraction = fraction;
        content.done = done;
    }
    broadcast(WM_APP_UPDATE);
}

fn broadcast(msg: u32) {
    let windows = WINDOWS.lock().clone();
    for &hwnd in &windows {
        unsafe {
            let _ = PostMessageW(HWND(hwnd as _), msg, WPARAM(0), LPARAM(0));
        }
    }
    if windows.is_empty() {
        if let Some(id) = *UI_THREAD_ID.lock() {
            if id != 0 {
                unsafe {
                    let _ = PostThreadMessageW(id, msg, WPARAM(0), LPARAM(0));
                }
            }
        }
    }
}

fn ui_thread_main() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        *UI_THREAD_ID.lock() = Some(GetCurrentThreadId());
        if !register_overlay_class() {
            log::warn!("Overlay window class registration failed; overlays disabled");
            return;
        }
        recreate_windows();
        register_widget_class();
        create_widget_window();
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn register_overlay_class() -> bool {
    let hmodule = match GetModuleHandleW(None) {
        Ok(module) => module,
        Err(err) => {
            log::warn!("GetModuleHandleW failed: {}", err);
            return false;
        }
    };
    let hcursor: HCURSOR = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(overlay_wndproc),
        hInstance: HINSTANCE(hmodule.0),
        hCursor: hcursor,
        lpszClassName: OVERLAY_CLASS,
        ..Default::default()
    };
    if RegisterClassW(&class) == 0 {
        log::warn!("RegisterClassW(StretchLockOverlay) failed");
        return false;
    }
    true
}

unsafe fn recreate_windows() -> Vec<HWND> {
    let stale: Vec<isize> = WINDOWS.lock().drain(..).collect();
    for value in stale {
        let _ = DestroyWindow(HWND(value as _));
    }

    let _ = EnumDisplayMonitors(None, None, Some(monitor_enum_proc), LPARAM(0));

    let windows: Vec<HWND> = WINDOWS
        .lock()
        .iter()
        .map(|&value| HWND(value as _))
        .collect();
    if ACTIVE.load(Ordering::SeqCst) {
        for &hwnd in &windows {
            show_topmost(hwnd);
        }
    }
    log::debug!("Overlay windows rebuilt ({} monitors)", windows.len());
    windows
}

unsafe extern "system" fn monitor_enum_proc(
    hmonitor: windows::Win32::Graphics::Gdi::HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    _data: LPARAM,
) -> BOOL {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(hmonitor, &mut info).as_bool() {
        if let Some(hwnd) = create_overlay_window(&info.rcMonitor) {
            WINDOWS.lock().push(hwnd.0 as isize);
        }
    }
    BOOL(1)
}

unsafe fn create_overlay_window(rect: &RECT) -> Option<HWND> {
    let hmodule = GetModuleHandleW(None).ok()?;
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        OVERLAY_CLASS,
        w!("StretchLock break overlay"),
        WS_POPUP,
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
        None,
        None,
        HINSTANCE(hmodule.0),
        None,
    )
    .ok()?;
    // ~25 fps redraws for the mascot animation; the handler only invalidates
    // when the overlay is visible and animation is enabled.
    let _timer = SetTimer(hwnd, ANIM_TIMER_ID, ANIM_INTERVAL_MS, None);
    force_topmost(hwnd);
    Some(hwnd)
}

unsafe fn force_topmost(hwnd: HWND) {
    let _ = SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
}

unsafe fn show_topmost(hwnd: HWND) {
    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    force_topmost(hwnd);
    let _ = InvalidateRect(hwnd, None, true);
}

unsafe extern "system" fn overlay_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_APP_ACTIVATE => {
            show_topmost(hwnd);
            LRESULT(0)
        }
        WM_APP_DEACTIVATE => {
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_APP_UPDATE => {
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            recreate_windows();
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_TIMER => {
            if ACTIVE.load(Ordering::SeqCst) {
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                let mut card = card_rect(&client);
                let margin = (40.0 * (client.bottom as f32 / 900.0)) as i32;
                card.left -= margin;
                card.top -= margin;
                card.right += margin;
                card.bottom += margin;
                let _ = InvalidateRect(hwnd, Some(&card), false);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            // The resume button only exists once the break has finished and
            // the mouse hook has been released.
            if CONTENT.lock().done {
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                let screen = GetDC(None);
                let dpi = GetDeviceCaps(screen, LOGPIXELSY).max(96);
                let _ = ReleaseDC(None, screen);
                let scale = dpi as f32 / 96.0;
                let rect = resume_button_rect(&client, scale);
                let lp = lparam.0 as u32;
                let x = (lp & 0xFFFF) as i16 as i32;
                let y = ((lp >> 16) & 0xFFFF) as i16 as i32;
                if x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom {
                    notify_resume();
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Capture the whole virtual desktop, blur it and darken it. Runs before the
/// overlay becomes visible so the capture shows the user's work, not us.
unsafe fn capture_desktop() {
    log::info!("capture_desktop enter");
    let origin_x = GetSystemMetrics(SM_XVIRTUALSCREEN);
    let origin_y = GetSystemMetrics(SM_YVIRTUALSCREEN);
    let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
    let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
    if width <= 0 || height <= 0 {
        log::warn!("Virtual screen metrics unavailable; overlay falls back to solid");
        return;
    }
    let screen = GetDC(None);
    if screen.is_invalid() {
        log::warn!("GetDC(NULL) failed; overlay falls back to solid");
        return;
    }
    let mem = CreateCompatibleDC(screen);
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib: HBITMAP = match CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, None, 0) {
        Ok(dib) => dib,
        Err(err) => {
            log::warn!("CreateDIBSection failed ({}); overlay falls back to solid", err);
            let _ = DeleteDC(mem);
            let _ = ReleaseDC(None, screen);
            return;
        }
    };

    let old = SelectObject(mem, dib);
    let blit = BitBlt(mem, 0, 0, width, height, screen, origin_x, origin_y, SRCCOPY);
    SelectObject(mem, old);

    // Copy while the DIB is still alive, then release GDI objects.
    let mut pixels = if blit.is_ok() && !bits.is_null() {
        let count = (width as usize) * (height as usize);
        let slice = std::slice::from_raw_parts(bits as *const u32, count);
        slice.to_vec()
    } else {
        log::warn!("BitBlt of the desktop failed; overlay falls back to solid");
        Vec::new()
    };
    let _ = DeleteObject(dib);
    let _ = DeleteDC(mem);
    let _ = ReleaseDC(None, screen);

    if pixels.is_empty() {
        return;
    }
    blur(&mut pixels, width as usize, height as usize, 22);
    // Aurora deep-violet backdrop: blend the blurred desktop toward #0b0913,
    // keeping a faint desktop hint at the centre and near-pure violet edges.
    let cx = width as f32 / 2.0;
    let cy = height as f32 * 0.45;
    let max_r = (width.min(height) as f32) * 0.75;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let idx = y * width as usize + x;
            let pixel = pixels[idx];
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt() / max_r;
            let tint = 0.55 + 0.27 * dist.min(1.0);
            let b = ((pixel & 0xFF) as f32 * (1.0 - tint) + 19.0 * tint) as u32;
            let g = (((pixel >> 8) & 0xFF) as f32 * (1.0 - tint) + 9.0 * tint) as u32;
            let r = (((pixel >> 16) & 0xFF) as f32 * (1.0 - tint) + 11.0 * tint) as u32;
            pixels[idx] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
        }
    }
    // Bake the aurora glow blobs once so the D2D paint only draws a bitmap.
    log::info!("capture_desktop blur+tint done, baking glow");
    {
        let w = width as u32;
        let h = height as u32;
        let short = width.min(height) as f32;
        draw_glow_blob(&mut pixels, w, h, -0.10 * short, -0.10 * short, 0.75 * short, 139, 92, 246, 0.30);
        draw_glow_blob(&mut pixels, w, h, width as f32 + 0.10 * short, 0.15 * short, 0.62 * short, 34, 211, 238, 0.20);
        draw_glow_blob(&mut pixels, w, h, 0.15 * short, height as f32 + 0.10 * short, 0.68 * short, 139, 92, 246, 0.14);
    }
    *CAPTURE.lock() = Some(Capture {
        pixels,
        width,
        height,
        origin_x,
        origin_y,
    });
    log::info!("capture_desktop glow baked, done");
    log::info!("Overlay background captured ({}x{})", width, height);
}

/// Separable box blur (two passes) using prefix sums, operating on 32-bit
/// pixels in place.
fn blur(pixels: &mut [u32], width: usize, height: usize, radius: usize) {
    if width == 0 || height == 0 || radius == 0 {
        return;
    }
    let mut scratch = vec![0u32; pixels.len()];
    for _ in 0..2 {
        for y in 0..height {
            let start = y * width;
            box_blur_line(&mut pixels[start..start + width], &mut scratch[..width], radius);
        }
        let mut column = vec![0u32; height];
        let mut out = vec![0u32; height];
        for x in 0..width {
            for y in 0..height {
                column[y] = pixels[y * width + x];
            }
            box_blur_line(&mut column, &mut out, radius);
            for y in 0..height {
                pixels[y * width + x] = out[y];
            }
        }
    }
}

fn box_blur_line(line: &mut [u32], scratch: &mut [u32], radius: usize) {
    let len = line.len();
    if len == 0 {
        return;
    }
    // Prefix sums per channel (u32 is plenty: 255 * 16k max width).
    let mut sum_b = vec![0u32; len + 1];
    let mut sum_g = vec![0u32; len + 1];
    let mut sum_r = vec![0u32; len + 1];
    for (i, value) in line.iter().enumerate() {
        sum_b[i + 1] = sum_b[i] + (value & 0xFF);
        sum_g[i + 1] = sum_g[i] + ((value >> 8) & 0xFF);
        sum_r[i + 1] = sum_r[i] + ((value >> 16) & 0xFF);
    }
    for i in 0..len {
        let start = i.saturating_sub(radius);
        let end = (i + radius + 1).min(len);
        let n = (end - start) as u32;
        let b = (sum_b[end] - sum_b[start]) / n;
        let g = (sum_g[end] - sum_g[start]) / n;
        let r = (sum_r[end] - sum_r[start]) / n;
        scratch[i] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
    }
    line.copy_from_slice(&scratch[..len]);
}


// ---------------------------------------------------------------------------
// Cartoon mascot
// ---------------------------------------------------------------------------

/// A tiny software rasteriser for the mascot. Shapes are drawn at SS× into a
/// premultiplied-free ARGB buffer that starts out as the card colour, then the
/// buffer is down-sampled by StretchBlt (HALFTONE), which gives smooth edges
/// without any Direct2D dependency.

/// Aurora progress gradient: violet -> fuchsia -> cyan.
/// True if (px, py) lies inside a rounded rectangle.
/// Frosted-glass fill: dims the background in a rounded rect, then adds a tint.
/// Aurora focus halo: a gradient glow band just outside a rounded rect.
/// Soft radial glow blob for the aurora background.
unsafe fn draw_glow_blob(
    pixels: &mut [u32],
    w: u32,
    h: u32,
    cx: f32,
    cy: f32,
    radius: f32,
    r: u8,
    g: u8,
    b: u8,
    max_alpha: f32,
) {
    let x0 = (cx - radius).floor().max(0.0) as i32;
    let x1 = (cx + radius).ceil().min(w as f32) as i32;
    let y0 = (cy - radius).floor().max(0.0) as i32;
    let y1 = (cy + radius).ceil().min(h as f32) as i32;
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = (x as f32 + 0.5 - cx) / radius;
            let dy = (y as f32 + 0.5 - cy) / radius;
            let d2 = dx * dx + dy * dy;
            if d2 >= 1.0 {
                continue;
            }
            let a = max_alpha * (1.0 - d2).powf(1.5);
            if a <= 0.004 {
                continue;
            }
            let idx = (y as u32 * w + x as u32) as usize;
            let dst = pixels[idx];
            let dr = ((dst >> 16) & 0xFF) as f32;
            let dg = ((dst >> 8) & 0xFF) as f32;
            let db = (dst & 0xFF) as f32;
            let mix = |s: f32, d: f32| -> u32 { (s * a + d * (1.0 - a)).round() as u32 };
            let rr = mix(r as f32, dr);
            let gg = mix(g as f32, dg);
            let bb = mix(b as f32, db);
            pixels[idx] = 0xFF00_0000 | (rr << 16) | (gg << 8) | bb;
        }
    }
}

/// Renders one animation frame of the stretching mascot.
///
/// `t` is seconds since the break started. The character breathes, blinks and
/// raises its arms in a slow stretch loop, with sparkles drifting upward.
unsafe fn paint(hwnd: HWND) {
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);
    let width = (client.right - client.left).max(1);
    let height = (client.bottom - client.top).max(1);
    let short = width.min(height) as f32;

    let factory = {
        let mut guard = D2D_FACTORY.lock();
        if guard.is_none() {
            *guard =
                D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok();
        }
        guard.as_ref().cloned()
    };
    let Some(factory) = factory else { return };
    let write = {
        let mut guard = DWRITE_FACTORY.lock();
        if guard.is_none() {
            *guard = DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED).ok();
        }
        guard.as_ref().cloned()
    };
    let Some(write) = write else { return };

    let key = hwnd.0 as isize;
    let mut targets = WINDOW_TARGETS.lock();
    let map = targets.get_or_insert_with(HashMap::new);
    if !map.contains_key(&key) {
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: D2D_SIZE_U {
                width: width as u32,
                height: height as u32,
            },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let Ok(target) = factory.CreateHwndRenderTarget(&props, &hwnd_props) else {
            return;
        };
        map.insert(
            key,
            WindowTarget {
                target,
                capture_bitmap: None,
                capture_version: 0,
            },
        );
    }
    let wt = map.get_mut(&key).unwrap();
    let target = wt.target.clone();

    let version = CAPTURE_VERSION.load(Ordering::SeqCst);
    if wt.capture_bitmap.is_none() || wt.capture_version != version {
        wt.capture_version = version;
        wt.capture_bitmap = rebuild_capture_bitmap(&target);
    }

    let _ = target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
    let _ = target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

    target.BeginDraw();

    let bg = d2d_color(11, 9, 19, 1.0);
    let _ = target.Clear(Some(&bg));
    if let Some(bmp) = &wt.capture_bitmap {
        if let Some(capture) = CAPTURE.lock().as_ref().map(|c| c.clone()) {
            let mut win = RECT::default();
            let _ = GetWindowRect(hwnd, &mut win);
            let src = D2D_RECT_F {
                left: (win.left - capture.origin_x) as f32,
                top: (win.top - capture.origin_y) as f32,
                right: (win.left - capture.origin_x + width) as f32,
                bottom: (win.top - capture.origin_y + height) as f32,
            };
            let dest = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: width as f32,
                bottom: height as f32,
            };
            let _ = target.DrawBitmap(
                Some(bmp),
                Some(&dest),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                Some(&src),
            );
        }
    }

    let content = CONTENT.lock().clone();
    // Redesign phase 1 layout: wide screens split into a left
    // clock/date/message column (~42%) and a right column carrying the
    // existing flow (mascot, ring, stats, passphrase) around `cx`.
    // Narrow screens keep the original single centred column.
    let wide = width >= 1100 || width as f32 >= height as f32 * 1.4;
    let cx = if wide {
        width as f32 * 0.71
    } else {
        width as f32 / 2.0
    };
    // Horizontal span for the full-width rows (stats, hint) in the column.
    let (col_left, col_right) = if wide {
        (width as f32 * 0.48, width as f32)
    } else {
        (0.0, width as f32)
    };

    let screen = GetDC(None);
    let dpi = if screen.is_invalid() {
        96
    } else {
        GetDeviceCaps(screen, LOGPIXELSY).max(96)
    };
    if !screen.is_invalid() {
        let _ = ReleaseDC(None, screen);
    }
    let scale = dpi as f32 / 96.0;
    let g = 8.0 * scale;
    let mut cursor = short * 0.045;

    // ---- Clock + date ----
    // Computed fresh on every paint (the overlay repaints at ~25 fps while
    // active), so no new content field is needed. Weekday names are mapped
    // by hand instead of relying on the system locale.
    let now = Local::now();
    let clock_text = format!("{:02}:{:02}", now.hour(), now.minute());
    let weekday_cn = match now.weekday() {
        chrono::Weekday::Mon => "星期一",
        chrono::Weekday::Tue => "星期二",
        chrono::Weekday::Wed => "星期三",
        chrono::Weekday::Thu => "星期四",
        chrono::Weekday::Fri => "星期五",
        chrono::Weekday::Sat => "星期六",
        chrono::Weekday::Sun => "星期日",
    };
    let date_text = format!("{}月{}日 {}", now.month(), now.day(), weekday_cn);
    if wide {
        // Left column: big light clock, date, then the break message as
        // plain text (no speech-bubble box in this layout).
        let clock_size = short * 0.13;
        let col_cx = width as f32 * 0.24;
        let col_w = width as f32 * 0.40;
        let mut ly = height as f32 * 0.24;
        if let Some(fmt) = text_format(&write, w!("Segoe UI"), clock_size, DWRITE_FONT_WEIGHT_LIGHT, true, true) {
            if let Some(white) = solid_brush(&target, 255, 255, 255, 1.0) {
                let rect = D2D_RECT_F {
                    left: col_cx - col_w / 2.0,
                    top: ly,
                    right: col_cx + col_w / 2.0,
                    bottom: ly + clock_size * 1.25,
                };
                draw_text_d2d(&target, &fmt, &clock_text, &rect, &white);
            }
        }
        ly += clock_size * 1.25 + g * 2.0;
        if let Some(fmt) = text_format(&write, w!("Segoe UI"), 20.0 * scale, DWRITE_FONT_WEIGHT_NORMAL, true, true) {
            if let Some(c) = solid_brush(&target, 216, 210, 232, 1.0) {
                let rect = D2D_RECT_F {
                    left: col_cx - col_w / 2.0,
                    top: ly,
                    right: col_cx + col_w / 2.0,
                    bottom: ly + 28.0 * scale,
                };
                draw_text_d2d(&target, &fmt, &date_text, &rect, &c);
            }
        }
        ly += 28.0 * scale + g * 4.0;
        if !content.message.is_empty() {
            if let Some(fmt) = text_format(&write, w!("Segoe UI"), 20.0 * scale, DWRITE_FONT_WEIGHT_NORMAL, true, false) {
                if let Some(white) = solid_brush(&target, 255, 255, 255, 0.92) {
                    let rect = D2D_RECT_F {
                        left: col_cx - col_w / 2.0,
                        top: ly,
                        right: col_cx + col_w / 2.0,
                        bottom: ly + 20.0 * scale * 1.5 * 3.0,
                    };
                    draw_text_d2d(&target, &fmt, &content.message, &rect, &white);
                }
            }
        }
    } else {
        // Narrow: a compact clock + date centred above the existing flow.
        let nsize = short * 0.11;
        if let Some(fmt) = text_format(&write, w!("Segoe UI"), nsize, DWRITE_FONT_WEIGHT_LIGHT, true, true) {
            if let Some(white) = solid_brush(&target, 255, 255, 255, 1.0) {
                let rect = D2D_RECT_F {
                    left: 0.0,
                    top: cursor,
                    right: width as f32,
                    bottom: cursor + nsize * 1.25,
                };
                draw_text_d2d(&target, &fmt, &clock_text, &rect, &white);
            }
        }
        cursor += nsize * 1.25 + g;
        if let Some(fmt) = text_format(&write, w!("Segoe UI"), 16.0 * scale, DWRITE_FONT_WEIGHT_NORMAL, true, true) {
            if let Some(c) = solid_brush(&target, 216, 210, 232, 1.0) {
                let rect = D2D_RECT_F {
                    left: 0.0,
                    top: cursor,
                    right: width as f32,
                    bottom: cursor + 22.0 * scale,
                };
                draw_text_d2d(&target, &fmt, &date_text, &rect, &c);
            }
        }
        cursor += 22.0 * scale + g * 3.0;
    }

    // ---- Speech bubble (narrow layout only; wide uses the left column) ----
    let bubble_font = 20.0 * scale;
    let max_bubble_w = short * 0.80;
    if !wide && !content.message.is_empty() {
        let line_h = bubble_font * 1.5;
        let bubble_h = line_h * 2.0 + 26.0 * scale;
        let bubble_w = max_bubble_w;
        let bubble_left = cx - bubble_w / 2.0;
        let rr = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: bubble_left,
                top: cursor,
                right: bubble_left + bubble_w,
                bottom: cursor + bubble_h,
            },
            radiusX: 18.0 * scale,
            radiusY: 18.0 * scale,
        };
        if let Some(glass) = solid_brush(&target, 255, 255, 255, 0.08) {
            let _ = target.FillRoundedRectangle(&rr, Some(&glass));
        }
        if let Some(border) = solid_brush(&target, 255, 255, 255, 0.12) {
            let _ = target.DrawRoundedRectangle(&rr, Some(&border), 1.0, None);
        }
        if let Some(fmt) = text_format(&write, w!("Segoe UI"), bubble_font, DWRITE_FONT_WEIGHT_NORMAL, true, true) {
            if let Some(white) = solid_brush(&target, 255, 255, 255, 1.0) {
                let rect = D2D_RECT_F {
                    left: bubble_left,
                    top: cursor + 13.0 * scale,
                    right: bubble_left + bubble_w,
                    bottom: cursor + bubble_h - 13.0 * scale,
                };
                draw_text_d2d(&target, &fmt, &content.message, &rect, &white);
            }
        }
        cursor = bubble_h + cursor + g * 3.0;
    }

    // ---- Mascot ----
    let mascot_size = short * 0.30;
    let mascot_rect = RECT {
        left: (cx - mascot_size / 2.0) as i32,
        top: cursor as i32,
        right: (cx + mascot_size / 2.0) as i32,
        bottom: (cursor + mascot_size) as i32,
    };
    draw_mascot_d2d(&target, &mascot_rect, &content);
    cursor = mascot_rect.bottom as f32 + g * 3.0;

    // ---- Progress ring + countdown ----
    if !content.done {
        let ring_diam = short * 0.30;
        let ring_center_x = cx;
        let ring_center_y = cursor + ring_diam / 2.0;
        let fraction = content.fraction;
        draw_ring_d2d(
            &target,
            &factory,
            ring_center_x,
            ring_center_y,
            ring_diam / 2.0,
            ring_diam * 0.06,
            fraction,
        );
        let countdown_size = ring_diam * 0.26;
        if let Some(fmt) = text_format(&write, w!("Microsoft YaHei UI"), countdown_size, DWRITE_FONT_WEIGHT_BOLD, true, true) {
            if let Some(white) = solid_brush(&target, 255, 255, 255, 1.0) {
                let rect = D2D_RECT_F {
                    left: ring_center_x - ring_diam / 2.0,
                    top: ring_center_y - ring_diam / 2.0,
                    right: ring_center_x + ring_diam / 2.0,
                    bottom: ring_center_y + ring_diam / 2.0,
                };
                draw_text_d2d(&target, &fmt, &content.countdown, &rect, &white);
            }
        }
        cursor = ring_center_y + ring_diam / 2.0 + g * 3.0;
    } else {
        cursor = mascot_rect.bottom as f32 + g * 4.0;
    }

    // ---- Stats ----
    if let Some(fmt) = text_format(&write, w!("Segoe UI"), 14.0 * scale, DWRITE_FONT_WEIGHT_NORMAL, true, false) {
        if let Some(c) = solid_brush(&target, 216, 210, 232, 1.0) {
            let rect = D2D_RECT_F {
                left: col_left,
                top: cursor,
                right: col_right,
                bottom: cursor + 20.0 * scale,
            };
            draw_text_d2d(&target, &fmt, &content.stats, &rect, &c);
        }
    }
    cursor += 20.0 * scale + g * 2.0;

    if !content.done {
        // ---- Passphrase field ----
        let field_w = (short * 0.28).max(280.0 * scale);
        let field_h = 48.0 * scale;
        let field = D2D_RECT_F {
            left: cx - field_w / 2.0,
            top: cursor,
            right: cx + field_w / 2.0,
            bottom: cursor + field_h,
        };
        let typed = content.passphrase_len > 0;

        // 玻璃填充
        let field_rr = D2D1_ROUNDED_RECT {
            rect: field,
            radiusX: field_h / 2.0,
            radiusY: field_h / 2.0,
        };
        if let Some(glass) = solid_brush(&target, 255, 255, 255, 0.10) {
            let _ = target.FillRoundedRectangle(&field_rr, Some(&glass));
        }
        // 边框：聚焦时强调渐变，未聚焦时白 15%
        if typed {
            if let Some(edge) = aurora_gradient_brush(&target, cx, cursor + field_h / 2.0, field_w / 2.0) {
                let _ = target.DrawRoundedRectangle(&field_rr, Some(&edge), 1.5 * scale, None);
            }
        } else if let Some(edge) = solid_brush(&target, 255, 255, 255, 0.15) {
            let _ = target.DrawRoundedRectangle(&field_rr, Some(&edge), 1.0, None);
        }

        // 锁图标：锁环 + 锁体，整体垂直居中于胶囊
        if let Some(lock) = solid_brush(&target, 135, 135, 135, 1.0) {
            let lx = field.left + 16.0 * scale;
            let lcy = cursor + field_h / 2.0;
            let lr = 6.0 * scale;
            let ring_cy = lcy - 5.0 * scale;
            let lock_ring = D2D1_ELLIPSE {
                point: D2D_POINT_2F { x: lx, y: ring_cy },
                radiusX: lr,
                radiusY: lr,
            };
            let _ = target.DrawEllipse(&lock_ring, Some(&lock), 1.5 * scale, None);
            let lock_body = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: lx - lr - 1.0 * scale,
                    top: lcy - 1.0 * scale,
                    right: lx + lr + 1.0 * scale,
                    bottom: lcy + 11.0 * scale,
                },
                radiusX: 3.0 * scale,
                radiusY: 3.0 * scale,
            };
            let _ = target.FillRoundedRectangle(&lock_body, Some(&lock));
        }

        // 输入文字 / placeholder
        let text_left = field.left + 34.0 * scale;
        let text_rect = D2D_RECT_F {
            left: text_left,
            top: cursor,
            right: field.right - 10.0 * scale,
            bottom: cursor + field_h,
        };
        if typed {
            // iOS-style dots, drawn as filled circles rather than text
            // bullets: one dot per typed character, showing at most the
            // last 24, centred in the field area right of the lock icon.
            // Pitch/diameter shrink when many dots would overflow.
            let n = content.passphrase_len.min(24);
            if let Some(white) = solid_brush(&target, 255, 255, 255, 1.0) {
                let cy = cursor + field_h / 2.0;
                let area_w = (text_rect.right - text_rect.left).max(1.0);
                let mut d = 10.0 * scale;
                let mut pitch = 14.0 * scale;
                if n > 1 && (n as f32 - 1.0) * pitch + d > area_w {
                    pitch = ((area_w - d) / (n as f32 - 1.0)).max(d * 1.05);
                    d = d.min(pitch * 0.75);
                }
                let span = if n > 1 {
                    (n as f32 - 1.0) * pitch + d
                } else {
                    d
                };
                let mut dx = text_rect.left + (area_w - span) / 2.0 + d / 2.0;
                for _ in 0..n {
                    let dot = D2D1_ELLIPSE {
                        point: D2D_POINT_2F { x: dx, y: cy },
                        radiusX: d / 2.0,
                        radiusY: d / 2.0,
                    };
                    let _ = target.FillEllipse(&dot, Some(&white));
                    dx += pitch;
                }
            }
        } else if let Some(fmt) = text_format(&write, w!("Segoe UI"), 17.0 * scale, DWRITE_FONT_WEIGHT_NORMAL, false, true) {
            if let Some(ph) = solid_brush(&target, 120, 106, 110, 1.0) {
                draw_text_d2d(&target, &fmt, "输入口令", &text_rect, &ph);
            }
        }

        cursor = field.bottom + 6.0 * scale;
        if let Some(fmt) = text_format(&write, w!("Segoe UI"), 13.0 * scale, DWRITE_FONT_WEIGHT_NORMAL, true, false) {
            if let Some(h) = solid_brush(&target, 148, 135, 138, 1.0) {
                let rect = D2D_RECT_F {
                    left: col_left,
                    top: cursor,
                    right: col_right,
                    bottom: cursor + 18.0 * scale,
                };
                draw_text_d2d(&target, &fmt, &content.hint, &rect, &h);
            }
        }
    } else {
        // ---- Resume button ----
        let btn = resume_button_rect(&client, scale);
        let btn_rr = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: btn.left as f32,
                top: btn.top as f32,
                right: btn.right as f32,
                bottom: btn.bottom as f32,
            },
            radiusX: (btn.bottom - btn.top) as f32 / 2.0,
            radiusY: (btn.bottom - btn.top) as f32 / 2.0,
        };
        if let Some(fill) = solid_brush(&target, 167, 139, 250, 1.0) {
            let _ = target.FillRoundedRectangle(&btn_rr, Some(&fill));
        }
        if let Some(fmt) = text_format(&write, w!("Segoe UI"), 20.0 * scale, DWRITE_FONT_WEIGHT_NORMAL, true, true) {
            if let Some(tc) = solid_brush(&target, 34, 20, 54, 1.0) {
                let rect = D2D_RECT_F {
                    left: btn.left as f32,
                    top: btn.top as f32,
                    right: btn.right as f32,
                    bottom: btn.bottom as f32,
                };
                draw_text_d2d(&target, &fmt, "开始使用", &rect, &tc);
            }
        }
    }

    if target.EndDraw(None, None).is_err() {
        let _ = target.Resize(&D2D_SIZE_U {
            width: width as u32,
            height: height as u32,
        });
        wt.capture_bitmap = None;
    }
}


/// Paint the blurred desktop capture for this monitor, or a solid fallback.
/// Composite the current mascot frame into the back-buffer pixels.
#[allow(clippy::too_many_arguments)]
/// Greedy character-wise wrapping (handles CJK); returns lines + widest width.
// ---- Direct2D rendering ----

fn d2d_color(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}


unsafe fn round_stroke(factory: &ID2D1Factory) -> Option<ID2D1StrokeStyle> {
    let props = D2D1_STROKE_STYLE_PROPERTIES {
        startCap: D2D1_CAP_STYLE_ROUND,
        endCap: D2D1_CAP_STYLE_ROUND,
        dashCap: D2D1_CAP_STYLE_ROUND,
        lineJoin: D2D1_LINE_JOIN_ROUND,
        miterLimit: 10.0,
        dashStyle: D2D1_DASH_STYLE_SOLID,
        dashOffset: 0.0,
    };
    factory
        .CreateStrokeStyle(&props, None)
        .ok()
}

unsafe fn solid_brush(
    target: &ID2D1HwndRenderTarget,
    r: u8,
    g: u8,
    b: u8,
    a: f32,
) -> Option<ID2D1Brush> {
    let color = d2d_color(r, g, b, a);
    let sb = target.CreateSolidColorBrush(&color, None).ok()?;
    sb.cast().ok()
}

unsafe fn aurora_gradient_brush(
    target: &ID2D1HwndRenderTarget,
    cx: f32,
    cy: f32,
    radius: f32,
) -> Option<ID2D1Brush> {
    let stops = [
        D2D1_GRADIENT_STOP {
            position: 0.0,
            color: d2d_color(167, 139, 250, 1.0),
        },
        D2D1_GRADIENT_STOP {
            position: 0.5,
            color: d2d_color(240, 171, 252, 1.0),
        },
        D2D1_GRADIENT_STOP {
            position: 1.0,
            color: d2d_color(103, 232, 249, 1.0),
        },
    ];
    let collection = target
        .CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
        .ok()?;
    let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
        startPoint: D2D_POINT_2F { x: cx - radius, y: cy },
        endPoint: D2D_POINT_2F { x: cx + radius, y: cy },
    };
    let lb = target
        .CreateLinearGradientBrush(&props, None, Some(&collection))
        .ok()?;
    lb.cast().ok()
}

unsafe fn text_format(
    write: &IDWriteFactory,
    family: windows::core::PCWSTR,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
    center: bool,
    vcenter: bool,
) -> Option<IDWriteTextFormat> {
    let fmt = write
        .CreateTextFormat(
            family,
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("zh-CN"),
        )
        .ok()?;
    let align = if center {
        DWRITE_TEXT_ALIGNMENT_CENTER
    } else {
        DWRITE_TEXT_ALIGNMENT_LEADING
    };
    let _ = fmt.SetTextAlignment(align);
    let palign = if vcenter {
        DWRITE_PARAGRAPH_ALIGNMENT_CENTER
    } else {
        DWRITE_PARAGRAPH_ALIGNMENT_NEAR
    };
    let _ = fmt.SetParagraphAlignment(palign);
    Some(fmt)
}

unsafe fn draw_text_d2d(
    target: &ID2D1HwndRenderTarget,
    format: &IDWriteTextFormat,
    text: &str,
    rect: &D2D_RECT_F,
    brush: &ID2D1Brush,
) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return;
    }
    let _ = target.DrawText(
        &wide,
        Some(format),
        rect,
        Some(brush),
        D2D1_DRAW_TEXT_OPTIONS_NONE,
        DWRITE_MEASURING_MODE_NATURAL,
    );
}

unsafe fn rebuild_capture_bitmap(target: &ID2D1HwndRenderTarget) -> Option<ID2D1Bitmap> {
    let capture = CAPTURE.lock().as_ref()?.clone();
    let props = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_IGNORE,
        },
        dpiX: 0.0,
        dpiY: 0.0,
    };
    let size = D2D_SIZE_U {
        width: capture.width as u32,
        height: capture.height as u32,
    };
    target
        .CreateBitmap(
            size,
            Some(capture.pixels.as_ptr() as *const core::ffi::c_void),
            capture.width as u32 * 4,
            &props,
        )
        .ok()
}

unsafe fn draw_mascot_d2d(
    target: &ID2D1HwndRenderTarget,
    rect: &RECT,
    content: &OverlayContent,
) {
    let w = (rect.right - rect.left).max(1) as u32;
    let h = (rect.bottom - rect.top).max(1) as u32;
    let size = w.min(h);
    let t = if content.animate {
        animation_ms() as f32 / 1000.0
    } else {
        0.0
    };
    let Some((ow, oh, buf)) = crate::mascot::render_selected_lottie(size, t) else {
        return;
    };

    let typed = content.passphrase_len > 0;
    let (scale, dy) = if content.done {
        let b = (t / 0.5 * std::f32::consts::TAU).sin().abs();
        (1.0 + 0.04 * b, -4.0 * b)
    } else if typed {
        (1.04, 3.0)
    } else {
        let breathe = 1.0 + 0.02 * (t / 2.6 * std::f32::consts::TAU).sin();
        let float = 2.0 * (t / 3.2 * std::f32::consts::TAU).sin();
        (breathe, float)
    };

    let cx = (rect.left + rect.right) as f32 / 2.0;
    let cy = (rect.top + rect.bottom) as f32 / 2.0 + dy;
    let r = size as f32 / 2.0 * scale;

    // 圆形毛玻璃底
    let circle = D2D1_ELLIPSE {
        point: D2D_POINT_2F { x: cx, y: cy },
        radiusX: r,
        radiusY: r,
    };
    if let Some(glass) = solid_brush(target, 255, 255, 255, 0.10) {
        let _ = target.FillEllipse(&circle, Some(&glass));
    }
    if let Some(border) = solid_brush(target, 255, 255, 255, 0.16) {
        let _ = target.DrawEllipse(&circle, Some(&border), 1.0, None);
    }

    // 公仔缩到圆直径 88%，且按动画实际宽高比显示（不变形）
    let cw = r * 2.0 * 0.88;
    let aspect = oh as f32 / ow.max(1) as f32;
    let (dw, dh) = if aspect <= 1.0 {
        (cw, cw * aspect)
    } else {
        (cw / aspect, cw)
    };
    let dest = D2D_RECT_F {
        left: cx - dw / 2.0,
        top: cy - dh / 2.0,
        right: cx + dw / 2.0,
        bottom: cy + dh / 2.0,
    };

    let props = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_R8G8B8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 0.0,
        dpiY: 0.0,
    };
    let sz = D2D_SIZE_U {
        width: ow,
        height: oh,
    };
    if let Ok(bmp) = target.CreateBitmap(
        sz,
        Some(buf.as_ptr() as *const core::ffi::c_void),
        ow * 4,
        &props,
    ) {
        let _ = target.DrawBitmap(
            Some(&bmp),
            Some(&dest),
            1.0,
            D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
            None,
        );
    }
}


unsafe fn draw_ring_d2d(
    target: &ID2D1HwndRenderTarget,
    factory: &ID2D1Factory,
    cx: f32,
    cy: f32,
    radius: f32,
    thickness: f32,
    fraction: f32,
) {
    let stroke = round_stroke(factory);
    if let Some(track) = solid_brush(target, 255, 255, 255, 0.22) {
        let e = D2D1_ELLIPSE {
            point: D2D_POINT_2F { x: cx, y: cy },
            radiusX: radius,
            radiusY: radius,
        };
        let _ = target.DrawEllipse(&e, Some(&track), thickness, stroke.as_ref());
    }
    let frac = fraction.clamp(0.0, 1.0);
    if frac <= 0.001 {
        return;
    }
    if let Some(grad) = aurora_gradient_brush(target, cx, cy, radius) {
        if frac >= 0.999 {
            let e = D2D1_ELLIPSE {
                point: D2D_POINT_2F { x: cx, y: cy },
                radiusX: radius,
                radiusY: radius,
            };
            let _ = target.DrawEllipse(&e, Some(&grad), thickness, stroke.as_ref());
        } else if let Ok(path) = factory.CreatePathGeometry() {
            if let Ok(sink) = path.Open() {
                let from = -std::f32::consts::FRAC_PI_2;
                let to = from + frac * std::f32::consts::TAU;
                let start = D2D_POINT_2F {
                    x: cx + radius * from.cos(),
                    y: cy + radius * from.sin(),
                };
                let end = D2D_POINT_2F {
                    x: cx + radius * to.cos(),
                    y: cy + radius * to.sin(),
                };
                sink.BeginFigure(start, D2D1_FIGURE_BEGIN_FILLED);
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: end,
                    size: D2D_SIZE_F {
                        width: radius,
                        height: radius,
                    },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                    arcSize: if frac > 0.5 {
                        D2D1_ARC_SIZE_LARGE
                    } else {
                        D2D1_ARC_SIZE_SMALL
                    },
                });
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
                let _ = sink.Close();
                if let Ok(geom) = path.cast::<ID2D1Geometry>() {
                    let _ = target.DrawGeometry(Some(&geom), Some(&grad), thickness, stroke.as_ref());
                }
            }
        }
    }
}



/// Update and show the top-right countdown widget.
pub fn widget_update(text: &str) {
    *WIDGET_TEXT.lock() = text.to_owned();
    if let Some(value) = *WIDGET_HWND.lock() {
        unsafe {
            let _ = PostMessageW(HWND(value as _), WM_APP_WIDGET_UPDATE, WPARAM(0), LPARAM(0));
        }
    }
}

/// Hide the top-right countdown widget.
pub fn widget_hide() {
    if let Some(value) = *WIDGET_HWND.lock() {
        unsafe {
            let _ = PostMessageW(HWND(value as _), WM_APP_WIDGET_HIDE, WPARAM(0), LPARAM(0));
        }
    }
}

unsafe fn register_widget_class() -> bool {
    let Ok(hmodule) = GetModuleHandleW(None) else {
        return false;
    };
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(widget_wndproc),
        hInstance: HINSTANCE(hmodule.0),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        lpszClassName: WIDGET_CLASS,
        ..Default::default()
    };
    RegisterClassW(&class) != 0
}

unsafe fn create_widget_window() {
    let Ok(hmodule) = GetModuleHandleW(None) else {
        return;
    };
    let mut work = RECT::default();
    let _ = SystemParametersInfoW(
        SPI_GETWORKAREA,
        0,
        Some(&mut work as *mut RECT as *mut core::ffi::c_void),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    let w = 96i32;
    let h = 34i32;
    let x = work.right - w - 12;
    let y = work.top + 12;
    if let Ok(hwnd) = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT,
        WIDGET_CLASS,
        w!("StretchLock countdown"),
        WS_POPUP,
        x,
        y,
        w,
        h,
        None,
        None,
        HINSTANCE(hmodule.0),
        None,
    ) {
        *WIDGET_HWND.lock() = Some(hwnd.0 as isize);
    }
}

unsafe extern "system" fn widget_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_APP_WIDGET_UPDATE => {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            force_topmost(hwnd);
            let _ = InvalidateRect(hwnd, None, true);
            LRESULT(0)
        }
        WM_APP_WIDGET_HIDE => {
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            widget_paint(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn widget_paint(hwnd: HWND) {
    let width = 96i32;
    let height = 34i32;
    let screen = GetDC(None);
    if screen.is_invalid() {
        return;
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = match CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, None, 0) {
        Ok(d) => d,
        Err(_) => {
            let _ = ReleaseDC(None, screen);
            return;
        }
    };
    let mem = CreateCompatibleDC(screen);
    let old_dib = SelectObject(mem, dib);

    // 1) 手动填充圆角胶囊（半透明深色，预乘 alpha）
    let count = (width * height) as usize;
    let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, count);
    let r = height as f32 / 2.0;
    let a = 150u32;
    let pr = (20 * a / 255) as u32;
    let pg = (16 * a / 255) as u32;
    let pb = (28 * a / 255) as u32;
    for y in 0..height {
        for x in 0..width {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let inside = if fx < r {
                let dx = r - fx;
                let dy = fy - r;
                dx * dx + dy * dy <= r * r
            } else if fx >= width as f32 - r {
                let dx = fx - (width as f32 - r);
                let dy = fy - r;
                dx * dx + dy * dy <= r * r
            } else {
                true
            };
            let idx = (y * width + x) as usize;
            pixels[idx] = if inside {
                (a << 24) | (pr << 16) | (pg << 8) | pb
            } else {
                0
            };
        }
    }

    // 2) GDI 画白字
    let _ = SetBkMode(mem, TRANSPARENT);
    let _ = SetTextColor(mem, COLORREF(0x00FFFFFF));
    let font = CreateFontW(-15, 0, 0, 0, 600, 0, 0, 0, 0, 0, 0, 0, 0, w!("Segoe UI"));
    let old_font = SelectObject(mem, font);
    let text = WIDGET_TEXT.lock().clone();
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    let _ = DrawTextW(mem, &mut wide, &mut rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    let _ = SelectObject(mem, old_font);
    let _ = DeleteObject(font);

    // 3) 补 alpha：GDI 写的字 alpha=0，补为 255
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) as usize;
            let p = pixels[idx];
            let alpha = (p >> 24) & 0xFF;
            let rgb = p & 0x00FF_FFFF;
            if alpha == 0 && rgb != 0 {
                pixels[idx] = 0xFF00_0000 | rgb;
            }
        }
    }

    // 4) UpdateLayeredWindow
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let size = SIZE { cx: width, cy: height };
    let pt_src = POINT { x: 0, y: 0 };
    let _ = UpdateLayeredWindow(
        hwnd,
        screen,
        None,
        Some(&size),
        mem,
        Some(&pt_src),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );

    let _ = SelectObject(mem, old_dib);
    let _ = DeleteObject(dib);
    let _ = DeleteDC(mem);
    let _ = ReleaseDC(None, screen);
}

