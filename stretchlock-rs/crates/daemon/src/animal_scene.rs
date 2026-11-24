//! Bundled animal-scene sprites: hand-written flat SVGs rendered through
//! resvg — the same pipeline as the mascot SVG sets (`mascot::render_selected`).
//!
//! Sprites are single animals; multi-animal scenes reuse one sprite at
//! several positions in `scene_draw`. Rendered pixmaps are cached per
//! (sprite, size bucket) so the 25 fps paint loop does not re-rasterise
//! the same size every frame. A sprite that fails to parse or render
//! yields `None`, and the caller falls back to a particle scene.

#![cfg(windows)]

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use stretchlock_core::scene::LockScene;

/// All scene SVGs share this viewBox size.
const VIEWBOX: f32 = 48.0;
/// Rendered sizes are rounded up to this bucket so window resizes do not
/// thrash the cache.
const SIZE_BUCKET: u32 = 32;

const SCENE_SVGS: &[(LockScene, &str)] = &[
    (
        LockScene::CatChasingButterfly,
        include_str!("../../../assets/scenes/cat.svg"),
    ),
    (
        LockScene::Rabbits,
        include_str!("../../../assets/scenes/rabbit.svg"),
    ),
    (LockScene::Birds, include_str!("../../../assets/scenes/bird.svg")),
    (LockScene::Owl, include_str!("../../../assets/scenes/owl.svg")),
    (
        LockScene::Penguins,
        include_str!("../../../assets/scenes/penguin.svg"),
    ),
    (
        LockScene::Panda,
        include_str!("../../../assets/scenes/panda.svg"),
    ),
    (LockScene::Ducks, include_str!("../../../assets/scenes/duck.svg")),
];

const BUTTERFLY_SVG: &str = include_str!("../../../assets/scenes/butterfly.svg");

/// Cache key: sprite slot (scene index in `SCENE_SVGS`, or `BUTTERFLY_KEY`
/// for the butterfly) plus the size bucket.
type CacheKey = (usize, u32);
const BUTTERFLY_KEY: usize = usize::MAX;

static CACHE: Mutex<Option<HashMap<CacheKey, Arc<(u32, u32, Vec<u8>)>>>> = Mutex::new(None);

fn bucket(size: u32) -> u32 {
    size.clamp(SIZE_BUCKET, 512)
        .div_ceil(SIZE_BUCKET)
        .saturating_mul(SIZE_BUCKET)
}

fn render_cached(key: usize, svg: &str, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let size = bucket(size);
    let cache_key = (key, size);
    {
        let guard = CACHE.lock();
        if let Some(map) = guard.as_ref() {
            if let Some(hit) = map.get(&cache_key) {
                let (w, h, pixels) = &**hit;
                return Some((*w, *h, pixels.clone()));
            }
        }
    }
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).ok()?;
    let scale = size as f32 / VIEWBOX;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let rendered = (size, size, pixmap.data().to_vec());
    let mut guard = CACHE.lock();
    guard
        .get_or_insert_with(HashMap::new)
        .insert(cache_key, Arc::new(rendered.clone()));
    Some(rendered)
}

/// Render the sprite for an animal scene at (about) `size` px square.
/// Returns premultiplied RGBA, like the mascot Lottie renderer.
pub fn render_sprite(scene: LockScene, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let (idx, svg) = SCENE_SVGS
        .iter()
        .enumerate()
        .find(|(_, (s, _))| *s == scene)
        .map(|(i, (_, svg))| (i, *svg))?;
    render_cached(idx, svg, size)
}

/// Render the butterfly that accompanies the cat scene.
pub fn render_butterfly(size: u32) -> Option<(u32, u32, Vec<u8>)> {
    render_cached(BUTTERFLY_KEY, BUTTERFLY_SVG, size)
}
