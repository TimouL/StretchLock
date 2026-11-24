//! Tray icon and menu. Built on the tao main thread (tray-icon's hidden
//! window needs a Win32 message pump, which the tao event loop provides);
//! menu item updates are applied there, synced from `Shared` once a
//! second and after every worker result.

use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use stretchlock_core::ipc::{Status, TimerState};

use crate::icon;

/// Commands from menu clicks and the settings WebView, executed by the
/// IPC worker.
pub enum Command {
    SetEnabled(bool),
    ForceBreak,
    UpdateConfig(
        Box<crate::settings::DraftConfig>,
        Option<crate::settings::PassphraseChange>,
    ),
    ToggleDaemonAutostart(bool),
    ToggleTrayAutostart(bool),
    /// Passphrase-gated action: verify, then act on `purpose`.
    VerifyPassphrase {
        purpose: crate::settings::PromptPurpose,
        passphrase: String,
    },
}

/// Outcomes of draining the global menu channel; window work is done by
/// the main loop, IPC work goes through `Command`. Gated actions open the
/// passphrase prompt first.
pub enum MenuAction {
    Command(Command),
    Gated(crate::settings::PromptPurpose),
}

/// Status line text shown as the disabled first menu entry.
pub fn next_break_text(status: Option<&Status>) -> String {
    match status {
        None => "下次休息: --:--".to_string(),
        Some(status) => match (status.enabled, status.state) {
            (false, _) => "计时器已禁用".to_string(),
            (true, TimerState::Breaking) => "休息中…".to_string(),
            (true, TimerState::AwaitingResume) => "休息完成".to_string(),
            (true, _) => {
                let m = status.remaining_seconds / 60;
                let s = status.remaining_seconds % 60;
                format!("下次休息: {m:02}:{s:02}")
            }
        },
    }
}

pub struct Tray {
    _tray: tray_icon::TrayIcon,
    status_item: MenuItem,
    enable_item: CheckMenuItem,
    daemon_autostart_item: CheckMenuItem,
    tray_autostart_item: CheckMenuItem,
}

impl Tray {
    pub fn new(enabled: bool, daemon_autostart: bool, tray_autostart: bool) -> Result<Self, String> {
        let status_item = MenuItem::with_id("status", "下次休息: --:--", false, None);
        let enable_item = CheckMenuItem::with_id("enable", "启用计时器", true, enabled, None);
        let daemon_autostart_item = CheckMenuItem::with_id(
            "autostart_daemon",
            "服务开机自启",
            true,
            daemon_autostart,
            None,
        );
        let tray_autostart_item = CheckMenuItem::with_id(
            "autostart_tray",
            "托盘开机自启",
            true,
            tray_autostart,
            None,
        );
        let break_item = MenuItem::with_id("break", "立即休息", true, None);
        let settings_item = MenuItem::with_id("settings", "设置…", true, None);
        let quit_item = MenuItem::with_id("quit", "退出托盘", true, None);
        let stop_item = MenuItem::with_id("stop", "停止服务…", true, None);

        let menu = Menu::new();
        for item in [
            &status_item as &dyn IsMenuItem,
            &PredefinedMenuItem::separator(),
            &enable_item,
            &daemon_autostart_item,
            &tray_autostart_item,
            &PredefinedMenuItem::separator(),
            &break_item,
            &settings_item,
            &PredefinedMenuItem::separator(),
            &quit_item,
            &stop_item,
        ] {
            menu.append(item).map_err(|e| format!("append menu item: {e}"))?;
        }

        let icon = Icon::from_rgba(icon::rgba(), 32, 32)
            .map_err(|e| format!("tray icon: {e}"))?;

        let tray = TrayIconBuilder::new()
            .with_id("stretchlock-tray")
            .with_menu(Box::new(menu))
            .with_tooltip("StretchLock")
            .with_icon(icon)
            .build()
            .map_err(|e| format!("build tray icon: {e}"))?;
        Ok(Self {
            _tray: tray,
            status_item,
            enable_item,
            daemon_autostart_item,
            tray_autostart_item,
        })
    }

    /// Push poller results into the status line and checkbox states.
    pub fn set_status(&self, status: Option<&Status>, tray_autostart: bool) {
        let _ = self.status_item.set_text(next_break_text(status));
        if let Some(status) = status {
            let _ = self.enable_item.set_checked(status.enabled);
        }
        let _ = self.tray_autostart_item.set_checked(tray_autostart);
    }

    pub fn set_enable_checked(&self, checked: bool) {
        let _ = self.enable_item.set_checked(checked);
    }

    pub fn set_daemon_autostart_checked(&self, checked: bool) {
        let _ = self.daemon_autostart_item.set_checked(checked);
    }

    /// Drain pending menu clicks. Must run on the tao main thread.
    pub fn drain_menu_events(&self) -> Vec<MenuAction> {
        let mut actions = Vec::new();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            log::debug!("menu event: {}", event.id.0);
            let action = match event.id.0.as_str() {
                // muda toggles CheckMenuItem state itself before the event
                // arrives (see muda's Windows dispatch), so the current
                // value already IS the requested new state.
                "enable" => {
                    if self.enable_item.is_checked() {
                        MenuAction::Command(Command::SetEnabled(true))
                    } else {
                        MenuAction::Gated(crate::settings::PromptPurpose::DisableTimer)
                    }
                }
                "autostart_daemon" => MenuAction::Command(Command::ToggleDaemonAutostart(
                    self.daemon_autostart_item.is_checked(),
                )),
                "autostart_tray" => MenuAction::Command(Command::ToggleTrayAutostart(
                    self.tray_autostart_item.is_checked(),
                )),
                "break" => MenuAction::Command(Command::ForceBreak),
                "settings" => MenuAction::Gated(crate::settings::PromptPurpose::OpenSettings),
                "stop" => MenuAction::Gated(crate::settings::PromptPurpose::StopService),
                "quit" => MenuAction::Gated(crate::settings::PromptPurpose::Quit),
                _ => continue,
            };
            actions.push(action);
        }
        actions
    }
}
