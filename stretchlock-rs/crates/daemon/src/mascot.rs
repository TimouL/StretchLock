//! Sprite mascot with GIF support and selectable bundled characters.
//!
//! Priority:
//!   1. `%APPDATA%\StretchLock\sprites\mascot.gif` — animated GIF (per-frame delays);
//!   2. `%APPDATA%\StretchLock\sprites\*.png` — PNG sequence;
//!   3. the bundled character selected in settings (default "cat").
//!
//! Frames are normalised to BGRA, trimmed to their content bounds and
//! downscaled if oversized.

#![cfg(windows)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::sprite::{decode_png, load_gif, load_png_sequence};

/// Sprite frame type, shared with the animal scenes via `sprite`.
pub use crate::sprite::Frame;

enum SpriteSource {
    Timed { frames: Vec<Frame>, delays: Vec<u32> },
    Sequence { frames: Vec<Frame>, fps: f32 },
}

#[derive(Clone, Copy)]
struct BuiltinSet {
    name: &'static str,
    frames: &'static [&'static [u8]],
}

const BUNDLED: &[BuiltinSet] = &[
    BuiltinSet {
        name: "cat",
        frames: &[
            include_bytes!("../../../assets/mascot/cat_00.png"),
            include_bytes!("../../../assets/mascot/cat_01.png"),
            include_bytes!("../../../assets/mascot/cat_02.png"),
            include_bytes!("../../../assets/mascot/cat_03.png"),
            include_bytes!("../../../assets/mascot/cat_04.png"),
            include_bytes!("../../../assets/mascot/cat_05.png"),
            include_bytes!("../../../assets/mascot/cat_06.png"),
            include_bytes!("../../../assets/mascot/cat_07.png"),
            include_bytes!("../../../assets/mascot/cat_08.png"),
            include_bytes!("../../../assets/mascot/cat_09.png"),
        ],
    },
    BuiltinSet {
        name: "dog",
        frames: &[
            include_bytes!("../../../assets/mascot/dog_00.png"),
            include_bytes!("../../../assets/mascot/dog_01.png"),
            include_bytes!("../../../assets/mascot/dog_02.png"),
            include_bytes!("../../../assets/mascot/dog_03.png"),
            include_bytes!("../../../assets/mascot/dog_04.png"),
            include_bytes!("../../../assets/mascot/dog_05.png"),
            include_bytes!("../../../assets/mascot/dog_06.png"),
            include_bytes!("../../../assets/mascot/dog_07.png"),
            include_bytes!("../../../assets/mascot/dog_08.png"),
            include_bytes!("../../../assets/mascot/dog_09.png"),
        ],
    },
    BuiltinSet {
        name: "dino",
        frames: &[
            include_bytes!("../../../assets/mascot/dino_00.png"),
            include_bytes!("../../../assets/mascot/dino_01.png"),
            include_bytes!("../../../assets/mascot/dino_02.png"),
            include_bytes!("../../../assets/mascot/dino_03.png"),
            include_bytes!("../../../assets/mascot/dino_04.png"),
            include_bytes!("../../../assets/mascot/dino_05.png"),
            include_bytes!("../../../assets/mascot/dino_06.png"),
            include_bytes!("../../../assets/mascot/dino_07.png"),
            include_bytes!("../../../assets/mascot/dino_08.png"),
            include_bytes!("../../../assets/mascot/dino_09.png"),
        ],
    },
];

static SOURCE: Mutex<Option<Arc<SpriteSource>>> = Mutex::new(None);
static SELECTED: Mutex<String> = Mutex::new(String::new());
static DESCRIPTION: Mutex<String> = Mutex::new(String::new());

const DEFAULT_FPS: f32 = 12.0;
const PIXEL_ART_MAX_SIDE: u32 = 128;

pub fn builtin_names() -> Vec<&'static str> {
    BUNDLED.iter().map(|s| s.name).collect()
}

pub fn select_set(name: &str) {
    *SELECTED.lock() = name.to_string();
    *SOURCE.lock() = None;
}

pub fn user_sprite_dir() -> PathBuf {
    stretchlock_core::config::get_config_dir().join("sprites")
}

pub fn source_description() -> String {
    DESCRIPTION.lock().clone()
}

fn selected_name() -> String {
    let name = SELECTED.lock().clone();
    if name.is_empty() {
        "cat".to_string()
    } else {
        name
    }
}

fn source() -> Arc<SpriteSource> {
    let mut guard = SOURCE.lock();
    if let Some(src) = guard.as_ref() {
        return src.clone();
    }
    let built = load_source(&selected_name());
    let arc = Arc::new(built);
    *guard = Some(arc.clone());
    arc
}

fn load_source(selected: &str) -> SpriteSource {
    let dir = user_sprite_dir();

    let gif_path = dir.join("mascot.gif");
    if gif_path.is_file() {
        if let Some((frames, delays)) = load_gif(&gif_path) {
            *DESCRIPTION.lock() = format!("GIF ({} 帧, {} fps)", frames.len(), format_fps(&delays));
            return SpriteSource::Timed { frames, delays };
        }
    }

    let png_frames = load_png_sequence(&dir);
    if !png_frames.is_empty() {
        *DESCRIPTION.lock() = format!("PNG 序列 ({} 帧)", png_frames.len());
        return SpriteSource::Sequence { frames: png_frames, fps: DEFAULT_FPS };
    }

    let set = BUNDLED.iter().find(|s| s.name == selected).or_else(|| BUNDLED.first()).cloned().unwrap();
    let frames: Vec<Frame> = set.frames.iter().filter_map(|b| decode_png(b)).collect();
    *DESCRIPTION.lock() = format!("内置角色 {} ({} 帧)", set.name, frames.len());
    SpriteSource::Sequence { frames, fps: DEFAULT_FPS }
}

fn format_fps(delays: &[u32]) -> u32 {
    if delays.is_empty() { return 0; }
    let sum: u32 = delays.iter().sum();
    if sum == 0 { return 0; }
    (delays.len() as f32 / (sum as f32 / 1000.0)).round() as u32
}

#[allow(clippy::too_many_arguments)]
pub fn composite_into(
    out: &mut [u32],
    out_w: u32,
    out_h: u32,
    x: i32,
    y: i32,
    target_w: u32,
    target_h: u32,
    seconds: f32,
) {
    let src = source();
    let (frame, _delay): (&Frame, u32) = match &*src {
        SpriteSource::Timed { frames, delays } => {
            if frames.is_empty() { return; }
            let loop_ms: u64 = delays.iter().map(|d| *d as u64).sum();
            let ms = (seconds * 1000.0) as u64;
            let m = if loop_ms == 0 { 0 } else { ms % loop_ms };
            let mut total = 0u64;
            let mut idx = 0usize;
            for (i, delay) in delays.iter().enumerate() {
                total += *delay as u64;
                if m < total { idx = i; break; }
            }
            let idx = idx.min(frames.len() - 1);
            (&frames[idx], delays[idx])
        }
        SpriteSource::Sequence { frames, fps } => {
            if frames.is_empty() { return; }
            let fps = fps.max(1.0);
            let idx = ((seconds * fps) as usize) % frames.len();
            (&frames[idx], (1000.0 / fps) as u32)
        }
    };
    let _ = _delay;

    let scale = (target_w as f32 / frame.width as f32).min(target_h as f32 / frame.height as f32);
    let draw_w = (frame.width as f32 * scale).max(1.0) as u32;
    let draw_h = (frame.height as f32 * scale).max(1.0) as u32;
    let offset_x = x + ((target_w as i32 - draw_w as i32) / 2);
    let offset_y = y + ((target_h as i32 - draw_h as i32) / 2);
    let nearest = frame.width.max(frame.height) <= PIXEL_ART_MAX_SIDE;

    for dy in 0..draw_h {
        let fy = if nearest { ((dy as f32 / scale) as u32).min(frame.height - 1) as f32 + 0.5 }
            else { (dy as f32 + 0.5) / scale - 0.5 };
        let y0 = fy.floor().max(0.0) as u32;
        let y1 = (y0 + 1).min(frame.height - 1);
        let ty = (fy - y0 as f32).clamp(0.0, 1.0);
        let py = offset_y + dy as i32;
        if py < 0 || py >= out_h as i32 { continue; }
        for dx in 0..draw_w {
            let fx = if nearest { ((dx as f32 / scale) as u32).min(frame.width - 1) as f32 + 0.5 }
                else { (dx as f32 + 0.5) / scale - 0.5 };
            let x0 = fx.floor().max(0.0) as u32;
            let x1 = (x0 + 1).min(frame.width - 1);
            let tx = (fx - x0 as f32).clamp(0.0, 1.0);
            let px = offset_x + dx as i32;
            if px < 0 || px >= out_w as i32 { continue; }
            let p00 = frame.pixels[(y0 * frame.width + x0) as usize];
            let p10 = frame.pixels[(y0 * frame.width + x1) as usize];
            let p01 = frame.pixels[(y1 * frame.width + x0) as usize];
            let p11 = frame.pixels[(y1 * frame.width + x1) as usize];
            let lerp = |a: u32, b: u32, t: f32| a as f32 + (b as f32 - a as f32) * t;
            let ch = |p: u32, shift: u32| (p >> shift) & 0xFF;
            let mut acc = [0f32; 4];
            for (k, shift) in [0u32, 8, 16, 24].iter().enumerate() {
                let top = lerp(ch(p00, *shift), ch(p10, *shift), tx);
                let bottom = lerp(ch(p01, *shift), ch(p11, *shift), tx);
                acc[k] = lerp(top as u32, bottom as u32, ty);
            }
            let alpha = acc[3] / 255.0;
            if alpha <= 0.004 { continue; }
            let idx = (py as u32 * out_w + px as u32) as usize;
            let dst = out[idx];
            let mix = |s: f32, d: u32| -> u32 { (s * alpha + d as f32 * (1.0 - alpha)).round().clamp(0.0, 255.0) as u32 };
            let r = mix(acc[0], (dst >> 16) & 0xFF);
            let g = mix(acc[1], (dst >> 8) & 0xFF);
            let b = mix(acc[2], dst & 0xFF);
            let a = acc[3].round().clamp(0.0, 255.0) as u32;
            out[idx] = (a << 24) | (r << 16) | (g << 8) | b;
        }
    }
}

const SVG_SETS: &[(&str, &str)] = &[
    ("cat", include_str!("../../../assets/mascot/svg/cat.svg")),
    ("dog", include_str!("../../../assets/mascot/svg/dog.svg")),
    ("dino", include_str!("../../../assets/mascot/svg/dino.svg")),
];

/// Render the selected bundled SVG mascot into a square RGBA premultiplied buffer.
pub fn render_selected(size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let name = selected_name();
    let svg = SVG_SETS.iter().find(|(n, _)| *n == name)?.1;
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).ok()?;
    let scale = size as f32 / 36.0;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Some((size, size, pixmap.data().to_vec()))
}

const LOTTIE_SETS: &[(&str, &[u8])] = &[
    ("cat", include_bytes!("../../../assets/mascot/lottie/walking-cat.lottie")),
    ("dog", include_bytes!("../../../assets/mascot/lottie/cat-playing.lottie")),
    ("dino", include_bytes!("../../../assets/mascot/lottie/happy-monkey-astronaut.lottie")),
];

fn media_type_for_path(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default().to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "application/octet-stream",
    }
}

fn load_dotlottie_animation(bytes: &[u8]) -> Option<rasterlottie::Animation> {
    use base64::Engine as _;
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut json: Option<String> = None;
    let mut images: HashMap<String, Vec<u8>> = HashMap::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).ok()?;
        let name = file.name().to_string();
        if name.starts_with("animations/") && name.ends_with(".json") {
            let mut s = String::new();
            file.read_to_string(&mut s).ok()?;
            json = Some(s);
        } else if name.starts_with("images/") {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).ok()?;
            images.insert(name, buf);
        }
    }
    let mut animation = rasterlottie::Animation::from_json_str(json.as_deref()?).ok()?;
    for asset in &mut animation.assets {
        if !asset.is_image_asset() || asset.is_embedded_image_asset() {
            continue;
        }
        let path = asset.path.as_deref().unwrap_or_default();
        let mut candidates = vec![path.to_string(), format!("images/{path}")];
        if let Some(base) = asset.base_path.as_deref() {
            candidates.push(format!("{base}/{path}"));
        }
        if let Some(raw) = candidates.into_iter().find_map(|k| images.get(&k).cloned()) {
            let media = media_type_for_path(path);
            asset.base_path = None;
            asset.embedded = Some(1);
            asset.path = Some(format!(
                "data:{media};base64,{}",
                base64::engine::general_purpose::STANDARD.encode(raw)
            ));
        }
    }
    Some(animation)
}

/// Render the selected bundled Lottie animation into a square RGBA buffer at time `t`.
struct CachedLottie {
    prepared: rasterlottie::PreparedAnimation,
    width: u32,
    in_point: f32,
    out_point: f32,
    frame_rate: f32,
}

thread_local! {
    static LOTTIE_CACHE: std::cell::RefCell<HashMap<String, CachedLottie>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Render the selected bundled Lottie animation at time `t`, scaled to `size` wide.
pub fn render_selected_lottie(size: u32, t: f32) -> Option<(u32, u32, Vec<u8>)> {
    let name = selected_name();
    LOTTIE_CACHE.with(|slot| {
        let mut cache = slot.borrow_mut();
        if !cache.contains_key(&name) {
            let bytes = LOTTIE_SETS.iter().find(|(n, _)| *n == name)?.1;
            let animation = load_dotlottie_animation(bytes)?;
            let prepared = rasterlottie::Renderer::target_corpus()
                .prepare(&animation)
                .ok()?;
            cache.insert(
                name.clone(),
                CachedLottie {
                    width: animation.width,
                    in_point: animation.in_point,
                    out_point: animation.out_point,
                    frame_rate: animation.frame_rate,
                    prepared,
                },
            );
        }
        let cached = cache.get(&name)?;
        let duration = cached.out_point - cached.in_point;
        let frame = if duration > 0.0 {
            cached.in_point + (t * cached.frame_rate) % duration
        } else {
            cached.in_point
        };
        let scale = size as f32 / cached.width as f32;
        let mut config = rasterlottie::RenderConfig::default();
        config.scale = scale;
        let out = cached.prepared.render_frame(frame, config).ok()?;
        // 水平翻转（原始动画朝左，翻转后朝右，与正面贴纸视觉一致）
        let ow = out.width as usize;
        let oh = out.height as usize;
        let mut pixels = out.pixels;
        for y in 0..oh {
            let row = y * ow * 4;
            for x in 0..ow / 2 {
                let a = row + x * 4;
                let b = row + (ow - 1 - x) * 4;
                for c in 0..4 {
                    pixels.swap(a + c, b + c);
                }
            }
        }
        Some((out.width, out.height, pixels))
    })
}

