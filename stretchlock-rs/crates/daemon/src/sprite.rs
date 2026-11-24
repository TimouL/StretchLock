//! Shared sprite frame loading for the mascot and the lock-screen
//! animal scenes: animated GIFs (per-frame delays) and PNG sequences,
//! normalised to trimmed, downscaled BGRA frames.

#![cfg(windows)]

use std::path::PathBuf;

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>, // 0xAABBGGRR
}


pub const MAX_SIDE: u32 = 512;

pub fn load_gif(path: &PathBuf) -> Option<(Vec<Frame>, Vec<u32>)> {
    let bytes = std::fs::read(path).ok()?;
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(std::io::Cursor::new(bytes)).ok()?;

    let width = decoder.width() as u32;
    let height = decoder.height() as u32;
    let mut canvas = vec![0u32; (width * height) as usize];
    let mut frames = Vec::new();
    let mut delays = Vec::new();

    while let Some(frame) = decoder.read_next_frame().ok()? {
        let delay = (frame.delay.max(2) * 10) as u32;
        let fw = frame.width as u32;
        let fh = frame.height as u32;
        let top = frame.top.max(0) as u32;
        let left = frame.left.max(0) as u32;

        for y in 0..fh {
            for x in 0..fw {
                let si = ((y * fw + x) * 4) as usize;
                let r = frame.buffer[si] as u32;
                let g = frame.buffer[si + 1] as u32;
                let b = frame.buffer[si + 2] as u32;
                let a = frame.buffer[si + 3] as u32;
                let dx = left + x;
                let dy = top + y;
                if dx >= width || dy >= height { continue; }
                let di = (dy * width + dx) as usize;
                if frame.dispose == gif::DisposalMethod::Background {
                    canvas[di] = 0;
                    continue;
                }
                if a == 0 { continue; }
                let dst = canvas[di];
                let alpha = a as f32 / 255.0;
                let mix = |s: u32, d: u32| -> u32 { (s as f32 * alpha + d as f32 * (1.0 - alpha)).round() as u32 };
                let rr = mix(r, (dst >> 16) & 0xFF);
                let gg = mix(g, (dst >> 8) & 0xFF);
                let bb = mix(b, dst & 0xFF);
                canvas[di] = 0xFF00_0000 | (rr << 16) | (gg << 8) | bb;
            }
        }
        let (w, h, pixels) = trim_and_scale(&canvas, width, height);
        frames.push(Frame { width: w, height: h, pixels });
        delays.push(delay);
    }
    Some((frames, delays))
}

pub fn load_png_sequence(dir: &PathBuf) -> Vec<Frame> {
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries.flatten().map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("png")).unwrap_or(false))
            .collect(),
        Err(_) => return Vec::new(),
    };
    paths.sort();
    paths.iter().filter_map(|p| std::fs::read(p).ok().and_then(|b| decode_png(&b))).collect()
}

pub fn decode_png(bytes: &[u8]) -> Option<Frame> {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => 4,
    };
    let (width, height) = (info.width, info.height);
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for i in 0..(width * height) as usize {
        let si = i * channels;
        let (r, g, b, a) = match channels {
            4 => (buf[si], buf[si+1], buf[si+2], buf[si+3]),
            3 => (buf[si], buf[si+1], buf[si+2], 255),
            1 => (buf[si], buf[si], buf[si], 255),
            _ => (buf[si], buf[si], buf[si], buf[si+1]),
        };
        pixels.push(((a as u32) << 24) | ((b as u32) << 16) | ((g as u32) << 8) | r as u32);
    }
    let (w, h, pixels) = trim_and_scale(&pixels, width, height);
    Some(Frame { width: w, height: h, pixels })
}

pub fn trim_and_scale(pixels: &[u32], width: u32, height: u32) -> (u32, u32, Vec<u32>) {
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    for y in 0..height {
        for x in 0..width {
            if (pixels[(y * width + x) as usize] >> 24) & 0xFF > 8 {
                min_x = min_x.min(x); min_y = min_y.min(y);
                max_x = max_x.max(x); max_y = max_y.max(y);
            }
        }
    }
    if max_x < min_x || max_y < min_y { return (width, height, pixels.to_vec()); }
    let crop_w = max_x - min_x + 1;
    let crop_h = max_y - min_y + 1;
    let mut cropped = vec![0u32; (crop_w * crop_h) as usize];
    for y in 0..crop_h {
        for x in 0..crop_w {
            cropped[(y * crop_w + x) as usize] = pixels[((min_y + y) * width + (min_x + x)) as usize];
        }
    }
    let longest = crop_w.max(crop_h);
    if longest <= MAX_SIDE { return (crop_w, crop_h, cropped); }
    let factor = MAX_SIDE as f32 / longest as f32;
    let (nw, nh) = ((crop_w as f32 * factor).max(1.0) as u32, (crop_h as f32 * factor).max(1.0) as u32);
    let mut scaled = vec![0u32; (nw * nh) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let sx0 = (x as f32 / factor) as u32;
            let sy0 = (y as f32 / factor) as u32;
            let sx1 = ((x as f32 + 1.0) / factor).ceil() as u32;
            let sy1 = ((y as f32 + 1.0) / factor).ceil() as u32;
            let mut r=0u32; let mut g=0u32; let mut b=0u32; let mut a=0u32; let mut n=0u32;
            for sy in sy0..sy1.min(crop_h) {
                for sx in sx0..sx1.min(crop_w) {
                    let p = cropped[(sy * crop_w + sx) as usize];
                    r += (p >> 16) & 0xFF; g += (p >> 8) & 0xFF; b += p & 0xFF; a += (p >> 24) & 0xFF; n += 1;
                }
            }
            if n > 0 { scaled[(y * nw + x) as usize] = (a/n)<<24 | (r/n)<<16 | (g/n)<<8 | (b/n); }
        }
    }
    (nw, nh, scaled)
}

