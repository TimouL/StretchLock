use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::passphrase::{self, PassphraseHash, DEFAULT_PASSPHRASE};

pub const CONFIG_FILENAME: &str = "config.json";
pub const DEFAULT_WORK_INTERVAL_MINUTES: u32 = 60;
pub const DEFAULT_BREAK_DURATION_MINUTES: u32 = 5;
pub const MIN_WORK_INTERVAL: u32 = 1;
pub const MAX_WORK_INTERVAL: u32 = 480;
pub const MIN_BREAK_DURATION: u32 = 1;
pub const MAX_BREAK_DURATION: u32 = 60;
pub const DEFAULT_LOG_LEVEL: &str = "INFO";
/// Upper bounds for the customizable break messages.
pub const MAX_MESSAGES: usize = 40;
pub const MAX_MESSAGE_CHARS: usize = 80;
pub const VALID_LOG_LEVELS: [&str; 5] = ["DEBUG", "INFO", "WARNING", "ERROR", "OFF"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config {
    #[serde(default = "default_work_interval")]
    pub work_interval_minutes: u32,
    #[serde(default = "default_break_duration")]
    pub break_duration_minutes: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase_hash: Option<PassphraseHash>,
    #[serde(default = "default_true")]
    pub autostart_daemon: bool,
    #[serde(default = "default_true")]
    pub autostart_tray: bool,
    /// Playful break messages; one is picked per break.
    #[serde(default = "default_messages")]
    pub messages: Vec<String>,
    /// Animate the cartoon mascot on the overlay.
    #[serde(default = "default_true")]
    pub animation_enabled: bool,
    /// Which bundled mascot to use (cat/dog/dino).
    #[serde(default = "default_mascot")]
    pub mascot: String,
}

fn default_mascot() -> String {
    "cat".to_string()
}

/// Cute, non-preachy reminder lines shown in the overlay speech bubble.
pub fn default_messages() -> Vec<String> {
    [
        "哎呀，时间到啦！眼睛辛苦这么久，起来走两步吧～",
        "嘿！该伸个懒腰啦，把手臂举高高～",
        "眼睛说它想休息一下，你也陪陪它吧～",
        "站起来扭扭腰，喝口水，再回来继续战斗！",
        "肩膀是不是有点酸？转一转，放松一下～",
        "久坐伤身，起来蹦两下！",
        "深呼吸三次，让脑子也透透气～",
        "别看我，看窗外！远处那个点，盯它十秒～",
    ]
    .iter()
    .map(|line| line.to_string())
    .collect()
}



fn default_work_interval() -> u32 {
    DEFAULT_WORK_INTERVAL_MINUTES
}

fn default_break_duration() -> u32 {
    DEFAULT_BREAK_DURATION_MINUTES
}

fn default_true() -> bool {
    true
}

fn default_log_level() -> String {
    DEFAULT_LOG_LEVEL.to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            work_interval_minutes: DEFAULT_WORK_INTERVAL_MINUTES,
            break_duration_minutes: DEFAULT_BREAK_DURATION_MINUTES,
            enabled: true,
            log_level: DEFAULT_LOG_LEVEL.to_string(),
            passphrase_hash: Some(PassphraseHash::generate(DEFAULT_PASSPHRASE)),
            autostart_daemon: true,
            autostart_tray: true,
            messages: default_messages(),
            animation_enabled: true,
            mascot: "cat".to_string(),
        }
    }
}

impl Config {
    pub fn validate(&mut self) {
        self.work_interval_minutes = self
            .work_interval_minutes
            .clamp(MIN_WORK_INTERVAL, MAX_WORK_INTERVAL);
        self.break_duration_minutes = self
            .break_duration_minutes
            .clamp(MIN_BREAK_DURATION, MAX_BREAK_DURATION);
        self.log_level = self.log_level.to_uppercase();
        if !VALID_LOG_LEVELS.contains(&self.log_level.as_str()) {
            self.log_level = DEFAULT_LOG_LEVEL.to_string();
        }
        self.messages = self
            .messages
            .iter()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .map(|line| line.chars().take(MAX_MESSAGE_CHARS).collect())
            .take(MAX_MESSAGES)
            .collect();
        if self.messages.is_empty() {
            self.messages = default_messages();
        }
        if !["cat", "dog", "dino"].contains(&self.mascot.as_str()) {
            self.mascot = "cat".to_string();
        }
    }

    /// Pick a message for this break, rotating through the list.
    pub fn message_for(&self, index: usize) -> &str {
        if self.messages.is_empty() {
            return "";
        }
        &self.messages[index % self.messages.len()]
    }

    pub fn set_passphrase(&mut self, plain: &str) {
        self.passphrase_hash = Some(PassphraseHash::generate(plain));
    }

    pub fn verify_passphrase(&self, plain: &str) -> bool {
        self.passphrase_hash
            .as_ref()
            .map(|hash| passphrase::verify(plain, hash))
            .unwrap_or(false)
    }
}

#[derive(Deserialize, Default)]
struct RawConfig {
    work_interval_minutes: Option<Value>,
    break_duration_minutes: Option<Value>,
    enabled: Option<Value>,
    log_level: Option<Value>,
    passphrase_hash: Option<Value>,
    #[serde(alias = "passphrase")]
    legacy_passphrase: Option<Value>,
    autostart_daemon: Option<Value>,
    autostart_tray: Option<Value>,
    messages: Option<Value>,
    animation_enabled: Option<Value>,
    mascot: Option<Value>,
}

impl From<RawConfig> for Config {
    fn from(raw: RawConfig) -> Self {
        let mut config = Config {
            work_interval_minutes: value_u32(
                raw.work_interval_minutes.as_ref(),
                DEFAULT_WORK_INTERVAL_MINUTES,
            ),
            break_duration_minutes: value_u32(
                raw.break_duration_minutes.as_ref(),
                DEFAULT_BREAK_DURATION_MINUTES,
            ),
            enabled: value_bool(raw.enabled.as_ref(), true),
            log_level: value_str(raw.log_level.as_ref(), DEFAULT_LOG_LEVEL),
            passphrase_hash: raw
                .passphrase_hash
                .as_ref()
                .and_then(|v| serde_json::from_value(v.clone()).ok()),
            autostart_daemon: value_bool(raw.autostart_daemon.as_ref(), true),
            autostart_tray: value_bool(raw.autostart_tray.as_ref(), true),
            messages: value_messages(raw.messages.as_ref()),
            animation_enabled: value_bool(raw.animation_enabled.as_ref(), true),
            mascot: value_str(raw.mascot.as_ref(), "cat"),
        };
        if config.passphrase_hash.is_none() {
            if raw.legacy_passphrase.is_some() {
                log::info!("Migrating legacy plaintext passphrase to PBKDF2 hash");
            }
            let legacy = raw
                .legacy_passphrase
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_PASSPHRASE);
            let plain = if legacy.is_empty() {
                DEFAULT_PASSPHRASE
            } else {
                legacy
            };
            config.passphrase_hash = Some(PassphraseHash::generate(plain));
        }
        config
    }
}

pub(crate) fn value_u32(value: Option<&Value>, default: u32) -> u32 {
    match value {
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                i.clamp(0, u32::MAX as i64) as u32
            } else {
                n.as_f64().map(|f| f.max(0.0) as u32).unwrap_or(default)
            }
        }
        _ => default,
    }
}

pub(crate) fn value_u64(value: Option<&Value>, default: u64) -> u64 {
    match value {
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                i.max(0) as u64
            } else {
                n.as_f64().map(|f| f.max(0.0) as u64).unwrap_or(default)
            }
        }
        _ => default,
    }
}

pub(crate) fn value_messages(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => {
            let lines: Vec<String> = items
                .iter()
                .filter_map(Value::as_str)
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty())
                .collect();
            if lines.is_empty() {
                default_messages()
            } else {
                lines
            }
        }
        _ => default_messages(),
    }
}

pub(crate) fn value_bool(value: Option<&Value>, default: bool) -> bool {
    value.and_then(Value::as_bool).unwrap_or(default)
}

pub(crate) fn value_str(value: Option<&Value>, default: &str) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

pub(crate) fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err)
        }
    }
}

pub fn get_config_dir() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("StretchLock");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub struct ConfigStore {
    pub dir: PathBuf,
}

impl Default for ConfigStore {
    fn default() -> Self {
        Self {
            dir: get_config_dir(),
        }
    }
}

impl ConfigStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn config_path(&self) -> PathBuf {
        self.dir.join(CONFIG_FILENAME)
    }

    pub fn stats_path(&self) -> PathBuf {
        self.dir.join(crate::stats::STATS_FILENAME)
    }

    pub fn log_path(&self) -> PathBuf {
        self.dir.join(crate::logging::LOG_FILENAME)
    }

    pub fn load(&self) -> Config {
        let path = self.config_path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                log::warn!(
                    "No config file found at {} ({}); using defaults",
                    path.display(),
                    err
                );
                return Config::default();
            }
        };
        match serde_json::from_str::<RawConfig>(&text) {
            Ok(raw) => {
                let mut config = Config::from(raw);
                config.validate();
                log::info!("Config loaded from {}", path.display());
                config
            }
            Err(err) => {
                log::warn!("Failed to read config ({}); using defaults", err);
                Config::default()
            }
        }
    }

    pub fn save(&self, config: &Config) -> io::Result<()> {
        let mut config = config.clone();
        config.validate();
        let json = serde_json::to_string_pretty(&config)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        write_atomic(&self.config_path(), &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn defaults_match_python_reference() {
        let config = Config::default();
        assert_eq!(config.work_interval_minutes, 60);
        assert_eq!(config.break_duration_minutes, 5);
        assert!(config.enabled);
        assert_eq!(config.log_level, "INFO");
        assert!(config.autostart_daemon);
        assert!(config.autostart_tray);
        assert!(config.verify_passphrase("unlock"));
        assert!(!config.verify_passphrase("wrong"));
    }

    #[test]
    fn validate_clamps_ranges_and_level() {
        let mut config = Config {
            work_interval_minutes: 0,
            break_duration_minutes: 1000,
            log_level: "debug".into(),
            ..Config::default()
        };
        config.validate();
        assert_eq!(config.work_interval_minutes, 1);
        assert_eq!(config.break_duration_minutes, 60);
        assert_eq!(config.log_level, "DEBUG");

        let mut config = Config {
            work_interval_minutes: 9999,
            break_duration_minutes: 0,
            log_level: "bogus".into(),
            ..Config::default()
        };
        config.validate();
        assert_eq!(config.work_interval_minutes, 480);
        assert_eq!(config.break_duration_minutes, 1);
        assert_eq!(config.log_level, "INFO");

        let mut config = Config {
            log_level: "off".into(),
            ..Config::default()
        };
        config.validate();
        assert_eq!(config.log_level, "OFF");
    }

    #[test]
    fn serde_round_trip_preserves_hash() {
        let config = Config::default();
        let json = serde_json::to_value(&config).unwrap();
        assert!(json.get("passphrase").is_none());
        assert!(json.get("passphrase_hash").is_some());
        let parsed: Config = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, config);
        assert!(parsed.verify_passphrase("unlock"));
    }

    #[test]
    fn missing_fields_fall_back_to_defaults_on_deserialize() {
        let parsed: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed.work_interval_minutes, 60);
        assert_eq!(parsed.break_duration_minutes, 5);
        assert!(parsed.enabled);
        assert_eq!(parsed.log_level, "INFO");
        assert!(parsed.autostart_daemon);
        assert!(parsed.autostart_tray);
        assert!(parsed.passphrase_hash.is_none());
    }

    #[test]
    fn legacy_plaintext_passphrase_is_migrated() {
        let raw = r#"{"passphrase": "pw123"}"#;
        let config: Config = serde_json::from_str::<RawConfig>(raw).unwrap().into();
        assert!(config.verify_passphrase("pw123"));
        assert!(!config.verify_passphrase("unlock"));
        let json = serde_json::to_value(&config).unwrap();
        assert!(json.get("passphrase").is_none());
        assert!(json.get("passphrase_hash").is_some());
    }

    #[test]
    fn legacy_empty_passphrase_falls_back_to_default() {
        let raw = r#"{"passphrase": ""}"#;
        let config: Config = serde_json::from_str::<RawConfig>(raw).unwrap().into();
        assert!(config.verify_passphrase("unlock"));
    }

    #[test]
    fn legacy_plaintext_ignored_when_hash_present() {
        let raw = r#"{"passphrase": "old", "passphrase_hash": {"salt_hex": "aa", "iterations": 10, "hash_hex": "bb"}}"#;
        let config: Config = serde_json::from_str::<RawConfig>(raw).unwrap().into();
        assert_eq!(
            config.passphrase_hash,
            Some(PassphraseHash {
                salt_hex: "aa".into(),
                iterations: 10,
                hash_hex: "bb".into()
            })
        );
    }

    #[test]
    fn set_passphrase_updates_hash() {
        let mut config = Config::default();
        config.set_passphrase("newpw");
        assert!(config.verify_passphrase("newpw"));
        assert!(!config.verify_passphrase("unlock"));
    }

    #[test]
    fn store_missing_file_yields_defaults() {
        let store = ConfigStore::new(temp_dir("cfg-missing"));
        let config = store.load();
        assert_eq!(config.work_interval_minutes, 60);
        assert!(config.verify_passphrase("unlock"));
    }

    #[test]
    fn store_save_load_round_trip() {
        let dir = temp_dir("cfg-roundtrip");
        let store = ConfigStore::new(&dir);
        let mut config = Config::default();
        config.work_interval_minutes = 90;
        config.set_passphrase("secret");
        config.log_level = "debug".into();
        store.save(&config).unwrap();
        assert!(!dir.join("config.tmp").exists());
        let loaded = store.load();
        assert_eq!(loaded.work_interval_minutes, 90);
        assert_eq!(loaded.log_level, "DEBUG");
        assert!(loaded.verify_passphrase("secret"));
        let text = fs::read_to_string(store.config_path()).unwrap();
        assert!(!text.contains("secret"));
    }

    #[test]
    fn store_corrupt_file_yields_defaults() {
        let dir = temp_dir("cfg-corrupt");
        let store = ConfigStore::new(&dir);
        fs::write(store.config_path(), "{not json").unwrap();
        let config = store.load();
        assert_eq!(config.work_interval_minutes, 60);
    }

    #[test]
    fn store_tolerates_wrong_field_types() {
        let dir = temp_dir("cfg-tolerant");
        let store = ConfigStore::new(&dir);
        fs::write(
            store.config_path(),
            r#"{"work_interval_minutes": "fast", "break_duration_minutes": 9999, "enabled": 1, "log_level": 42, "unknown_key": true}"#,
        )
        .unwrap();
        let config = store.load();
        assert_eq!(config.work_interval_minutes, 60);
        assert_eq!(config.break_duration_minutes, 60);
        assert!(config.enabled);
        assert_eq!(config.log_level, "INFO");
    }

    #[test]
    fn store_save_creates_parent_dirs() {
        let dir = temp_dir("cfg-deep").join("a/b/c");
        let store = ConfigStore::new(&dir);
        store.save(&Config::default()).unwrap();
        assert!(store.config_path().exists());
    }
}
