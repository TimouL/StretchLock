//! Settings page logic: serde types for the WebView IPC protocol and pure
//! validation/normalization used by the IPC worker. No UI toolkit here —
//! the page itself is the HTML embedded in `ui.rs`.

use serde::{Deserialize, Serialize};

use stretchlock_core::config::{
    Config, DEFAULT_LOG_LEVEL, MAX_BREAK_DURATION, MAX_WORK_INTERVAL, MIN_BREAK_DURATION,
    MIN_WORK_INTERVAL, VALID_LOG_LEVELS,
};

/// Commands sent up from the page (`window.ipc.postMessage`).
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum UiEvent {
    Ready,
    Close,
    Save {
        config: DraftConfig,
        #[serde(default)]
        passphrase_change: Option<PassphraseChange>,
    },
    Stop {
        passphrase: String,
    },
    /// Generic passphrase prompt submission (gate for menu actions).
    Prompt {
        purpose: String,
        passphrase: String,
    },
    /// Page asks the host to start an interactive window move.
    Drag,
}

/// What the passphrase gate protects. Shared by the tray menu (MenuAction)
/// and the page protocol (Prompt.purpose string).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptPurpose {
    DisableTimer,
    OpenSettings,
    Quit,
    StopService,
}

impl PromptPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            PromptPurpose::DisableTimer => "disable",
            PromptPurpose::OpenSettings => "settings",
            PromptPurpose::Quit => "quit",
            PromptPurpose::StopService => "stop",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "disable" => Some(PromptPurpose::DisableTimer),
            "settings" => Some(PromptPurpose::OpenSettings),
            "quit" => Some(PromptPurpose::Quit),
            "stop" => Some(PromptPurpose::StopService),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            PromptPurpose::DisableTimer => "停用计时器",
            PromptPurpose::OpenSettings => "进入设置",
            PromptPurpose::Quit => "退出托盘",
            PromptPurpose::StopService => "停止服务",
        }
    }
}

/// Numeric/log-level/toggle part of a save request.
#[derive(Deserialize, Clone, Debug)]
pub struct DraftConfig {
    pub work_minutes: u32,
    pub break_minutes: u32,
    pub enabled: bool,
    pub log_level: String,
    pub autostart_daemon: bool,
    pub autostart_tray: bool,
    /// One message per line.
    #[serde(default)]
    pub messages: String,
    #[serde(default = "default_true")]
    pub animation_enabled: bool,
    #[serde(default = "default_mascot")]
    pub mascot: String,
}

fn default_true() -> bool {
    true
}

fn default_mascot() -> String {
    "cat".to_string()
}

/// Passphrase change request; only present when the user typed something.
#[derive(Deserialize, Clone, Debug)]
pub struct PassphraseChange {
    pub old: String,
    pub new: String,
    pub confirm: String,
}

/// Page-facing view of the config. Deliberately excludes the passphrase
/// hash; the page never sees any passphrase material.
#[derive(Serialize, Clone, Debug)]
pub struct PageConfig {
    pub work_minutes: u32,
    pub break_minutes: u32,
    pub enabled: bool,
    pub log_level: String,
    pub autostart_daemon: bool,
    pub autostart_tray: bool,
    pub messages: String,
    pub animation_enabled: bool,
    pub mascot: String,
}

impl From<&Config> for PageConfig {
    fn from(config: &Config) -> Self {
        Self {
            work_minutes: config.work_interval_minutes,
            break_minutes: config.break_duration_minutes,
            enabled: config.enabled,
            log_level: config.log_level.clone(),
            autostart_daemon: config.autostart_daemon,
            autostart_tray: config.autostart_tray,
            messages: config.messages.join("\n"),
            animation_enabled: config.animation_enabled,
            mascot: config.mascot.clone(),
        }
    }
}

/// Normalize a submitted log level; unknown values fall back to the default.
pub fn normalize_log_level(input: &str) -> String {
    let upper = input.trim().to_uppercase();
    if VALID_LOG_LEVELS.contains(&upper.as_str()) {
        upper
    } else {
        DEFAULT_LOG_LEVEL.to_string()
    }
}

/// Overlay the page draft onto the current config (passphrase untouched).
pub fn apply_draft(current: &Config, draft: &DraftConfig) -> Config {
    let mut config = current.clone();
    config.work_interval_minutes =
        draft.work_minutes.clamp(MIN_WORK_INTERVAL, MAX_WORK_INTERVAL);
    config.break_duration_minutes =
        draft.break_minutes.clamp(MIN_BREAK_DURATION, MAX_BREAK_DURATION);
    config.log_level = normalize_log_level(&draft.log_level);
    config.enabled = draft.enabled;
    config.autostart_daemon = draft.autostart_daemon;
    config.autostart_tray = draft.autostart_tray;
    config.animation_enabled = draft.animation_enabled;
    if ["cat", "dog", "dino"].contains(&draft.mascot.as_str()) {
        config.mascot = draft.mascot.clone();
    }
    let lines: Vec<String> = draft
        .messages
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    if !lines.is_empty() {
        config.messages = lines;
    }
    config
}

/// Validate a passphrase change against the current config. The page only
/// sends `PassphraseChange` when at least one field is filled.
pub fn validate_passphrase_change(
    current: &Config,
    change: &PassphraseChange,
) -> Result<(), String> {
    if !current.verify_passphrase(&change.old) {
        return Err("当前口令不正确".into());
    }
    if change.new.is_empty() {
        return Err("新口令不能为空".into());
    }
    if change.new != change.confirm {
        return Err("两次输入的新口令不一致".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> UiEvent {
        serde_json::from_str(json).expect("parse ui event")
    }

    #[test]
    fn ui_events_parse() {
        assert!(matches!(parse(r#"{"cmd":"ready"}"#), UiEvent::Ready));
        assert!(matches!(parse(r#"{"cmd":"close"}"#), UiEvent::Close));
        assert!(matches!(parse(r#"{"cmd":"drag"}"#), UiEvent::Drag));

        let save = parse(
            r#"{"cmd":"save","config":{"work_minutes":45,"break_minutes":7,
               "enabled":false,"log_level":"INFO","autostart_daemon":true,
               "autostart_tray":false}}"#,
        );
        match save {
            UiEvent::Save { config, passphrase_change } => {
                assert_eq!(config.work_minutes, 45);
                assert_eq!(config.break_minutes, 7);
                assert!(!config.enabled);
                assert_eq!(config.log_level, "INFO");
                assert!(config.autostart_daemon);
                assert!(!config.autostart_tray);
                assert!(passphrase_change.is_none());
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let save = parse(
            r#"{"cmd":"save","config":{"work_minutes":60,"break_minutes":5,
               "enabled":true,"log_level":"DEBUG","autostart_daemon":true,
               "autostart_tray":true},
               "passphrase_change":{"old":"unlock","new":"n","confirm":"n"}}"#,
        );
        match save {
            UiEvent::Save { config, passphrase_change } => {
                assert_eq!(config.work_minutes, 60);
                let change = passphrase_change.expect("passphrase change present");
                assert_eq!(change.old, "unlock");
                assert_eq!(change.new, "n");
                assert_eq!(change.confirm, "n");
            }
            other => panic!("unexpected event: {other:?}"),
        }

        match parse(r#"{"cmd":"stop","passphrase":"pw"}"#) {
            UiEvent::Stop { passphrase } => assert_eq!(passphrase, "pw"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn ui_events_reject_unknown_command() {
        assert!(serde_json::from_str::<UiEvent>(r#"{"cmd":"nope"}"#).is_err());
        assert!(serde_json::from_str::<UiEvent>(r#"{"cmd":"save"}"#).is_err());
    }

    #[test]
    fn apply_draft_clamps_and_normalizes() {
        let current = Config::default();
        let draft = DraftConfig {
            work_minutes: 9999,
            break_minutes: 0,
            enabled: false,
            log_level: "debug".into(),
            autostart_daemon: false,
            autostart_tray: false,
            messages: String::new(),
            animation_enabled: false,
            mascot: "cat".into(),
        };
        let config = apply_draft(&current, &draft);
        assert_eq!(config.work_interval_minutes, MAX_WORK_INTERVAL);
        assert_eq!(config.break_duration_minutes, MIN_BREAK_DURATION);
        assert_eq!(config.log_level, "DEBUG");
        assert!(!config.enabled);
        assert!(!config.autostart_daemon);
        assert!(!config.autostart_tray);
        // passphrase hash survives a plain settings save
        assert!(config.verify_passphrase("unlock"));
    }

    #[test]
    fn invalid_log_level_falls_back() {
        assert_eq!(normalize_log_level("warning"), "WARNING");
        assert_eq!(normalize_log_level(" off "), "OFF");
        assert_eq!(normalize_log_level("verbose"), DEFAULT_LOG_LEVEL);
    }

    #[test]
    fn passphrase_change_validation() {
        let config = Config::default();
        let good = PassphraseChange {
            old: "unlock".into(),
            new: "newpw".into(),
            confirm: "newpw".into(),
        };
        assert!(validate_passphrase_change(&config, &good).is_ok());

        let wrong_old = PassphraseChange {
            old: "nope".into(),
            new: "newpw".into(),
            confirm: "newpw".into(),
        };
        assert_eq!(
            validate_passphrase_change(&config, &wrong_old).unwrap_err(),
            "当前口令不正确"
        );

        let empty_new = PassphraseChange {
            old: "unlock".into(),
            new: String::new(),
            confirm: String::new(),
        };
        assert_eq!(
            validate_passphrase_change(&config, &empty_new).unwrap_err(),
            "新口令不能为空"
        );

        let mismatch = PassphraseChange {
            old: "unlock".into(),
            new: "a".into(),
            confirm: "b".into(),
        };
        assert_eq!(
            validate_passphrase_change(&config, &mismatch).unwrap_err(),
            "两次输入的新口令不一致"
        );
    }
}
