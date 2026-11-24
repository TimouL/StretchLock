//! Portable orchestration: wires config, stats and timer together and
//! drives the Windows-only platform pieces (mouse hook + overlay).
//! Mirrors the role of `app.py` in the Python version.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use stretchlock_core::config::{Config, ConfigStore};
use stretchlock_core::ipc::{Request, Response, Status, TimerState};
use stretchlock_core::scene::{LockScene, ScenePicker};
use stretchlock_core::stats::Stats;

use crate::timer::{Timer, TimerConfig};

#[cfg(windows)]
use crate::hooks::HookGuard;
#[cfg(windows)]
use crate::overlay;

/// How the IPC server talks to the application.
pub trait RequestHandler: Send + Sync + 'static {
    /// Process one request. The bool is true when the daemon must exit
    /// after the response has been delivered (StopService success).
    fn handle(&self, request: Request) -> (Response, bool);
    /// Best-effort teardown before the process exits.
    fn shutdown(&self);
}

pub struct App {
    config: Mutex<Config>,
    store: ConfigStore,
    stats: Mutex<Stats>,
    timer: Arc<Timer>,
    breaking: AtomicBool,
    /// Rotates through the configured break messages, one per break.
    message_index: std::sync::atomic::AtomicUsize,
    /// Background-scene picker; its state is persisted in stats so the
    /// no-repeat / animal-pity guarantee survives restarts.
    scene_picker: Mutex<ScenePicker>,
    /// Scene chosen for the current break.
    current_scene: Mutex<Option<LockScene>>,
    #[cfg(windows)]
    hook: Mutex<Option<HookGuard>>,
    #[cfg(windows)]
    keyboard: Mutex<Option<crate::hooks::KeyboardGuard>>,
}

impl App {
    pub fn new(store: ConfigStore, mut config: Config, stats: Stats) -> Self {
        config.validate();
        #[cfg(windows)]
        crate::mascot::select_set(&config.mascot);
        // Restore the scene picker from the persisted stats state.
        let (last_scene_id, streak) = stats.scene_state();
        let scene_picker = ScenePicker::from_parts(LockScene::from_id(last_scene_id), streak);
        let timer = Arc::new(Timer::new(TimerConfig {
            work_seconds: config.work_interval_minutes * 60,
            break_seconds: config.break_duration_minutes * 60,
        }));
        if config.enabled {
            timer.start();
        } else {
            timer.stop();
        }
        App {
            config: Mutex::new(config),
            store,
            stats: Mutex::new(stats),
            timer,
            breaking: AtomicBool::new(false),
            message_index: std::sync::atomic::AtomicUsize::new(0),
            scene_picker: Mutex::new(scene_picker),
            current_scene: Mutex::new(None),
            #[cfg(windows)]
            hook: Mutex::new(None),
            #[cfg(windows)]
            keyboard: Mutex::new(None),
        }
    }

    /// Advance the timer by one second and react to phase transitions.
    pub fn tick_once(&self) {
        let events = self.timer.tick();
        if events.break_started {
            if self.config.lock().enabled {
                self.begin_break();
            } else {
                // Stale Working state while disabled: pull back to Idle.
                self.timer.stop();
            }
        }
        if events.break_ended {
            self.on_break_finished();
        }
        let snapshot = self.timer.snapshot();
        if snapshot.state == TimerState::Breaking {
            self.show_break_text();
            #[cfg(windows)]
            overlay::widget_hide();
        } else if snapshot.state == TimerState::AwaitingResume {
            // Lock stays; resume is event-driven from the overlay button.
            #[cfg(windows)]
            overlay::widget_update("00:00");
        } else {
            if self.breaking.load(Ordering::SeqCst) {
                self.end_break();
            }
            if snapshot.state == TimerState::Working {
                #[cfg(windows)]
                overlay::widget_update(&format_mmss(snapshot.remaining_seconds));
            } else {
                #[cfg(windows)]
                overlay::widget_hide();
            }
        }
    }

    /// Event-driven passphrase override, called the moment Enter is pressed.
    pub fn check_passphrase_override(&self) {
        #[cfg(windows)]
        {
            if self.timer.snapshot().state != TimerState::Breaking {
                return;
            }
            if let Some(typed) = crate::hooks::take_submitted_passphrase() {
                if self.config.lock().verify_passphrase(&typed) {
                    log::info!("Break overridden via passphrase");
                    self.timer.skip_break();
                    self.end_break();
                } else {
                    log::debug!("Passphrase mismatch during break");
                }
            }
        }
    }

    pub fn force_break(&self) {
        log::info!("force_break enter");
        self.timer.force_break_now();
        log::info!("force_break timer set, begin_break");
        self.begin_break();
        log::info!("force_break done");
    }

    fn begin_break(&self) {
        log::info!("begin_break enter");
        self.breaking.store(true, Ordering::SeqCst);
        self.message_index.fetch_add(1, Ordering::SeqCst);
        // Pick this break's background scene and persist the picker
        // state right away, so the guarantee holds across restarts.
        let (scene, streak) = {
            let mut picker = self.scene_picker.lock();
            let scene = picker.pick();
            (scene, picker.non_animal_streak())
        };
        *self.current_scene.lock() = Some(scene);
        self.stats.lock().set_scene_state(scene.id(), streak);
        log::info!("Break scene: {} ({})", scene.name(), scene.id());
        log::info!("Break started");
        #[cfg(windows)]
        {
            {
                let mut hook = self.hook.lock();
                if hook.is_none() {
                    *hook = crate::hooks::start_mouse_block();
                    if hook.is_none() {
                        log::warn!("Mouse hook unavailable; overlay alone handles blocking");
                    }
                }
            }
            {
                let mut keyboard = self.keyboard.lock();
                if keyboard.is_none() {
                    *keyboard = crate::hooks::start_keyboard_capture();
                    if keyboard.is_none() {
                        log::warn!("Keyboard capture unavailable; passphrase override inactive");
                    }
                }
                crate::hooks::reset_passphrase_buffer();
            }
            log::info!("begin_break hooks ok, activate overlay");
            overlay::activate();
            log::info!("begin_break overlay activated, show text");
            self.show_break_text();
            log::info!("begin_break text shown");
        }
    }

    fn end_break(&self) {
        self.breaking.store(false, Ordering::SeqCst);
        #[cfg(windows)]
        {
            self.hook.lock().take();
            self.keyboard.lock().take();
            overlay::deactivate();
        }
        log::info!("Break ended");
    }

    /// Break countdown finished: record the completed break, keep the lock
    /// screen up, release the mouse (so the resume button is clickable) and
    /// switch the overlay to its "done" view.
    fn on_break_finished(&self) {
        self.stats.lock().record_break();
        log::info!("Break finished; awaiting resume");
        #[cfg(windows)]
        {
            self.hook.lock().take();
            self.show_done_text();
        }
    }

    /// User clicked the resume button.
    pub fn on_resume(&self) {
        if self.timer.snapshot().state != TimerState::AwaitingResume {
            return;
        }
        if self.config.lock().enabled {
            self.timer.resume_after_break();
            log::info!("Work resumed after break");
        } else {
            self.timer.stop();
            log::info!("Break done; timer disabled, staying idle");
        }
        self.end_break();
    }

    fn show_break_text(&self) {
        #[cfg(windows)]
        {
            let snapshot = self.timer.snapshot();
            let (breaks_today, total_breaks, streak) = self.stats.lock().get_summary();
            let hint = if self.keyboard.lock().is_some() {
                "输入口令后按回车可提前结束"
            } else {
                ""
            };
            let config = self.config.lock();
            let message = config
                .message_for(self.message_index.load(Ordering::SeqCst))
                .to_string();
            let animate = config.animation_enabled;
            let break_total = (config.break_duration_minutes as u32).saturating_mul(60);
            drop(config);
            let fraction = if break_total == 0 {
                0.0
            } else {
                snapshot.remaining_seconds as f32 / break_total as f32
            };
            overlay::update(
                "休息一下",
                &format_mmss(snapshot.remaining_seconds),
                &format!(
                    "今日 {breaks_today} 次    累计 {total_breaks} 次    连续 {streak} 天"
                ),
                hint,
                &message,
                crate::hooks::passphrase_len(),
                animate,
                fraction.clamp(0.0, 1.0),
                false,
                *self.current_scene.lock(),
            );
        }
    }

    /// Done view: fixed message + resume button, no passphrase field.
    fn show_done_text(&self) {
        #[cfg(windows)]
        {
            let (breaks_today, total_breaks, streak) = self.stats.lock().get_summary();
            let animate = self.config.lock().animation_enabled;
            overlay::update(
                "休息一下",
                "00:00",
                &format!(
                    "今日 {breaks_today} 次    累计 {total_breaks} 次    连续 {streak} 天"
                ),
                "",
                "休息时间已到，可以开始使用电脑了",
                0,
                animate,
                0.0,
                true,
                *self.current_scene.lock(),
            );
        }
    }

    fn status(&self) -> Response {
        let (breaks_today, _total, streak) = self.stats.lock().get_summary();
        let snapshot = self.timer.snapshot();
        Response::Status(Status {
            enabled: self.config.lock().enabled,
            state: snapshot.state,
            remaining_seconds: snapshot.remaining_seconds,
            breaks_today,
            streak,
        })
    }

    fn set_enabled(&self, enabled: bool) {
        {
            let mut config = self.config.lock();
            config.enabled = enabled;
            if let Err(err) = self.store.save(&config) {
                log::warn!("Failed to save config: {}", err);
            }
        }
        self.timer.set_enabled(enabled);
        if !enabled && self.breaking.load(Ordering::SeqCst) {
            // Disabling during a break releases the lock immediately.
            self.end_break();
        }
        log::info!(
            "Timer {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    fn apply_config(&self, new_config: Config) {
        // Snapshot the fields we need, then release the config lock before
        // touching the timer / overlay (keeps lock scope small).
        let (work, break_min, enabled, log_level, mascot) = {
            let mut config = self.config.lock();
            *config = new_config;
            if let Err(err) = self.store.save(&config) {
                log::warn!("Failed to save config: {}", err);
            }
            (
                config.work_interval_minutes,
                config.break_duration_minutes,
                config.enabled,
                config.log_level.clone(),
                config.mascot.clone(),
            )
        };

        #[cfg(windows)]
        crate::mascot::select_set(&mascot);
        self.timer.apply_config(work, break_min);

        if !enabled {
            self.timer.stop();
            if self.breaking.load(Ordering::SeqCst) {
                self.end_break();
            }
            log::info!("Settings applied (timer disabled)");
            return;
        }
        stretchlock_core::logging::set_level(&log_level);
        log::info!(
            "Settings applied: work {} min, break {} min, log level {}",
            work,
            break_min,
            log_level
        );
    }
}

impl RequestHandler for App {
    fn handle(&self, request: Request) -> (Response, bool) {
        match request {
            Request::GetStatus => (self.status(), false),
            Request::SetEnabled(enabled) => {
                self.set_enabled(enabled);
                (Response::Ok, false)
            }
            Request::ForceBreak => {
                self.force_break();
                (Response::Ok, false)
            }
            Request::UpdateConfig(config) => {
                self.apply_config(*config);
                (Response::Ok, false)
            }
            Request::StopService { passphrase } => {
                if self.config.lock().verify_passphrase(&passphrase) {
                    log::info!("StopService accepted");
                    (Response::Ok, true)
                } else {
                    log::warn!("StopService denied: passphrase mismatch");
                    (Response::Denied, false)
                }
            }
            Request::VerifyPassphrase { passphrase } => {
                if self.config.lock().verify_passphrase(&passphrase) {
                    (Response::Ok, false)
                } else {
                    (Response::Denied, false)
                }
            }
        }
    }

    fn shutdown(&self) {
        log::info!("Shutting down StretchLock daemon");
        self.breaking.store(false, Ordering::SeqCst);
        #[cfg(windows)]
        {
            self.hook.lock().take();
            overlay::deactivate();
        }
        self.timer.stop();
    }
}

fn format_mmss(seconds: u32) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}
