//! Background scene layer for the break overlay: the 16 lock-screen
//! scenes from `stretchlock_core::scene`.
//!
//! The 9 ambient scenes are drawn procedurally with Direct2D primitives
//! (parameters come from each scene's `SceneSpec`); the 7 animal scenes
//! blit the bundled SVG sprites from `animal_scene`, with trajectories
//! computed here from the animation time. If an animal sprite fails to
//! render, the layer falls back to the star scene (with one warning per
//! scene) so the overlay never ends up blank.
//!
//! Everything here is decorative: it draws between the captured desktop
//! bitmap and the overlay's text content, and takes no input.

#![cfg(windows)]

use std::collections::HashSet;
use std::f32::consts::{PI, TAU};

use parking_lot::Mutex;
use windows::core::{w, Interface};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use stretchlock_core::scene::{spread, LockScene};

use crate::overlay::{draw_text_d2d, solid_brush, text_format};

fn frac(x: f32) -> f32 {
    x - x.floor()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Deterministic per-element placement value in 0..1.
fn sp(i: usize, salt: u32) -> f32 {
    spread(salt.wrapping_add((i as u32).wrapping_mul(7)))
}

/// Per-element duration interpolated from the spec range.
fn dur(range: (f32, f32), i: usize, salt: u32) -> f32 {
    lerp(range.0, range.1, sp(i, salt))
}

fn pt(p: (f32, f32)) -> D2D_POINT_2F {
    D2D_POINT_2F { x: p.0, y: p.1 }
}

fn rot(px: f32, py: f32, ang: f32) -> (f32, f32) {
    let (s, c) = ang.sin_cos();
    (px * c - py * s, px * s + py * c)
}

fn xlate(cx: f32, cy: f32, p: (f32, f32), ang: f32) -> (f32, f32) {
    let (x, y) = rot(p.0, p.1, ang);
    (cx + x, cy + y)
}

unsafe fn fill_circle(target: &ID2D1HwndRenderTarget, cx: f32, cy: f32, r: f32, brush: &ID2D1Brush) {
    let e = D2D1_ELLIPSE {
        point: D2D_POINT_2F { x: cx, y: cy },
        radiusX: r,
        radiusY: r,
    };
    let _ = target.FillEllipse(&e, Some(brush));
}

unsafe fn fill_ellipse(
    target: &ID2D1HwndRenderTarget,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    brush: &ID2D1Brush,
) {
    let e = D2D1_ELLIPSE {
        point: D2D_POINT_2F { x: cx, y: cy },
        radiusX: rx,
        radiusY: ry,
    };
    let _ = target.FillEllipse(&e, Some(brush));
}

unsafe fn stroke_ellipse(
    target: &ID2D1HwndRenderTarget,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    brush: &ID2D1Brush,
    width: f32,
) {
    let e = D2D1_ELLIPSE {
        point: D2D_POINT_2F { x: cx, y: cy },
        radiusX: rx,
        radiusY: ry,
    };
    let _ = target.DrawEllipse(&e, Some(brush), width, None);
}

unsafe fn line(
    target: &ID2D1HwndRenderTarget,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    brush: &ID2D1Brush,
    width: f32,
) {
    let _ = target.DrawLine(pt((x1, y1)), pt((x2, y2)), Some(brush), width, None);
}

unsafe fn fill_poly(
    target: &ID2D1HwndRenderTarget,
    factory: &ID2D1Factory,
    points: &[(f32, f32)],
    brush: &ID2D1Brush,
) {
    if points.len() < 3 {
        return;
    }
    let Ok(path) = factory.CreatePathGeometry() else { return };
    let Ok(sink) = path.Open() else { return };
    sink.BeginFigure(pt(points[0]), D2D1_FIGURE_BEGIN_FILLED);
    for p in &points[1..] {
        sink.AddLine(pt(*p));
    }
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    let _ = sink.Close();
    if let Ok(geom) = path.cast::<ID2D1Geometry>() {
        let _ = target.FillGeometry(Some(&geom), Some(brush), None);
    }
}

/// Draw one frame of the scene layer. `t` is seconds since the break
/// started; `width`/`height` are the overlay client size.
pub unsafe fn draw_scene(
    target: &ID2D1HwndRenderTarget,
    write: &IDWriteFactory,
    factory: &ID2D1Factory,
    scene: LockScene,
    t: f32,
    width: f32,
    height: f32,
) {
    let short = width.min(height).max(1.0);
    // Unit: prototype sizes are CSS px on a ~800 px-short-side canvas.
    let u = short / 800.0;
    match scene {
        LockScene::Fireflies => fireflies(target, t, width, height, u),
        LockScene::Stars => stars(target, t, width, height, u),
        LockScene::Bubbles => bubbles(target, t, width, height, u),
        LockScene::Fish => fish(target, factory, t, width, height, u),
        LockScene::Plane => plane(target, factory, t, width, height, u),
        LockScene::Rain => rain(target, t, width, height, u),
        LockScene::Notes => notes(target, write, t, width, height, u),
        LockScene::Moon => moon(target, t, width, height, u),
        LockScene::Leaves => leaves(target, factory, t, width, height, u),
        animal => {
            if !animals(target, animal, t, width, height, short) {
                warn_fallback_once(animal);
                stars(target, t, width, height, u);
            }
        }
    }
}

static FALLBACK_WARNED: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

fn warn_fallback_once(scene: LockScene) {
    let mut guard = FALLBACK_WARNED.lock();
    let set = guard.get_or_insert_with(HashSet::new);
    if set.insert(scene.id()) {
        log::warn!(
            "Animal scene {} ({}) sprite unavailable; falling back to stars",
            scene.name(),
            scene.id()
        );
    }
}

// ---------------------------------------------------------------------------
// Ambient scenes (procedural)
// ---------------------------------------------------------------------------

unsafe fn fireflies(target: &ID2D1HwndRenderTarget, t: f32, w: f32, h: f32, u: f32) {
    let spec = LockScene::Fireflies.spec();
    for i in 0..spec.count {
        let bx = sp(i, 11) * w;
        let by = h * (0.30 + 0.55 * sp(i, 23));
        let d = dur(spec.duration_s, i, 37);
        let ph = sp(i, 41) * TAU;
        let x = bx + (t * TAU / d * 0.7 + ph).sin() * 30.0 * u + (t * TAU / d + ph * 2.0).cos() * 12.0 * u;
        let y = by + (t * TAU / d * 0.9 + ph).sin() * 20.0 * u;
        let pulse = 0.5 + 0.5 * (t * TAU / d + ph).sin();
        let a = 0.25 + 0.75 * pulse;
        let r = lerp(spec.size_px.0, spec.size_px.1, sp(i, 61)) * u * 0.5;
        if let Some(glow) = solid_brush(target, 220, 255, 178, 0.10 * a) {
            fill_circle(target, x, y, r * 3.2, &glow);
        }
        if let Some(core) = solid_brush(target, 220, 255, 178, a) {
            fill_circle(target, x, y, r, &core);
        }
    }
}

unsafe fn stars(target: &ID2D1HwndRenderTarget, t: f32, w: f32, h: f32, u: f32) {
    let spec = LockScene::Stars.spec();
    for i in 0..spec.count {
        let x = sp(i, 101) * w;
        let y = sp(i, 113) * h * 0.56;
        let d = dur(spec.duration_s, i, 127);
        let pulse = 0.5 + 0.5 * (t * TAU / d + sp(i, 131) * TAU).sin();
        let alpha = 0.12 + 0.88 * pulse;
        let r = lerp(spec.size_px.0, spec.size_px.1, sp(i, 139)) * u * 0.5 * (0.65 + 0.65 * pulse);
        if let Some(b) = solid_brush(target, 255, 255, 255, alpha) {
            fill_circle(target, x, y, r, &b);
        }
        // A faint cross sparkle on the brighter stars.
        if pulse > 0.72 {
            if let Some(b) = solid_brush(target, 255, 255, 255, alpha * 0.45) {
                let arm = r * 3.0;
                line(target, x - arm, y, x + arm, y, &b, 1.0);
                line(target, x, y - arm, x, y + arm, &b, 1.0);
            }
        }
    }
}

unsafe fn bubbles(target: &ID2D1HwndRenderTarget, t: f32, w: f32, h: f32, u: f32) {
    let spec = LockScene::Bubbles.spec();
    for i in 0..spec.count {
        let d = dur(spec.duration_s, i, 211);
        let p = frac(t / d + sp(i, 211));
        let r = lerp(spec.size_px.0, spec.size_px.1, sp(i, 223)) * u * 0.5;
        let x = sp(i, 227) * w + (t * TAU / d * 2.0 + sp(i, 229) * TAU).sin() * 14.0 * u;
        let y = h * 1.05 - p * h * 1.15;
        if let Some(ring) = solid_brush(target, 255, 255, 255, 0.55) {
            stroke_ellipse(target, x, y, r, r, &ring, 1.5);
        }
        if let Some(hi) = solid_brush(target, 255, 255, 255, 0.7) {
            fill_circle(target, x - r * 0.35, y - r * 0.35, r * 0.18, &hi);
        }
    }
}

unsafe fn fish(
    target: &ID2D1HwndRenderTarget,
    factory: &ID2D1Factory,
    t: f32,
    w: f32,
    h: f32,
    u: f32,
) {
    const COLORS: [(u8, u8, u8); 3] = [(134, 239, 172), (253, 186, 116), (125, 211, 252)];
    const LENS: [f32; 3] = [56.0, 38.0, 46.0];
    const DURS: [f32; 3] = [24.0, 31.0, 27.0];
    const ROWS: [f32; 3] = [0.20, 0.42, 0.63];
    for i in 0..3 {
        let len = LENS[i] * u;
        let p = frac(t / DURS[i] + sp(i, 311));
        let x = -len + p * (w + 2.0 * len);
        let yc = h * ROWS[i] + (t * TAU / 3.2 + i as f32 * 1.7).sin() * 11.0 * u;
        let (r, g, b) = COLORS[i];
        let fh = len * 0.55;
        if let Some(body) = solid_brush(target, r, g, b, 0.92) {
            fill_poly(
                target,
                factory,
                &[
                    (x - len * 0.36, yc),
                    (x - len * 0.74, yc - fh * 0.52),
                    (x - len * 0.74, yc + fh * 0.52),
                ],
                &body,
            );
            fill_ellipse(target, x, yc, len * 0.5, fh * 0.5, &body);
        }
        if let Some(eye) = solid_brush(target, 15, 23, 42, 0.9) {
            fill_circle(target, x + len * 0.28, yc - fh * 0.08, len * 0.055, &eye);
        }
    }
}

unsafe fn plane(
    target: &ID2D1HwndRenderTarget,
    factory: &ID2D1Factory,
    t: f32,
    w: f32,
    h: f32,
    u: f32,
) {
    let p = frac(t / 15.0);
    let x = -60.0 * u + p * (w + 120.0 * u);
    let y = h * 0.20 + (p * TAU).sin() * h * 0.03 - p * h * 0.04;
    let ang = -0.12 + 0.30 * (p * TAU).cos();
    let s = u;
    let nose = (26.0 * s, 0.0);
    let back_top = (-20.0 * s, -16.0 * s);
    let back_mid = (-12.0 * s, 0.0);
    let back_bot = (-20.0 * s, 16.0 * s);
    if let Some(b) = solid_brush(target, 255, 255, 255, 0.92) {
        fill_poly(
            target,
            factory,
            &[
                xlate(x, y, nose, ang),
                xlate(x, y, back_top, ang),
                xlate(x, y, back_mid, ang),
            ],
            &b,
        );
    }
    if let Some(b) = solid_brush(target, 226, 232, 240, 0.65) {
        fill_poly(
            target,
            factory,
            &[
                xlate(x, y, nose, ang),
                xlate(x, y, back_mid, ang),
                xlate(x, y, back_bot, ang),
            ],
            &b,
        );
    }
}

unsafe fn rain(target: &ID2D1HwndRenderTarget, t: f32, w: f32, h: f32, u: f32) {
    let spec = LockScene::Rain.spec();
    if let Some(b) = solid_brush(target, 147, 197, 253, 0.5) {
        for i in 0..spec.count {
            let d = dur(spec.duration_s, i, 401);
            let len = lerp(spec.size_px.0, spec.size_px.1, sp(i, 409)) * u;
            let p = frac(t / d + sp(i, 419));
            let x = sp(i, 421) * w;
            let y = -len + p * (h + len);
            line(target, x, y, x + len * 0.122, y + len * 0.992, &b, 1.5);
        }
    }
    // Ripples along the bottom edge.
    for i in 0..4 {
        let fx = [0.12, 0.38, 0.63, 0.84][i] * w;
        let p = frac(t / 2.6 + i as f32 * 0.25);
        let rx = (4.0 + 38.0 * p) * u;
        if let Some(b) = solid_brush(target, 147, 197, 253, (1.0 - p) * 0.45) {
            stroke_ellipse(target, fx, h * 0.95, rx, rx * 0.26, &b, 1.5);
        }
    }
}

unsafe fn notes(
    target: &ID2D1HwndRenderTarget,
    write: &IDWriteFactory,
    t: f32,
    w: f32,
    h: f32,
    u: f32,
) {
    const GLYPHS: [&str; 3] = ["\u{266B}", "\u{266A}", "\u{2669}"];
    const COLORS: [(u8, u8, u8); 3] = [(240, 171, 252), (103, 232, 249), (167, 139, 250)];
    let spec = LockScene::Notes.spec();
    for i in 0..spec.count {
        let d = dur(spec.duration_s, i, 503);
        let p = frac(t / d + sp(i, 509));
        let size = lerp(spec.size_px.0, spec.size_px.1, sp(i, 521)) * u;
        let x = sp(i, 523) * w + (p * TAU + i as f32).sin() * 16.0 * u;
        let y = h * 0.84 - p * h * 0.64;
        let alpha = (p * PI).sin().clamp(0.0, 1.0) * 0.85;
        let (r, g, b) = COLORS[i % 3];
        let Some(fmt) = text_format(write, w!("Segoe UI"), size, DWRITE_FONT_WEIGHT_NORMAL, true, true) else {
            continue;
        };
        let Some(brush) = solid_brush(target, r, g, b, alpha) else {
            continue;
        };
        let rect = D2D_RECT_F {
            left: x - size,
            top: y - size,
            right: x + size,
            bottom: y + size,
        };
        draw_text_d2d(target, &fmt, GLYPHS[i % 3], &rect, &brush);
    }
}

unsafe fn moon(target: &ID2D1HwndRenderTarget, t: f32, w: f32, h: f32, u: f32) {
    // Clouds drift behind the moon.
    const CW: [f32; 3] = [130.0, 88.0, 170.0];
    const CD: [f32; 3] = [52.0, 40.0, 64.0];
    const CY: [f32; 3] = [0.10, 0.20, 0.06];
    for i in 0..3 {
        let cw = CW[i] * u;
        let p = frac(t / CD[i] + sp(i, 601));
        let x = -cw + p * (w + 2.0 * cw);
        let y = h * CY[i];
        if let Some(b) = solid_brush(target, 255, 255, 255, 0.12) {
            fill_ellipse(target, x, y, cw * 0.30, cw * 0.13, &b);
            fill_ellipse(target, x + cw * 0.24, y + cw * 0.05, cw * 0.24, cw * 0.11, &b);
            fill_ellipse(target, x - cw * 0.24, y + cw * 0.05, cw * 0.24, cw * 0.11, &b);
        }
    }
    // Crescent: pale disc with a dark disc offset over it (the backdrop
    // is tinted toward the same deep violet, so the bite reads cleanly).
    let r = 36.0 * u;
    let cx = w * 0.84 + (t * TAU / 7.0).sin() * 6.0 * u;
    let cy = h * 0.15;
    if let Some(b) = solid_brush(target, 254, 240, 138, 0.95) {
        fill_circle(target, cx, cy, r, &b);
    }
    if let Some(b) = solid_brush(target, 11, 9, 19, 0.88) {
        fill_circle(target, cx + r * 0.45, cy - r * 0.18, r * 0.86, &b);
    }
}

unsafe fn leaves(
    target: &ID2D1HwndRenderTarget,
    factory: &ID2D1Factory,
    t: f32,
    w: f32,
    h: f32,
    u: f32,
) {
    const COLORS: [(u8, u8, u8); 2] = [(134, 239, 172), (251, 146, 60)];
    let spec = LockScene::Leaves.spec();
    for i in 0..spec.count {
        let d = dur(spec.duration_s, i, 701);
        let p = frac(t / d + sp(i, 709));
        let size = lerp(spec.size_px.0, spec.size_px.1, sp(i, 719)) * u * 1.4;
        let y = -20.0 * u + p * (h + 40.0 * u);
        let x = sp(i, 727) * w + (p * TAU * 1.5 + i as f32).sin() * 30.0 * u;
        let dir = if i % 2 == 0 { 1.0 } else { -1.0 };
        let ang = dir * p * 380.0_f32.to_radians() + sp(i, 733) * TAU;
        let (r, g, b) = COLORS[i % 2];
        if let Some(brush) = solid_brush(target, r, g, b, 0.85) {
            fill_leaf(target, factory, x, y, size, ang, &brush);
        }
    }
}

unsafe fn fill_leaf(
    target: &ID2D1HwndRenderTarget,
    factory: &ID2D1Factory,
    cx: f32,
    cy: f32,
    len: f32,
    ang: f32,
    brush: &ID2D1Brush,
) {
    let Ok(path) = factory.CreatePathGeometry() else { return };
    let Ok(sink) = path.Open() else { return };
    let tip = (0.0, -len / 2.0);
    let tail = (0.0, len / 2.0);
    sink.BeginFigure(pt(xlate(cx, cy, tip, ang)), D2D1_FIGURE_BEGIN_FILLED);
    sink.AddBezier(&D2D1_BEZIER_SEGMENT {
        point1: pt(xlate(cx, cy, (len * 0.34, -len * 0.10), ang)),
        point2: pt(xlate(cx, cy, (len * 0.30, len * 0.30), ang)),
        point3: pt(xlate(cx, cy, tail, ang)),
    });
    sink.AddBezier(&D2D1_BEZIER_SEGMENT {
        point1: pt(xlate(cx, cy, (-len * 0.30, len * 0.30), ang)),
        point2: pt(xlate(cx, cy, (-len * 0.34, -len * 0.10), ang)),
        point3: pt(xlate(cx, cy, tip, ang)),
    });
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    let _ = sink.Close();
    if let Ok(geom) = path.cast::<ID2D1Geometry>() {
        let _ = target.FillGeometry(Some(&geom), Some(brush), None);
    }
}

// ---------------------------------------------------------------------------
// Animal scenes (bundled SVG sprites, trajectories computed here)
// ---------------------------------------------------------------------------

/// Blit one sprite centred at (cx, cy) with the given long-side size.
/// Returns false when the sprite could not be rendered.
unsafe fn blit(
    target: &ID2D1HwndRenderTarget,
    sprite: Option<(u32, u32, Vec<u8>)>,
    cx: f32,
    cy: f32,
    size: f32,
) -> bool {
    let Some((ow, oh, buf)) = sprite else { return false };
    if ow == 0 || oh == 0 {
        return false;
    }
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
    let Ok(bmp) = target.CreateBitmap(
        sz,
        Some(buf.as_ptr() as *const core::ffi::c_void),
        ow * 4,
        &props,
    ) else {
        return false;
    };
    let aspect = oh as f32 / ow as f32;
    let (dw, dh) = if aspect <= 1.0 {
        (size, size * aspect)
    } else {
        (size / aspect, size)
    };
    let dest = D2D_RECT_F {
        left: cx - dw / 2.0,
        top: cy - dh / 2.0,
        right: cx + dw / 2.0,
        bottom: cy + dh / 2.0,
    };
    let _ = target.DrawBitmap(
        Some(&bmp),
        Some(&dest),
        1.0,
        D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
        None,
    );
    true
}

/// Draw an animal scene. Returns false if any sprite failed, so the
/// caller can fall back to a particle scene.
unsafe fn animals(
    target: &ID2D1HwndRenderTarget,
    scene: LockScene,
    t: f32,
    w: f32,
    h: f32,
    short: f32,
) -> bool {
    let mut ok = true;
    match scene {
        LockScene::CatChasingButterfly => {
            // Cat sits lower-left of centre; the butterfly loops above it.
            ok &= blit(
                target,
                crate::animal_scene::render_sprite(scene, (short * 0.17) as u32),
                w * 0.30,
                h * 0.76,
                short * 0.17,
            );
            let ang = t * 0.9;
            let bx = w * 0.30 + ang.cos() * short * 0.10;
            let by = h * 0.57 + ang.sin() * short * 0.045;
            ok &= blit(
                target,
                crate::animal_scene::render_butterfly((short * 0.055) as u32),
                bx,
                by,
                short * 0.055,
            );
        }
        LockScene::Rabbits => {
            // Big rabbit leading, small one trailing, both hopping.
            for (trail, size, ground) in [(0.0f32, 0.115f32, 0.80f32), (0.055, 0.085, 0.835)] {
                let p = frac(t / 11.0 - trail);
                let x = -0.08 * w + p * 1.16 * w;
                let hop = (p * PI * 9.0).sin().abs() * short * 0.05;
                ok &= blit(
                    target,
                    crate::animal_scene::render_sprite(scene, (short * size) as u32),
                    x,
                    h * ground - hop,
                    short * size,
                );
            }
        }
        LockScene::Birds => {
            for i in 0..4 {
                let p = frac(t / 15.0 - i as f32 * 0.03);
                let x = -0.06 * w + p * 1.12 * w;
                let y = h * (0.15 + 0.05 * sp(i, 811)) + (t * 2.2 + i as f32 * 1.3).sin() * 6.0;
                ok &= blit(
                    target,
                    crate::animal_scene::render_sprite(scene, (short * 0.06) as u32),
                    x,
                    y,
                    short * 0.06,
                );
            }
        }
        LockScene::Owl => {
            // A simple branch for the owl to sit on.
            if let Some(b) = solid_brush(target, 146, 104, 63, 0.9) {
                line(target, w * 0.60, h * 0.815, w * 0.96, h * 0.785, &b, short * 0.012);
            }
            let sway = (t * 0.8).sin() * 4.0;
            ok &= blit(
                target,
                crate::animal_scene::render_sprite(scene, (short * 0.16) as u32),
                w * 0.78 + sway,
                h * 0.70,
                short * 0.16,
            );
        }
        LockScene::Penguins => {
            for i in 0..3 {
                let p = frac(t / 17.0 - i as f32 * 0.045);
                let x = -0.08 * w + p * 1.16 * w;
                let y = h * 0.80 - (t * 2.4 + i as f32 * 2.1).sin().abs() * 3.0;
                ok &= blit(
                    target,
                    crate::animal_scene::render_sprite(scene, (short * 0.105) as u32),
                    x,
                    y,
                    short * 0.105,
                );
            }
        }
        LockScene::Panda => {
            let sway = (t * 0.7).sin() * 3.0;
            ok &= blit(
                target,
                crate::animal_scene::render_sprite(scene, (short * 0.18) as u32),
                w * 0.24 + sway,
                h * 0.72,
                short * 0.18,
            );
        }
        LockScene::Ducks => {
            for i in 0..3 {
                let p = frac(t / 17.0 - i as f32 * 0.05);
                let x = -0.08 * w + p * 1.16 * w;
                let y = h * 0.82;
                let size = short * 0.095;
                // Expanding ripple under each duck.
                let rp = frac(t / 1.8 + i as f32 * 0.33);
                if let Some(b) = solid_brush(target, 125, 211, 252, (1.0 - rp) * 0.4) {
                    stroke_ellipse(
                        target,
                        x - size * 0.35,
                        y + size * 0.34,
                        size * (0.30 + 0.55 * rp),
                        size * (0.30 + 0.55 * rp) * 0.26,
                        &b,
                        1.5,
                    );
                }
                ok &= blit(
                    target,
                    crate::animal_scene::render_sprite(scene, size as u32),
                    x,
                    y,
                    size,
                );
            }
        }
        _ => return false,
    }
    ok
}
