use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{value_str, value_u32, value_u64, write_atomic};

pub const STATS_FILENAME: &str = "stats.json";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct StatsData {
    pub breaks_today: u32,
    pub total_breaks: u64,
    pub current_streak_days: u32,
    pub last_break_date: String,
    /// Id of the most recently shown lock-screen scene (`scene::LockScene::id`).
    pub last_scene: String,
    /// Consecutive non-animal scene picks, for the picker's pity rule.
    pub non_animal_streak: u32,
}

impl StatsData {
    pub fn record_break(&mut self, today: &str) {
        if self.last_break_date != today {
            let yesterday = day_before(today);
            if self.last_break_date == yesterday.as_deref().unwrap_or("") {
                self.current_streak_days = self.current_streak_days.saturating_add(1);
            } else {
                self.current_streak_days = 1;
            }
            self.breaks_today = 0;
            self.last_break_date = today.to_string();
        }
        self.breaks_today = self.breaks_today.saturating_add(1);
        self.total_breaks = self.total_breaks.saturating_add(1);
    }

    pub fn rollover_for_read(&mut self, today: &str) {
        if self.last_break_date.is_empty() || self.last_break_date == today {
            return;
        }
        let yesterday = day_before(today);
        if self.last_break_date != yesterday.as_deref().unwrap_or("") {
            self.current_streak_days = 0;
        }
        self.breaks_today = 0;
    }
}

pub fn today_iso() -> String {
    chrono::Local::now().date_naive().to_string()
}

pub fn day_before(iso_date: &str) -> Option<String> {
    chrono::NaiveDate::parse_from_str(iso_date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.pred_opt())
        .map(|d| d.to_string())
}

#[derive(Deserialize, Default)]
struct RawStats {
    breaks_today: Option<Value>,
    total_breaks: Option<Value>,
    current_streak_days: Option<Value>,
    last_break_date: Option<Value>,
    last_scene: Option<Value>,
    non_animal_streak: Option<Value>,
}

impl From<RawStats> for StatsData {
    fn from(raw: RawStats) -> Self {
        StatsData {
            breaks_today: value_u32(raw.breaks_today.as_ref(), 0),
            total_breaks: value_u64(raw.total_breaks.as_ref(), 0),
            current_streak_days: value_u32(raw.current_streak_days.as_ref(), 0),
            last_break_date: value_str(raw.last_break_date.as_ref(), ""),
            last_scene: value_str(raw.last_scene.as_ref(), ""),
            non_animal_streak: value_u32(raw.non_animal_streak.as_ref(), 0),
        }
    }
}

pub struct Stats {
    path: PathBuf,
    data: StatsData,
}

impl Stats {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(STATS_FILENAME);
        let data = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<RawStats>(&text) {
                Ok(raw) => StatsData::from(raw),
                Err(err) => {
                    log::warn!("Could not read stats ({}); starting fresh", err);
                    StatsData::default()
                }
            },
            Err(_) => StatsData::default(),
        };
        Self { path, data }
    }

    pub fn data(&self) -> &StatsData {
        &self.data
    }

    pub fn record_break(&mut self) {
        let today = today_iso();
        self.data.record_break(&today);
        if let Err(err) = self.save() {
            log::warn!("Failed to save stats: {}", err);
        }
        log::info!(
            "Break recorded (today: {}, total: {}, streak: {})",
            self.data.breaks_today,
            self.data.total_breaks,
            self.data.current_streak_days
        );
    }

    /// Scene-picker state persisted for the lock-screen scene guarantee:
    /// the last scene's id ("" when none yet) and the current
    /// consecutive non-animal pick count.
    pub fn scene_state(&self) -> (&str, u32) {
        (&self.data.last_scene, self.data.non_animal_streak)
    }

    /// Record a freshly picked scene and save immediately, so the
    /// no-repeat / animal-pity guarantee survives a daemon restart.
    pub fn set_scene_state(&mut self, last_scene: &str, non_animal_streak: u32) {
        self.data.last_scene = last_scene.to_string();
        self.data.non_animal_streak = non_animal_streak;
        if let Err(err) = self.save() {
            log::warn!("Failed to save scene state: {}", err);
        }
    }

    pub fn get_summary(&mut self) -> (u32, u64, u32) {        let today = today_iso();
        self.data.rollover_for_read(&today);
        (
            self.data.breaks_today,
            self.data.total_breaks,
            self.data.current_streak_days,
        )
    }

    pub fn save(&self) -> io::Result<()> {
        let json = serde_json::to_string_pretty(&self.data)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        write_atomic(&self.path, &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    fn data_with(last: &str, today: u32, total: u64, streak: u32) -> StatsData {
        StatsData {
            breaks_today: today,
            total_breaks: total,
            current_streak_days: streak,
            last_break_date: last.to_string(),
            ..StatsData::default()
        }
    }

    #[test]
    fn first_break_starts_streak() {
        let mut d = StatsData::default();
        d.record_break("2026-01-10");
        assert_eq!((d.breaks_today, d.total_breaks, d.current_streak_days), (1, 1, 1));
        assert_eq!(d.last_break_date, "2026-01-10");
    }

    #[test]
    fn second_break_same_day_keeps_streak() {
        let mut d = StatsData::default();
        d.record_break("2026-01-10");
        d.record_break("2026-01-10");
        assert_eq!((d.breaks_today, d.total_breaks, d.current_streak_days), (2, 2, 1));
    }

    #[test]
    fn consecutive_day_increments_streak() {
        let mut d = data_with("2026-01-10", 3, 3, 1);
        d.record_break("2026-01-11");
        assert_eq!((d.breaks_today, d.total_breaks, d.current_streak_days), (1, 4, 2));
    }

    #[test]
    fn gap_day_resets_streak_to_one() {
        let mut d = data_with("2026-01-10", 3, 10, 4);
        d.record_break("2026-01-13");
        assert_eq!((d.breaks_today, d.total_breaks, d.current_streak_days), (1, 11, 1));
    }

    #[test]
    fn read_rollover_gap_resets_streak() {
        let mut d = data_with("2026-01-08", 3, 10, 4);
        d.rollover_for_read("2026-01-10");
        assert_eq!((d.breaks_today, d.current_streak_days), (0, 0));
        assert_eq!(d.total_breaks, 10);
    }

    #[test]
    fn read_rollover_yesterday_keeps_streak() {
        let mut d = data_with("2026-01-09", 3, 10, 4);
        d.rollover_for_read("2026-01-10");
        assert_eq!((d.breaks_today, d.current_streak_days), (0, 4));
    }

    #[test]
    fn read_rollover_same_day_is_noop() {
        let mut d = data_with("2026-01-10", 3, 10, 4);
        d.rollover_for_read("2026-01-10");
        assert_eq!((d.breaks_today, d.current_streak_days), (3, 4));
    }

    #[test]
    fn day_before_handles_month_and_leap_boundaries() {
        assert_eq!(day_before("2026-01-01").as_deref(), Some("2025-12-31"));
        assert_eq!(day_before("2026-03-01").as_deref(), Some("2026-02-28"));
        assert_eq!(day_before("2024-03-01").as_deref(), Some("2024-02-29"));
        assert_eq!(day_before("garbage"), None);
    }

    #[test]
    fn stats_file_round_trip() {
        let dir = temp_dir("stats-roundtrip");
        let mut stats = Stats::load(&dir);
        stats.record_break();
        assert_eq!(stats.get_summary().0, 1);
        let reloaded = Stats::load(&dir);
        assert_eq!(reloaded.data().breaks_today, 1);
        assert_eq!(reloaded.data().total_breaks, 1);
        assert_eq!(reloaded.data().last_break_date, today_iso());
    }

    #[test]
    fn corrupt_stats_file_starts_fresh() {
        let dir = temp_dir("stats-corrupt");
        fs::write(dir.join(STATS_FILENAME), "not json at all").unwrap();
        let mut stats = Stats::load(&dir);
        assert_eq!(stats.get_summary(), (0, 0, 0));
    }

    #[test]
    fn legacy_python_stats_file_loads() {
        let dir = temp_dir("stats-legacy");
        // 2026-01-20 距今远超一天，读时日翻滚会清 streak——Python 版 _maybe_reset_day 同语义；
        // 改用昨天日期验证 streak 保留路径。
        let yesterday = chrono::Local::now()
            .date_naive()
            .pred_opt()
            .unwrap()
            .to_string();
        fs::write(
            dir.join(STATS_FILENAME),
            format!(
                r#"{{"breaks_today": 2, "total_breaks": 40, "current_streak_days": 3, "last_break_date": "{yesterday}"}}"#
            ),
        )
        .unwrap();
        let mut stats = Stats::load(&dir);
        assert_eq!(stats.get_summary(), (0, 40, 3));
    }

    #[test]
    fn wrong_typed_stats_fields_use_defaults() {
        let dir = temp_dir("stats-tolerant");
        fs::write(
            dir.join(STATS_FILENAME),
            r#"{"breaks_today": "many", "total_breaks": -5, "current_streak_days": 3.7, "last_break_date": 42}"#,
        )
        .unwrap();
        let stats = Stats::load(&dir);
        assert_eq!(stats.data().breaks_today, 0);
        assert_eq!(stats.data().total_breaks, 0);
        assert_eq!(stats.data().current_streak_days, 3);
        assert_eq!(stats.data().last_break_date, "");
    }

    #[test]
    fn scene_state_round_trip() {
        let dir = temp_dir("stats-scene-roundtrip");
        let mut stats = Stats::load(&dir);
        assert_eq!(stats.scene_state(), ("", 0));
        stats.set_scene_state("owl", 0);
        stats.set_scene_state("rain", 2);
        assert_eq!(stats.scene_state(), ("rain", 2));
        let reloaded = Stats::load(&dir);
        assert_eq!(reloaded.data().last_scene, "rain");
        assert_eq!(reloaded.data().non_animal_streak, 2);
        // Other stats survive alongside the scene state.
        assert_eq!(reloaded.data().total_breaks, 0);
    }

    #[test]
    fn legacy_stats_without_scene_fields_loads() {
        let dir = temp_dir("stats-scene-legacy");
        fs::write(
            dir.join(STATS_FILENAME),
            r#"{"breaks_today": 1, "total_breaks": 9, "current_streak_days": 2, "last_break_date": "2026-01-10"}"#,
        )
        .unwrap();
        let stats = Stats::load(&dir);
        assert_eq!(stats.scene_state(), ("", 0));
        assert_eq!(stats.data().total_breaks, 9);
    }

    #[test]
    fn wrong_typed_scene_fields_use_defaults() {
        let dir = temp_dir("stats-scene-tolerant");
        fs::write(
            dir.join(STATS_FILENAME),
            r#"{"last_scene": 7, "non_animal_streak": "many"}"#,
        )
        .unwrap();
        let stats = Stats::load(&dir);
        assert_eq!(stats.scene_state(), ("", 0));
    }
}
