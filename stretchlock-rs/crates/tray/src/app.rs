//! Application wiring: tao event loop + wry settings window + tray icon +
//! IPC worker thread, glued through `Shared`. The tao loop on the main
//! thread is the Win32 message pump the tray icon needs; menu clicks and
//! WebView commands reach the worker over a command channel, and worker
//! results flow back as `UserEvent` downlinks pushed into the page.
//!
//! The settings window is a borderless tao window hosting the WebView2
//! page from `ui.rs`; the webview is created lazily on first open. The
//! tray pump is a hidden zero-sized window parked off-screen; it must
//! never exit the app (only the tray menu's quit does, or a successful
//! StopService).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::{Window, WindowBuilder};
use wry::{WebView, WebViewBuilder};

use stretchlock_core::ipc::{Request, Response, Status};
use stretchlock_core::{Config, ConfigStore};

use crate::autostart;
use crate::ipc;
use crate::settings::{self, PageConfig, PromptPurpose, UiEvent};
use crate::tray::{Command, MenuAction, Tray};
use crate::ui::SETTINGS_HTML;

const SETTINGS_WINDOW_SIZE: LogicalSize<f64> = LogicalSize::new(520.0, 560.0);
const SETTINGS_WINDOW_MIN: LogicalSize<f64> = LogicalSize::new(430.0, 480.0);

/// Host -> page downlink messages (`window.__stretchlock.receive`).
enum Downlink {
    /// Window opened: current config + latest status.
    Open {
        config: PageConfig,
        status: Option<Status>,
    },
    /// One-second status poll.
    Status(Option<Status>),
    /// Config changed externally (tray menu toggles); live-syncs the
    /// page's toggle states without clobbering in-progress edits.
    Config(PageConfig),
    SaveOk,
    SaveError(String),
    /// Passphrase gate: show the prompt for `purpose`.
    Prompt {
        purpose: String,
    },
    /// Passphrase rejected; stay on the gate.
    PromptError {
        error: String,
    },
    /// Gate passed and the action finished; close the window (non-settings).
    PromptDone,
}

impl Downlink {
    fn json(&self) -> String {
        #[derive(serde::Serialize)]
        #[serde(tag = "event", rename_all = "kebab-case")]
        enum Msg<'a> {
            Open {
                config: &'a PageConfig,
                status: &'a Option<Status>,
            },
            Status {
                status: &'a Option<Status>,
            },
            Config {
                config: &'a PageConfig,
            },
            Saved,
            SaveError {
                error: &'a str,
            },
            Prompt {
                purpose: &'a str,
            },
            PromptError {
                error: &'a str,
            },
            PromptDone,
        }
        let msg = match self {
            Downlink::Open { config, status } => Msg::Open { config, status },
            Downlink::Status(status) => Msg::Status { status },
            Downlink::Config(config) => Msg::Config { config },
            Downlink::SaveOk => Msg::Saved,
            Downlink::SaveError(error) => Msg::SaveError { error },
            Downlink::Prompt { purpose } => Msg::Prompt { purpose },
            Downlink::PromptError { error } => Msg::PromptError { error },
            Downlink::PromptDone => Msg::PromptDone,
        };
        serde_json::to_string(&msg).expect("serialize downlink")
    }
}

/// Messages routed through the event loop proxy. The wry IPC callback
/// runs on the main thread but cannot touch `Window` directly through
/// `Shared`, so it sends `Ui` requests instead.
enum LoopMessage {
    /// Worker thread -> settings page.
    Downlink(Downlink),
    /// WebView -> main thread window actions.
    Ui(UiRequest),
}

enum UiRequest {
    Close(tao::window::WindowId),
    Drag(tao::window::WindowId),
}

/// State shared between the main thread, the wry IPC callback and the
/// IPC worker thread.
pub struct Shared {
    pub config: Mutex<Config>,
    pub last_status: Mutex<Option<Status>>,
    /// Menu clicks and WebView commands forwarded to the worker.
    pub commands: Mutex<Vec<Command>>,
    /// Loop wake-ups; set once the event loop is built.
    proxy: Mutex<Option<EventLoopProxy<LoopMessage>>>,
    /// Prompt requested while the page was still loading; the `ready`
    /// handshake re-sends it.
    pending_prompt: Mutex<Option<PromptPurpose>>,
    /// Bumped whenever `last_status` or the config changes.
    pub version: AtomicU64,
}

impl Shared {
    fn new(config: Config) -> Self {
        Self {
            config: Mutex::new(config),
            last_status: Mutex::new(None),
            commands: Mutex::new(Vec::new()),
            proxy: Mutex::new(None),
            pending_prompt: Mutex::new(None),
            version: AtomicU64::new(0),
        }
    }

    pub fn queue(&self, command: Command) {
        self.commands.lock().unwrap().push(command);
    }

    fn set_proxy(&self, proxy: EventLoopProxy<LoopMessage>) {
        *self.proxy.lock().unwrap() = Some(proxy);
    }

    fn send(&self, message: LoopMessage) {
        // Before the proxy exists no window can be open, so a dropped
        // downlink is harmless.
        if let Some(proxy) = self.proxy.lock().unwrap().as_ref() {
            let _ = proxy.send_event(message);
        }
    }

    fn push(&self, downlink: Downlink) {
        self.send(LoopMessage::Downlink(downlink));
    }

    fn status(&self) -> Option<Status> {
        self.last_status.lock().unwrap().clone()
    }

    fn request_prompt(&self, purpose: PromptPurpose) {
        *self.pending_prompt.lock().unwrap() = Some(purpose);
    }

    fn take_prompt(&self) -> Option<PromptPurpose> {
        self.pending_prompt.lock().unwrap().take()
    }

    /// Snapshot for the page: current config + latest status.
    fn page_state(&self) -> (PageConfig, Option<Status>) {
        (
            PageConfig::from(&*self.config.lock().unwrap()),
            self.last_status.lock().unwrap().clone(),
        )
    }
}

pub fn save_config(config: &Config) -> std::io::Result<()> {
    ConfigStore::default().save(config)
}

pub fn run(config: Config) {
    // The tray Run entry is the source of truth for the checkbox.
    let mut config = config;
    config.autostart_tray = autostart::enabled();

    if !ipc::daemon_running() {
        log::info!("daemon not running; trying to start it");
        if let Err(err) = ipc::ensure_daemon() {
            log::warn!("daemon not started: {err}");
        }
    }

    let shared = Arc::new(Shared::new(config));

    let mut event_loop = EventLoopBuilder::<LoopMessage>::with_user_event().build();
    shared.set_proxy(event_loop.create_proxy());

    let worker = std::thread::Builder::new()
        .name("ipc-worker".into())
        .spawn({
            let shared = Arc::clone(&shared);
            move || worker_main(shared)
        })
        .expect("spawn ipc worker");

    // Tray pump: hidden, zero-sized, parked off-screen so it can never
    // flash on screen. Created on this thread so tray-icon binds to a
    // window owned by the tao loop's thread.
    let pump = WindowBuilder::new()
        .with_title("StretchLock")
        .with_visible(false)
        .with_inner_size(LogicalSize::new(0.0f64, 0.0f64))
        .with_resizable(false)
        .with_position(LogicalPosition::new(-32000.0f64, -32000.0f64))
        .build(&event_loop)
        .expect("build tray pump window");

    let config = shared.config.lock().unwrap().clone();
    let tray = match Tray::new(config.enabled, config.autostart_daemon, config.autostart_tray) {
        Ok(tray) => tray,
        Err(err) => {
            log::error!("failed to create tray: {err}");
            eprintln!("failed to create tray: {err}");
            std::process::exit(1);
        }
    };

    let mut window: Option<SettingsWindow> = None;
    let mut last_version = 0u64;
    let mut last_tick = Instant::now() - Duration::from_secs(1);

    log::info!("tray event loop starting");
    let exit_code = event_loop.run_return(move |event, event_loop, control_flow| {
        match event {
            Event::NewEvents(StartCause::Init) => {
                // First 1Hz tick one second from now.
                *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1));
            }
            Event::UserEvent(message) => match message {
                LoopMessage::Downlink(downlink) => {
                    // Worker results land here; hidden pages drop them.
                    if let Some(win) = window.as_mut() {
                        win.push(&downlink);
                    }
                }
                LoopMessage::Ui(UiRequest::Close(id)) => {
                    if let Some(win) = window.as_mut() {
                        if win.window.id() == id {
                            win.hide();
                        }
                    }
                }
                LoopMessage::Ui(UiRequest::Drag(id)) => {
                    if let Some(win) = window.as_mut() {
                        if win.window.id() == id {
                            if let Err(err) = win.window.drag_window() {
                                log::warn!("drag_window failed: {err}");
                            }
                        }
                    }
                }
            },
            Event::WindowEvent {
                window_id,
                event: WindowEvent::CloseRequested,
                ..
            } => {
                // Only the settings window ever gets an OS-initiated
                // close (it is borderless; the page's X button goes
                // through UiRequest::Close). Hide it. The pump window
                // must never exit the app.
                if let Some(win) = window.as_mut() {
                    if win.window.id() == window_id {
                        win.hide();
                    }
                }
            }
            Event::LoopDestroyed => {}
            _ => {}
        }

        // 1Hz tick: tray status line + page status push. The timer is
        // re-armed after every batch, wake-ups included.
        if last_tick.elapsed() >= Duration::from_secs(1) {
            last_tick = Instant::now();
            sync_menu(&shared, &tray);
            if let Some(win) = window.as_mut() {
                win.push(&Downlink::Status(shared.status()));
            }
        }
        *control_flow = ControlFlow::WaitUntil(last_tick + Duration::from_secs(1));

        // Tray menu clicks arrive on the global muda channel; drain them
        // on every event batch so clicks feel instant.
        for action in tray.drain_menu_events() {
            match action {
                MenuAction::Command(command) => shared.queue(command),
                MenuAction::Gated(purpose) => {
                    shared.request_prompt(purpose);
                    ensure_settings_window(&mut window, event_loop, &shared);
                    if let Some(win) = window.as_mut() {
                        win.push(&Downlink::Prompt { purpose: purpose.as_str().to_string() });
                    }
                }
            }
        }

        // Re-sync tray state whenever worker results bump the version.
        let version = shared.version.load(Ordering::SeqCst);
        if version != last_version {
            last_version = version;
            sync_menu(&shared, &tray);
        }
    });

    log::info!("tray exiting; daemon keeps running");
    worker.join().ok();
    let _ = (pump, exit_code);
}

/// Open the settings window (creating the webview on first use) and push
/// the current config + status into the page.
fn ensure_settings_window(
    window: &mut Option<SettingsWindow>,
    target: &EventLoopWindowTarget<LoopMessage>,
    shared: &Arc<Shared>,
) {
    if window.is_none() {
        match SettingsWindow::create(target, shared) {
            Ok(win) => *window = Some(win),
            Err(err) => {
                log::error!("settings window: {err}");
                return;
            }
        }
    }
    let win = window.as_mut().unwrap();
    let (config, status) = shared.page_state();
    win.show();
    win.push(&Downlink::Open { config, status });
}

fn sync_menu(shared: &Arc<Shared>, tray: &Tray) {
    let status = shared.status();
    let config = shared.config.lock().unwrap().clone();
    tray.set_status(status.as_ref(), config.autostart_tray);
    tray.set_daemon_autostart_checked(config.autostart_daemon);
    tray.set_enable_checked(
        status
            .as_ref()
            .map(|s| s.enabled)
            .unwrap_or(config.enabled),
    );
}

/// The settings window: borderless tao window + lazily created WebView.
struct SettingsWindow {
    window: Window,
    webview: Option<WebView>,
}

impl SettingsWindow {
    fn create(
        target: &EventLoopWindowTarget<LoopMessage>,
        shared: &Arc<Shared>,
    ) -> Result<Self, String> {
        let window = WindowBuilder::new()
            .with_title("StretchLock 设置")
            .with_inner_size(SETTINGS_WINDOW_SIZE)
            .with_min_inner_size(SETTINGS_WINDOW_MIN)
            .with_decorations(false)
            .with_resizable(true)
            .with_visible(false)
            .build(target)
            .map_err(|err| format!("build settings window: {err}"))?;

        let handler_shared = Arc::clone(shared);
        let window_id = window.id();
        let webview = WebViewBuilder::new(&window)
            .with_html(SETTINGS_HTML)
            .with_ipc_handler(move |request| {
                handle_page_message(&handler_shared, window_id, request.body());
            })
            .build()
            .map_err(|err| format!("build webview: {err}"))?;

        Ok(Self {
            window,
            webview: Some(webview),
        })
    }

    fn show(&mut self) {
        self.window.set_visible(true);
        self.window.set_focus();
    }

    fn hide(&mut self) {
        self.window.set_visible(false);
    }

    /// Push a downlink into the page. Hidden windows get no script: the
    /// next open re-pushes everything anyway.
    fn push(&mut self, downlink: &Downlink) {
        if !self.window.is_visible() {
            return;
        }
        if let Some(webview) = &self.webview {
            let script = format!(
                "window.__stretchlock && window.__stretchlock.receive({});",
                downlink.json()
            );
            if let Err(err) = webview.evaluate_script(&script) {
                log::warn!("evaluate_script failed: {err}");
            }
        }
    }
}

/// Page -> host uplink. Runs on the wry IPC callback (main thread); only
/// cheap parsing here, heavy work goes to the worker's command queue.
fn handle_page_message(shared: &Arc<Shared>, window_id: tao::window::WindowId, body: &str) {
    let event: UiEvent = match serde_json::from_str(body) {
        Ok(event) => event,
        Err(err) => {
            log::warn!("bad page message: {err}");
            return;
        }
    };
    match event {
        UiEvent::Ready => {
            let (config, status) = shared.page_state();
            shared.push(Downlink::Open { config, status });
            if let Some(purpose) = shared.take_prompt() {
                shared.push(Downlink::Prompt { purpose: purpose.as_str().to_string() });
            }
        }
        UiEvent::Close => shared.send(LoopMessage::Ui(UiRequest::Close(window_id))),
        UiEvent::Drag => shared.send(LoopMessage::Ui(UiRequest::Drag(window_id))),
        UiEvent::Save { config, passphrase_change } => {
            shared.queue(Command::UpdateConfig(Box::new(config), passphrase_change));
        }
        UiEvent::Stop { passphrase } => {
            // Legacy stop path; still goes through the passphrase gate.
            shared.queue(Command::VerifyPassphrase {
                purpose: PromptPurpose::StopService,
                passphrase,
            });
        }
        UiEvent::Prompt { purpose, passphrase } => {
            match PromptPurpose::from_str(&purpose) {
                Some(purpose) => shared.queue(Command::VerifyPassphrase { purpose, passphrase }),
                None => log::warn!("unknown prompt purpose: {purpose}"),
            }
        }
    }
}

/// IPC worker: executes queued commands and polls GetStatus once a second.
fn worker_main(shared: Arc<Shared>) {
    let mut last_poll = Instant::now() - Duration::from_secs(1);
    loop {
        let commands: Vec<Command> =
            std::mem::take(&mut *shared.commands.lock().unwrap());
        for command in commands {
            handle_command(&shared, command);
        }
        if last_poll.elapsed() >= Duration::from_secs(1) {
            last_poll = Instant::now();
            poll_status(&shared);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn poll_status(shared: &Shared) {
    let status = match ipc::request(Request::GetStatus) {
        Ok(Response::Status(status)) => Some(status),
        Ok(_) => None,
        Err(err) => {
            log::debug!("GetStatus failed: {err}");
            None
        }
    };
    *shared.last_status.lock().unwrap() = status;
    shared.version.fetch_add(1, Ordering::SeqCst);
}

fn handle_command(shared: &Shared, command: Command) {
    match command {
        Command::SetEnabled(value) => match ipc::request(Request::SetEnabled(value)) {
            Ok(Response::Ok) => {
                log::info!("timer {}", if value { "enabled" } else { "disabled" });
                if let Some(status) = shared.last_status.lock().unwrap().as_mut() {
                    status.enabled = value;
                }
                shared.config.lock().unwrap().enabled = value;
                shared.version.fetch_add(1, Ordering::SeqCst);
                shared.push(Downlink::Config(PageConfig::from(
                    &*shared.config.lock().unwrap(),
                )));
            }
            Ok(other) => {
                log::warn!("SetEnabled -> {other:?}");
                shared.version.fetch_add(1, Ordering::SeqCst);
            }
            Err(err) => {
                log::error!("SetEnabled failed: {err}");
                shared.version.fetch_add(1, Ordering::SeqCst);
            }
        },
        Command::ForceBreak => match ipc::request(Request::ForceBreak) {
            Ok(Response::Ok) => log::info!("break forced"),
            Ok(other) => log::warn!("ForceBreak -> {other:?}"),
            Err(err) => log::error!("ForceBreak failed: {err}"),
        },
        Command::UpdateConfig(draft, passphrase_change) => {
            // Validate first so a rejected passphrase never reaches the
            // daemon and the old passphrase never leaves this process.
            if let Some(change) = &passphrase_change {
                if let Err(err) =
                    settings::validate_passphrase_change(&shared.config.lock().unwrap(), change)
                {
                    log::warn!("passphrase change rejected: {err}");
                    shared.push(Downlink::SaveError(err));
                    return;
                }
            }
            let mut config = settings::apply_draft(&shared.config.lock().unwrap(), &draft);
            if let Some(change) = &passphrase_change {
                config.set_passphrase(&change.new);
            }
            config.validate();
            let stored = config.clone();
            match ipc::request(Request::UpdateConfig(Box::new(config))) {
                Ok(Response::Ok) => {
                    log::info!("config updated");
                    *shared.config.lock().unwrap() = stored;
                    shared.version.fetch_add(1, Ordering::SeqCst);
                    // Echo the authoritative config back so the page shows
                    // clamped values and the menu picks up the change.
                    shared.push(Downlink::Config(PageConfig::from(
                        &*shared.config.lock().unwrap(),
                    )));
                    shared.push(Downlink::SaveOk);
                }
                Ok(other) => {
                    log::warn!("UpdateConfig -> {other:?}");
                    shared.push(Downlink::SaveError("保存设置失败".into()));
                }
                Err(err) => {
                    log::error!("UpdateConfig failed: {err}");
                    shared.push(Downlink::SaveError(format!("无法连接服务: {err}")));
                }
            }
        }
        Command::ToggleDaemonAutostart(enable) => {
            // The daemon owns its Run entry: persist the flag and push
            // UpdateConfig; the daemon applies the registry change.
            let mut config = shared.config.lock().unwrap().clone();
            config.autostart_daemon = enable;
            if let Err(err) = save_config(&config) {
                log::error!("save config (autostart_daemon): {err}");
                return;
            }
            match ipc::request(Request::UpdateConfig(Box::new(config))) {
                Ok(Response::Ok) => log::info!("daemon autostart -> {enable}"),
                Ok(other) => log::warn!("autostart_daemon UpdateConfig -> {other:?}"),
                Err(err) => log::error!("autostart_daemon UpdateConfig failed: {err}"),
            }
            shared.config.lock().unwrap().autostart_daemon = enable;
            shared.version.fetch_add(1, Ordering::SeqCst);
            shared.push(Downlink::Config(PageConfig::from(
                &*shared.config.lock().unwrap(),
            )));
        }
        Command::ToggleTrayAutostart(enable) => {
            let mut config = shared.config.lock().unwrap().clone();
            config.autostart_tray = enable;
            if let Err(err) = save_config(&config) {
                log::error!("save config (autostart_tray): {err}");
            }
            match autostart::set_enabled(enable) {
                Ok(()) => log::info!("tray autostart -> {enable}"),
                Err(err) => log::error!("write tray autostart value: {err}"),
            }
            shared.config.lock().unwrap().autostart_tray = enable;
            shared.version.fetch_add(1, Ordering::SeqCst);
            shared.push(Downlink::Config(PageConfig::from(
                &*shared.config.lock().unwrap(),
            )));
        }
        Command::VerifyPassphrase { purpose, passphrase } => {
            match ipc::request(Request::VerifyPassphrase { passphrase: passphrase.clone() }) {
                Ok(Response::Ok) => {
                    log::info!("passphrase gate passed for {:?}", purpose);
                    match purpose {
                        PromptPurpose::DisableTimer => {
                            match ipc::request(Request::SetEnabled(false)) {
                                Ok(Response::Ok) => {
                                    shared.config.lock().unwrap().enabled = false;
                                    shared.version.fetch_add(1, Ordering::SeqCst);
                                    shared.push(Downlink::PromptDone);
                                }
                                Ok(other) => {
                                    log::warn!("SetEnabled(false) -> {other:?}");
                                    shared.push(Downlink::PromptError { error: "停用失败".into() });
                                }
                                Err(err) => {
                                    log::error!("SetEnabled(false) failed: {err}");
                                    shared.push(Downlink::PromptError { error: "无法连接服务".into() });
                                }
                            }
                        }
                        PromptPurpose::OpenSettings => {
                            // Gate passed: reveal the settings content.
                            let (config, status) = shared.page_state();
                            shared.push(Downlink::Open { config, status });
                        }
                        PromptPurpose::Quit => {
                            // Tray exit only; the daemon keeps running.
                            log::info!("tray exit authorised");
                            std::process::exit(0);
                        }
                        PromptPurpose::StopService => {
                            match ipc::request(Request::StopService { passphrase }) {
                                Ok(Response::Ok) => {
                                    log::info!("service stopped; exiting tray");
                                    std::process::exit(0);
                                }
                                Ok(Response::Denied) => {
                                    shared.push(Downlink::PromptError { error: "口令错误".into() });
                                }
                                Ok(other) => {
                                    log::warn!("StopService -> {other:?}");
                                    shared.push(Downlink::PromptError { error: "停止服务失败".into() });
                                }
                                Err(err) => {
                                    log::error!("StopService failed: {err}");
                                    shared.push(Downlink::PromptError { error: "无法连接服务".into() });
                                }
                            }
                        }
                    }
                }
                Ok(Response::Denied) => {
                    shared.push(Downlink::PromptError { error: "口令错误".into() });
                }
                Ok(other) => {
                    log::warn!("VerifyPassphrase -> {other:?}");
                    shared.push(Downlink::PromptError { error: "校验失败".into() });
                }
                Err(err) => {
                    log::error!("VerifyPassphrase failed: {err}");
                    shared.push(Downlink::PromptError { error: "无法连接服务".into() });
                }
            }
        }
    }
}
