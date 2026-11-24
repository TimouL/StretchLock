//! Lock-screen background scenes: the 16 scenes from the web prototype
//! (9 ambient particle scenes + 7 animal scenes), the per-scene particle
//! parameters the daemon renderer consumes, and the scene picker.
//!
//! Picker rules (matching the prototype's "pity" behaviour):
//! - never the same scene twice in a row;
//! - after 2 consecutive non-animal picks, the next pick is forced to an
//!   animal scene, so an animal appears within at most 3 breaks.
//!
//! The picker state is persisted via `stats::StatsData` (`last_scene` +
//! `non_animal_streak`) so the guarantee survives daemon restarts.
//! Std-only: the random source is a wall-clock seed run through splitmix64.

/// One of the sixteen lock-screen scenes, in the same order as the
/// prototype: the 9 ambient scenes first, then the 7 animal scenes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LockScene {
    Fireflies,
    Stars,
    Bubbles,
    Fish,
    Plane,
    Rain,
    Notes,
    Moon,
    Leaves,
    CatChasingButterfly,
    Rabbits,
    Birds,
    Owl,
    Penguins,
    Panda,
    Ducks,
}

/// All sixteen scenes, in prototype order.
pub const ALL_SCENES: [LockScene; 16] = [
    LockScene::Fireflies,
    LockScene::Stars,
    LockScene::Bubbles,
    LockScene::Fish,
    LockScene::Plane,
    LockScene::Rain,
    LockScene::Notes,
    LockScene::Moon,
    LockScene::Leaves,
    LockScene::CatChasingButterfly,
    LockScene::Rabbits,
    LockScene::Birds,
    LockScene::Owl,
    LockScene::Penguins,
    LockScene::Panda,
    LockScene::Ducks,
];

/// The 7 animal scenes. Note `Fish` is *not* an animal scene for the
/// pity rule — in the prototype it belongs to the original ambient pool.
pub const ANIMAL_SCENES: [LockScene; 7] = [
    LockScene::CatChasingButterfly,
    LockScene::Rabbits,
    LockScene::Birds,
    LockScene::Owl,
    LockScene::Penguins,
    LockScene::Panda,
    LockScene::Ducks,
];

impl LockScene {
    /// Display name shown on the lock screen (prototype text).
    pub fn name(self) -> &'static str {
        match self {
            LockScene::Fireflies => "萤火虫之夜",
            LockScene::Stars => "星星眨眼",
            LockScene::Bubbles => "气泡上升",
            LockScene::Fish => "小鱼巡游",
            LockScene::Plane => "纸飞机放飞",
            LockScene::Rain => "夜雨涟漪",
            LockScene::Notes => "音符飘飘",
            LockScene::Moon => "月亮散步",
            LockScene::Leaves => "落叶轻舞",
            LockScene::CatChasingButterfly => "小猫追蝴蝶",
            LockScene::Rabbits => "小兔蹦蹦跳",
            LockScene::Birds => "小鸟过天空",
            LockScene::Owl => "猫头鹰守夜",
            LockScene::Penguins => "企鹅散步",
            LockScene::Panda => "熊猫啃竹子",
            LockScene::Ducks => "小鸭子划水",
        }
    }

    /// One-line hint shown with the name (prototype text).
    pub fn hint(self) -> &'static str {
        match self {
            LockScene::Fireflies => "点点微光，慢慢飘",
            LockScene::Stars => "抬头看看，星星在眨眼",
            LockScene::Bubbles => "跟着气泡，轻轻吐一口气",
            LockScene::Fish => "小鱼游过，看着就好",
            LockScene::Plane => "把烦恼折成飞机放走",
            LockScene::Rain => "听雨落，看涟漪散开",
            LockScene::Notes => "在心里哼一段小调",
            LockScene::Moon => "云朵陪着月亮慢慢走",
            LockScene::Leaves => "叶子落下来，肩膀也放下来",
            LockScene::CatChasingButterfly => "猫猫扑蝴蝶，差一点就抓到啦",
            LockScene::Rabbits => "两只兔子蹦蹦跳跳跑过去了",
            LockScene::Birds => "小鸟结伴飞过天空",
            LockScene::Owl => "猫头鹰在树枝上守夜，眼睛一眨一眨",
            LockScene::Penguins => "企鹅排队摇摇摆摆走过",
            LockScene::Panda => "熊猫坐着啃竹子，慢慢摇",
            LockScene::Ducks => "小鸭子排队划水，身后泛起涟漪",
        }
    }

    /// Stable identifier used for persistence in `stats.json`.
    pub fn id(self) -> &'static str {
        match self {
            LockScene::Fireflies => "fireflies",
            LockScene::Stars => "stars",
            LockScene::Bubbles => "bubbles",
            LockScene::Fish => "fish",
            LockScene::Plane => "plane",
            LockScene::Rain => "rain",
            LockScene::Notes => "notes",
            LockScene::Moon => "moon",
            LockScene::Leaves => "leaves",
            LockScene::CatChasingButterfly => "cat_butterfly",
            LockScene::Rabbits => "rabbits",
            LockScene::Birds => "birds",
            LockScene::Owl => "owl",
            LockScene::Penguins => "penguins",
            LockScene::Panda => "panda",
            LockScene::Ducks => "ducks",
        }
    }

    /// Inverse of [`LockScene::id`]; `None` for unknown/empty ids.
    pub fn from_id(id: &str) -> Option<LockScene> {
        ALL_SCENES.iter().copied().find(|s| s.id() == id)
    }

    /// True only for the 7 animal scenes (the pity-rule pool).
    pub fn is_animal(self) -> bool {
        ANIMAL_SCENES.contains(&self)
    }

    /// Particle / element parameters, taken from the prototype.
    pub fn spec(self) -> SceneSpec {
        match self {
            LockScene::Fireflies => SceneSpec {
                count: 14,
                size_px: (4.0, 8.0),
                duration_s: (7.0, 13.0),
                motion: Motion::Drift,
                detail: "glow #dcffb2; wander with fade in/out, lower 55% of the screen",
            },
            LockScene::Stars => SceneSpec {
                count: 26,
                size_px: (2.0, 5.2),
                duration_s: (2.2, 4.6),
                motion: Motion::Twinkle,
                detail: "white, opacity 0.12↔1, top 56% of the screen",
            },
            LockScene::Bubbles => SceneSpec {
                count: 12,
                size_px: (10.0, 36.0),
                duration_s: (8.5, 14.5),
                motion: Motion::Rise,
                detail: "white ring, rise from below the screen with a sideways sway",
            },
            LockScene::Fish => SceneSpec {
                count: 3,
                size_px: (38.0, 56.0),
                duration_s: (24.0, 31.0),
                motion: Motion::Swim,
                detail: "swim left→right across the screen, gentle vertical bob",
            },
            LockScene::Plane => SceneSpec {
                count: 1,
                size_px: (52.0, 52.0),
                duration_s: (15.0, 15.0),
                motion: Motion::FlyPath,
                detail: "flies left→right near the top, tilting",
            },
            LockScene::Rain => SceneSpec {
                count: 18,
                size_px: (42.0, 80.0),
                duration_s: (1.15, 1.95),
                motion: Motion::Fall,
                detail: "plus 4 ripple rings at the bottom, staggered",
            },
            LockScene::Notes => SceneSpec {
                count: 8,
                size_px: (20.0, 36.0),
                duration_s: (8.0, 13.0),
                motion: Motion::Float,
                detail: "glyphs ♫/♪/♩, rise while swaying",
            },
            LockScene::Moon => SceneSpec {
                count: 4, // 1 moon + 3 clouds
                size_px: (72.0, 170.0),
                duration_s: (7.0, 64.0),
                motion: Motion::CloudDrift,
                detail: "moon sways in place; clouds drift left→right, low opacity",
            },
            LockScene::Leaves => SceneSpec {
                count: 10,
                size_px: (11.0, 21.0),
                duration_s: (10.0, 17.0),
                motion: Motion::LeafFall,
                detail: "green/orange alternating, sway and rotate while falling",
            },
            LockScene::CatChasingButterfly => SceneSpec {
                count: 2, // 1 cat + 1 butterfly
                size_px: (28.0, 64.0),
                duration_s: (6.0, 10.0),
                motion: Motion::Chase,
                detail: "butterfly circles; cat watches from below",
            },
            LockScene::Rabbits => SceneSpec {
                count: 2,
                size_px: (36.0, 56.0),
                duration_s: (9.0, 13.0),
                motion: Motion::Hop,
                detail: "hop left→right in arcs, small rabbit trailing",
            },
            LockScene::Birds => SceneSpec {
                count: 4,
                size_px: (28.0, 44.0),
                duration_s: (12.0, 18.0),
                motion: Motion::FlockFly,
                detail: "fly left→right in a loose group, slight bob",
            },
            LockScene::Owl => SceneSpec {
                count: 1,
                size_px: (64.0, 64.0),
                duration_s: (5.0, 8.0),
                motion: Motion::PerchSway,
                detail: "perched on a branch, sways gently",
            },
            LockScene::Penguins => SceneSpec {
                count: 3,
                size_px: (36.0, 52.0),
                duration_s: (14.0, 20.0),
                motion: Motion::Waddle,
                detail: "waddle left→right in a queue, rocking side to side",
            },
            LockScene::Panda => SceneSpec {
                count: 1,
                size_px: (72.0, 72.0),
                duration_s: (6.0, 9.0),
                motion: Motion::PerchSway,
                detail: "sitting, sways gently, bamboo in arms",
            },
            LockScene::Ducks => SceneSpec {
                count: 3,
                size_px: (36.0, 52.0),
                duration_s: (14.0, 20.0),
                motion: Motion::Paddle,
                detail: "paddle left→right in a queue, ripple rings trailing",
            },
        }
    }
}

/// How a scene's elements move (mirrors the CSS keyframes in the prototype).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Twinkle,
    Drift,
    Rise,
    Swim,
    FlyPath,
    Fall,
    Float,
    CloudDrift,
    LeafFall,
    Hop,
    FlockFly,
    PerchSway,
    Waddle,
    Paddle,
    Chase,
}

/// Per-scene parameters for the renderer.
#[derive(Clone, Copy, Debug)]
pub struct SceneSpec {
    /// Number of elements to draw.
    pub count: usize,
    /// Element size range in px (diameter / long side).
    pub size_px: (f32, f32),
    /// Per-element animation-cycle range in seconds.
    pub duration_s: (f32, f32),
    pub motion: Motion,
    /// Placement / colour notes from the prototype.
    pub detail: &'static str,
}

/// Deterministic 0..1 spread so decorative elements never clump.
/// Same formula as the prototype's `spread(seed)`.
pub fn spread(seed: u32) -> f32 {
    let x = (seed as f32 * 999.7).sin() * 10_000.0;
    x - x.floor()
}

/// A std-only random value: wall-clock nanoseconds run through splitmix64.
pub fn random_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15);
    splitmix64(nanos)
}

/// One splitmix64 round: decorrelates nearby clock values well enough
/// for a decorative scene picker.
pub fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Stateful scene picker implementing the no-repeat + animal-pity rules.
/// Persist `last()` / `non_animal_streak()` (via stats) and rebuild with
/// [`ScenePicker::from_parts`] on startup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScenePicker {
    last: Option<LockScene>,
    non_animal_streak: u32,
}

impl ScenePicker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_parts(last: Option<LockScene>, non_animal_streak: u32) -> Self {
        Self {
            last,
            non_animal_streak,
        }
    }

    pub fn last(&self) -> Option<LockScene> {
        self.last
    }

    pub fn non_animal_streak(&self) -> u32 {
        self.non_animal_streak
    }

    /// Pick the next scene using a caller-supplied random value (handy
    /// for tests). Never returns `last()`; after 2 consecutive
    /// non-animal picks the pool is restricted to animal scenes.
    pub fn pick_next(&mut self, random: u64) -> LockScene {
        let pool: &[LockScene] = if self.non_animal_streak >= 2 {
            &ANIMAL_SCENES
        } else {
            &ALL_SCENES
        };
        let candidates: Vec<LockScene> = pool
            .iter()
            .copied()
            .filter(|s| Some(*s) != self.last)
            .collect();
        let next = candidates[(random as usize) % candidates.len()];
        self.last = Some(next);
        if next.is_animal() {
            self.non_animal_streak = 0;
        } else {
            self.non_animal_streak = self.non_animal_streak.saturating_add(1);
        }
        next
    }

    /// Pick the next scene using the wall-clock random source.
    pub fn pick(&mut self) -> LockScene {
        self.pick_next(random_seed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift64 for tests.
    fn rng(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    #[test]
    fn never_repeats_in_a_row() {
        let mut picker = ScenePicker::new();
        let mut seed = 12345u64;
        let mut prev: Option<LockScene> = None;
        for _ in 0..1_000 {
            let scene = picker.pick_next(rng(&mut seed));
            assert_ne!(Some(scene), prev);
            prev = Some(scene);
        }
    }

    #[test]
    fn animal_within_any_three_picks() {
        let mut picker = ScenePicker::new();
        let mut seed = 424_242u64;
        let picks: Vec<LockScene> = (0..1_000).map(|_| picker.pick_next(rng(&mut seed))).collect();
        for window in picks.windows(3) {
            assert!(
                window.iter().any(|s| s.is_animal()),
                "no animal in {window:?}"
            );
        }
    }

    #[test]
    fn all_sixteen_scenes_are_reachable() {
        let mut picker = ScenePicker::new();
        let mut seed = 987_654_321u64;
        let mut seen = [false; 16];
        for _ in 0..2_000 {
            let scene = picker.pick_next(rng(&mut seed));
            seen[ALL_SCENES.iter().position(|s| *s == scene).unwrap()] = true;
        }
        assert!(seen.iter().all(|s| *s), "unseen: {seen:?}");
    }

    #[test]
    fn animal_set_is_exactly_the_seven() {
        let animals: Vec<LockScene> = ALL_SCENES.iter().copied().filter(|s| s.is_animal()).collect();
        assert_eq!(animals, ANIMAL_SCENES.to_vec());
        assert_eq!(animals.len(), 7);
        // Fish swims in the ambient pool for the pity rule.
        assert!(!LockScene::Fish.is_animal());
    }

    #[test]
    fn pity_forces_animal_after_two_non_animals() {
        let mut picker = ScenePicker::from_parts(Some(LockScene::Fireflies), 2);
        // Any random value must land in the animal pool now.
        for r in [0u64, 1, 5, 123_456_789] {
            let mut p = picker.clone();
            let scene = p.pick_next(r);
            assert!(scene.is_animal(), "random {r} gave {scene:?}");
            assert_eq!(p.non_animal_streak(), 0);
        }
        // And the previous scene is still excluded from the forced pool.
        let mut p = ScenePicker::from_parts(Some(LockScene::Owl), 3);
        for r in 0..7u64 {
            assert_ne!(p.clone().pick_next(r), LockScene::Owl);
        }
        let _ = picker.pick_next(0);
    }

    #[test]
    fn streak_counts_non_animals() {
        // random = 0 picks the first candidate: with no previous scene
        // that is Fireflies (non-animal).
        let mut picker = ScenePicker::new();
        let first = picker.pick_next(0);
        assert_eq!(first, LockScene::Fireflies);
        assert_eq!(picker.non_animal_streak(), 1);
        assert_eq!(picker.last(), Some(LockScene::Fireflies));
    }

    #[test]
    fn names_match_prototype() {
        let expected = [
            "萤火虫之夜", "星星眨眼", "气泡上升", "小鱼巡游", "纸飞机放飞", "夜雨涟漪",
            "音符飘飘", "月亮散步", "落叶轻舞", "小猫追蝴蝶", "小兔蹦蹦跳", "小鸟过天空",
            "猫头鹰守夜", "企鹅散步", "熊猫啃竹子", "小鸭子划水",
        ];
        assert_eq!(ALL_SCENES.len(), 16);
        for (scene, name) in ALL_SCENES.iter().zip(expected.iter()) {
            assert_eq!(scene.name(), *name);
        }
    }

    #[test]
    fn id_round_trip() {
        for scene in ALL_SCENES {
            assert_eq!(LockScene::from_id(scene.id()), Some(scene));
        }
        assert_eq!(LockScene::from_id(""), None);
        assert_eq!(LockScene::from_id("nope"), None);
    }

    #[test]
    fn spread_stays_in_unit_range() {
        for seed in 0..1_000 {
            let v = spread(seed);
            assert!((0.0..1.0).contains(&v), "seed {seed} gave {v}");
        }
    }

    #[test]
    fn spec_counts_match_checklist() {
        assert_eq!(LockScene::Fireflies.spec().count, 14);
        assert_eq!(LockScene::Stars.spec().count, 26);
        assert_eq!(LockScene::Bubbles.spec().count, 12);
        assert_eq!(LockScene::Fish.spec().count, 3);
        assert_eq!(LockScene::Rain.spec().count, 18);
        assert_eq!(LockScene::Notes.spec().count, 8);
        assert_eq!(LockScene::Leaves.spec().count, 10);
        assert_eq!(LockScene::Rabbits.spec().count, 2);
        assert_eq!(LockScene::Birds.spec().count, 4);
        assert_eq!(LockScene::Penguins.spec().count, 3);
        assert_eq!(LockScene::Ducks.spec().count, 3);
    }
}
