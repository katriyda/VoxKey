use std::path::Path;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Pos2, Vec2};

use crate::i18n::{trf, ts};

use crate::config::Config;
use crate::engine::{self, EngineHandle};
use crate::events::{EngineEvent, TrayAction, UiEvent, UpdateEvent};
use crate::hotkey::parse_hotkey;
use crate::llm;
use crate::logging;
use crate::model::{self, ImportOutcome};
use crate::overlay::{Overlay, OverlayKind, WINDOW_HEIGHT};
use crate::platform::{self, Platform, TrayState};
use crate::settings::{self, SettingsAction, SettingsState};
use crate::update::{self, UpdateUiState};

const SETTINGS_VIEWPORT_NAME: &str = "voxkey-settings";
fn settings_window_title() -> &'static str {
    ts!("VoxKey 设置", "VoxKey Settings")
}

pub struct VoxKeyApp {
    ctx: egui::Context,
    config: Config,
    ui_tx: Sender<UiEvent>,
    ui_rx: Receiver<UiEvent>,
    engine: EngineHandle,
    platform: Platform,
    engine_ready: bool,
    session_active: bool,
    overlay: Overlay,
    overlay_os_visible: bool,
    overlay_width: f32,
    overlay_sent_width: f32,
    overlay_configured: bool,
    overlay_hwnd_raw: Option<isize>,
    overlay_style_refresh: u8,
    previous_foreground: Option<isize>,
    configure_attempts: u32,
    show_settings: bool,
    focus_settings: bool,
    settings_icon_applied: bool,
    settings: SettingsState,
    update: UpdateUiState,
    importing_model: bool,
    download_sample: Option<(Instant, u64)>,
    download_rate: f32,
    last_frame: Instant,
    exiting: bool,
}

impl VoxKeyApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: Config,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let cjk_available = crate::fonts::install(&cc.egui_ctx);
        crate::i18n::apply(config.ui_language, cjk_available);

        let (ui_tx, ui_rx) = std::sync::mpsc::channel();
        let engine = engine::spawn(config.language, ui_tx.clone(), cc.egui_ctx.clone())
            .map_err(std::io::Error::other)?;

        let hotkey = parse_hotkey(&config.hotkey).unwrap_or_else(|_| {
            parse_hotkey(crate::hotkey::DEFAULT_HOTKEY).expect("default hotkey is valid")
        });
        let tray_state = TrayState {
            language: config.language,
            llm_enabled: config.llm_enabled,
        };
        let platform = Platform::start(
            cc.egui_ctx.clone(),
            ui_tx.clone(),
            hotkey,
            tray_state,
            platform::APP_ICON_BYTES,
        );

        Ok(Self {
            ctx: cc.egui_ctx.clone(),
            config,
            ui_tx,
            ui_rx,
            engine,
            platform,
            engine_ready: false,
            session_active: false,
            overlay: Overlay::default(),
            overlay_os_visible: false,
            overlay_width: 360.0,
            overlay_sent_width: 0.0,
            overlay_configured: false,
            overlay_hwnd_raw: None,
            overlay_style_refresh: 0,
            previous_foreground: None,
            configure_attempts: 0,
            show_settings: false,
            focus_settings: false,
            settings_icon_applied: false,
            settings: SettingsState::from_config(&Config::default()),
            update: UpdateUiState::default(),
            importing_model: false,
            download_sample: None,
            download_rate: 0.0,
            last_frame: Instant::now(),
            exiting: false,
        })
    }

    fn process_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.ui_rx.try_recv() {
            match event {
                UiEvent::HotkeyDown => self.on_hotkey_down(),
                UiEvent::HotkeyUp => self.on_hotkey_up(),
                UiEvent::Tray(action) => self.on_tray_action(ctx, action),
                UiEvent::Engine(event) => self.on_engine_event(ctx, event),
                UiEvent::LlmTestResult(result) => {
                    self.settings.testing = false;
                    self.settings.message = Some(match result {
                        Ok(reply) => (true, trf!("连接成功：{reply}", "Connected: {reply}")),
                        Err(err) => (false, trf!("测试失败：{err}", "Test failed: {err}")),
                    });
                }
                UiEvent::Update(event) => self.on_update_event(ctx, event),
                UiEvent::ModelImportDone(result) => self.on_model_import(ctx, result),
                UiEvent::PlatformError(err) => {
                    crate::logging::error(&err);
                    self.overlay.present(
                        OverlayKind::Error,
                        ts!("VoxKey 启动异常", "VoxKey startup error"),
                        err,
                        Some(Duration::from_secs(7)),
                    );
                }
            }
        }
    }

    fn on_hotkey_down(&mut self) {
        if self.exiting || self.session_active {
            return;
        }
        if !self.engine_ready {
            if self.overlay.kind == OverlayKind::Download {
                self.overlay.set_detail(ts!(
                    "模型尚未就绪，请稍候…",
                    "The model is not ready yet, please wait…",
                ));
            } else {
                self.overlay.present(
                    OverlayKind::Error,
                    ts!("语音模型尚未就绪", "Speech model not ready"),
                    ts!(
                        "正在准备本地模型，请稍后再试",
                        "Preparing the local model, please try again shortly",
                    ),
                    Some(Duration::from_secs(2)),
                );
            }
            return;
        }

        let refine = llm::RefineConfig::from_config(&self.config);
        self.engine.start(self.config.language, refine);
        self.session_active = true;
        let key = parse_hotkey(&self.config.hotkey)
            .map(|spec| spec.label)
            .unwrap_or_else(|_| ts!("快捷键", "hotkey").to_string());
        self.overlay.present(
            OverlayKind::Listening,
            ts!("正在聆听…", "Listening…"),
            trf!(
                "按住 {key} 说话，松开后插入",
                "Hold {key} to speak, release to insert"
            ),
            None,
        );
    }

    fn on_hotkey_up(&mut self) {
        if !self.session_active || self.exiting {
            return;
        }
        self.engine.stop();
        if self.overlay.kind == OverlayKind::Listening || self.overlay.title.is_empty() {
            self.overlay.title = ts!("正在识别…", "Recognizing…").to_string();
        }
        self.overlay.kind = OverlayKind::Working;
        self.overlay.set_detail(ts!("正在识别…", "Recognizing…"));
    }

    fn on_tray_action(&mut self, ctx: &egui::Context, action: TrayAction) {
        match action {
            TrayAction::OpenSettings => self.open_settings(ctx),
            TrayAction::CheckUpdate => {
                self.open_settings(ctx);
                self.start_update_check();
            }
            TrayAction::ImportModel => self.start_model_import(),
            TrayAction::OpenModelDir => {
                let dir = model::model_dir();
                let path = dir.display().to_string();
                platform::open_url(&path);
                self.overlay.present(
                    OverlayKind::Success,
                    ts!("已打开模型目录", "Model folder opened"),
                    path,
                    Some(Duration::from_secs(4)),
                );
            }
            TrayAction::ReloadModel => self.reload_model(),
            TrayAction::ToggleLlm => {
                self.config.llm_enabled = !self.config.llm_enabled;
                self.apply_tray_state();
                if let Err(err) = self.config.save() {
                    self.overlay.present(
                        OverlayKind::Error,
                        ts!("无法保存设置", "Cannot save settings"),
                        err,
                        Some(Duration::from_secs(5)),
                    );
                }
            }
            TrayAction::SetLanguage(language) => {
                if self.config.language != language {
                    self.config.language = language;
                    self.apply_tray_state();
                    if let Err(err) = self.config.save() {
                        self.overlay.present(
                            OverlayKind::Error,
                            ts!("无法保存设置", "Cannot save settings"),
                            err,
                            Some(Duration::from_secs(5)),
                        );
                    }
                }
            }
            TrayAction::Quit => self.request_quit(ctx),
        }
    }

    fn open_settings(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            self.settings.reset_from(&self.config);
            self.show_settings = true;
            // A reopened settings viewport gets a brand new native window.
            self.settings_icon_applied = false;
        }
        self.focus_settings = true;
        ctx.request_repaint();
    }

    fn start_update_check(&mut self) {
        if self.update.checking || self.update.downloading {
            return;
        }
        self.update.checking = true;
        self.update.info = None;
        self.update.progress = None;
        self.update.message = None;

        let tx = self.ui_tx.clone();
        let ctx = self.ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("voxkey-update-check".to_string())
            .spawn(move || {
                let result = update::check_blocking();
                let _ = tx.send(UiEvent::Update(UpdateEvent::CheckFinished(result)));
                ctx.request_repaint();
            });
        if let Err(err) = spawn {
            self.update.checking = false;
            self.update.message = Some((
                false,
                trf!(
                    "无法启动更新检查：{err}",
                    "Cannot start the update check: {err}"
                ),
            ));
        }
    }

    fn start_update_download(&mut self) {
        let Some(info) = self.update.info.clone() else {
            return;
        };
        if self.update.downloading {
            return;
        }
        self.update.downloading = true;
        self.update.progress = Some((0, info.asset_size));
        self.update.message = None;
        self.ctx.request_repaint();

        let tx = self.ui_tx.clone();
        let ctx = self.ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("voxkey-update-download".to_string())
            .spawn(move || {
                let mut report = |downloaded: u64, total: u64| {
                    let _ = tx.send(UiEvent::Update(UpdateEvent::Progress { downloaded, total }));
                    ctx.request_repaint();
                };
                let event = match update::download_blocking(&info, &mut report) {
                    Ok(path) => UpdateEvent::Downloaded(path),
                    Err(err) => UpdateEvent::Failed(err),
                };
                let _ = tx.send(UiEvent::Update(event));
                ctx.request_repaint();
            });
        if let Err(err) = spawn {
            self.update.downloading = false;
            self.update.progress = None;
            self.update.message = Some((
                false,
                trf!(
                    "无法启动更新下载：{err}",
                    "Cannot start the update download: {err}"
                ),
            ));
        }
    }

    fn on_update_event(&mut self, ctx: &egui::Context, event: UpdateEvent) {
        match event {
            UpdateEvent::CheckFinished(Ok(None)) => {
                self.update.checking = false;
                self.update.info = None;
                self.update.message = Some((
                    true,
                    trf!(
                        "已是最新版本 v{}",
                        "Already up to date: v{}",
                        update::current_version()
                    ),
                ));
            }
            UpdateEvent::CheckFinished(Ok(Some(info))) => {
                self.update.checking = false;
                self.update.message = None;
                self.update.info = Some(info);
            }
            UpdateEvent::CheckFinished(Err(err)) => {
                self.update.checking = false;
                self.update.message = Some((
                    false,
                    trf!("检查更新失败：{err}", "Update check failed: {err}"),
                ));
            }
            UpdateEvent::Progress { downloaded, total } => {
                self.update.progress = Some((downloaded, total));
            }
            UpdateEvent::Downloaded(path) => {
                self.update.downloading = false;
                self.update.progress = None;
                self.apply_update(ctx, &path);
            }
            UpdateEvent::Failed(err) => {
                self.update.downloading = false;
                self.update.progress = None;
                self.update.message = Some((
                    false,
                    trf!("下载更新失败：{err}", "Update download failed: {err}"),
                ));
            }
        }
        ctx.request_repaint();
    }

    fn apply_update(&mut self, ctx: &egui::Context, path: &Path) {
        match update::install_and_restart(path) {
            Ok(()) => {
                crate::logging::info("已启动新版本，准备退出旧进程");
                self.update.message = Some((
                    true,
                    ts!(
                        "新版本已启动，VoxKey 即将重启…",
                        "The new version has started; VoxKey is restarting…"
                    )
                    .to_string(),
                ));
                self.request_quit(ctx);
            }
            Err(err) => {
                self.update.message = Some((
                    false,
                    trf!("安装更新失败：{err}", "Cannot install the update: {err}"),
                ));
            }
        }
    }

    /// Smooths download speed for the overlay so users can tell a slow transfer
    /// from a stalled one.
    fn track_download_rate(&mut self, downloaded: u64) -> f32 {
        let now = Instant::now();
        let (rate, sample) = match self.download_sample {
            Some((_, bytes)) if downloaded < bytes => (0.0f32, (now, downloaded)),
            Some((at, bytes)) => {
                let elapsed = now.duration_since(at).as_secs_f32();
                if elapsed < 0.2 {
                    (self.download_rate, (at, bytes))
                } else {
                    let instant = (downloaded - bytes) as f32 / 1_000_000.0 / elapsed;
                    let smoothed = if self.download_rate <= 0.05 {
                        instant
                    } else {
                        self.download_rate * 0.6 + instant * 0.4
                    };
                    (smoothed, (now, downloaded))
                }
            }
            None => (0.0f32, (now, downloaded)),
        };
        self.download_sample = Some(sample);
        self.download_rate = rate;
        rate
    }

    /// Re-runs model preparation inside the engine thread (after an import, a
    /// failed download or a manual file change).
    fn reload_model(&mut self) {
        self.engine_ready = false;
        self.download_sample = None;
        self.download_rate = 0.0;
        self.overlay.present(
            OverlayKind::Download,
            ts!("正在准备本地语音模型", "Preparing the local speech model"),
            ts!("重新检查模型文件…", "Re-checking the model files…"),
            None,
        );
        self.engine.reload_model();
    }

    /// Runs the offline import on its own thread: hashing 239 MB must not block
    /// the UI thread that also drives the hotkey hook.
    fn start_model_import(&mut self) {
        if self.importing_model {
            return;
        }
        self.importing_model = true;
        self.overlay.present(
            OverlayKind::Download,
            ts!("正在导入本地模型", "Importing the local model"),
            ts!(
                "校验文件大小与 SHA-256…",
                "Verifying file sizes and SHA-256…"
            ),
            None,
        );
        self.settings.model_message = None;

        let tx = self.ui_tx.clone();
        let ctx = self.ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("voxkey-model-import".to_string())
            .spawn(move || {
                let mut report = |downloaded: u64, total: u64, label: &str| {
                    let _ = tx.send(UiEvent::Engine(EngineEvent::Download {
                        downloaded,
                        total,
                        label: label.to_string(),
                    }));
                    ctx.request_repaint();
                };
                let result = model::import_from_inbox(&mut report);
                let _ = tx.send(UiEvent::ModelImportDone(result));
                ctx.request_repaint();
            });
        if let Err(err) = spawn {
            self.importing_model = false;
            self.settings.model_message = Some((
                false,
                trf!(
                    "无法启动导入线程：{err}",
                    "Cannot start the import thread: {err}"
                ),
            ));
        }
    }

    fn on_model_import(&mut self, ctx: &egui::Context, result: Result<ImportOutcome, String>) {
        self.importing_model = false;
        match result {
            Ok(ImportOutcome::Imported { message }) => {
                logging::info(&format!("离线模型导入完成：{message}"));
                self.settings.model_message = Some((true, message));
                self.engine_ready = false;
                self.download_sample = None;
                self.download_rate = 0.0;
                self.overlay.present(
                    OverlayKind::Download,
                    ts!("正在加载语音模型", "Loading the speech model"),
                    ts!(
                        "本地模型已导入，正在加载…",
                        "Local model imported, loading…"
                    ),
                    None,
                );
                self.engine.reload_model();
            }
            Ok(ImportOutcome::Empty { dir }) => {
                let path = dir.display().to_string();
                platform::open_url(&path);
                let message = trf!(
                    "已打开导入目录 {}。把 {} 与 {} 放进去，再从托盘选择“导入本地模型”。",
                    "Opened the import folder {}. Put {} and {} there, then choose “Import local model” from the tray menu.",
                    path,
                    model::MODEL_FILE,
                    model::TOKENS_FILE
                );
                logging::info(&message);
                self.settings.model_message = Some((false, message));
                self.overlay.present(
                    OverlayKind::Error,
                    ts!("还没有可导入的模型", "No model to import yet"),
                    ts!(
                        "已打开导入目录，放入文件后再导入一次",
                        "The import folder was opened; add the files and import again"
                    ),
                    Some(Duration::from_secs(6)),
                );
            }
            Err(err) => {
                logging::error(&format!("离线模型导入失败：{err}"));
                self.settings.model_message = Some((false, err));
                self.overlay.present(
                    OverlayKind::Error,
                    ts!("本地模型导入失败", "Local model import failed"),
                    ts!(
                        "详细原因见设置窗口或 voxkey.log",
                        "See the settings window or voxkey.log for details"
                    ),
                    Some(Duration::from_secs(8)),
                );
            }
        }
        ctx.request_repaint();
    }

    fn apply_tray_state(&mut self) {
        self.platform.set_tray_state(TrayState {
            language: self.config.language,
            llm_enabled: self.config.llm_enabled,
        });
    }

    fn on_engine_event(&mut self, _ctx: &egui::Context, event: EngineEvent) {
        match event {
            EngineEvent::Ready => {
                self.engine_ready = true;
                self.download_sample = None;
                self.download_rate = 0.0;
                if self.overlay.kind == OverlayKind::Download {
                    self.overlay.schedule_hide(Duration::from_millis(650));
                }
            }
            EngineEvent::Status(text) => {
                if self.session_active {
                    self.overlay.set_detail(text);
                } else if self.overlay.kind == OverlayKind::Download {
                    // "下载完成但还在加载" must not look like a frozen 100%.
                    self.overlay.set_detail(&text);
                }
            }
            EngineEvent::Download {
                downloaded,
                total,
                label,
            } => {
                if self.session_active {
                    return;
                }
                let rate = self.track_download_rate(downloaded);
                // Floor and cap at 99 until the whole model is ready: a rounded
                // 100% while the second file is still missing was exactly what
                // made users think the download had hung.
                let percent = downloaded
                    .saturating_mul(100)
                    .checked_div(total)
                    .map(|value| (value as u32).min(if downloaded >= total { 100 } else { 99 }))
                    .unwrap_or(0);
                let mut detail = if total > 0 {
                    format!(
                        "{label} · {percent}% · {:.1} / {:.1} MB",
                        downloaded as f64 / 1_000_000.0,
                        total as f64 / 1_000_000.0
                    )
                } else {
                    format!("{label} · {:.1} MB", downloaded as f64 / 1_000_000.0)
                };
                if rate > 0.05 {
                    detail.push_str(&format!(" · {rate:.1} MB/s"));
                }
                self.overlay.present(
                    OverlayKind::Download,
                    ts!("正在准备本地语音模型", "Preparing the local speech model"),
                    detail,
                    None,
                );
            }
            EngineEvent::ModelError(err) => {
                self.engine_ready = false;
                self.session_active = false;
                self.download_sample = None;
                self.download_rate = 0.0;
                // The overlay can only show one short line; the full reason
                // stays in the settings window and in voxkey.log.
                self.settings.model_message = Some((false, err));
                self.overlay.present(
                    OverlayKind::Error,
                    ts!("本地语音模型未就绪", "The local speech model is not ready"),
                    ts!(
                        "可手动下载后从托盘“导入本地模型”（见 docs/offline-model.md）",
                        "You can download it manually and use “Import local model” in the tray menu (see docs/offline-model.md)"
                    ),
                    Some(Duration::from_secs(20)),
                );
            }
            EngineEvent::RecordingStarted => {
                self.overlay.kind = OverlayKind::Listening;
                self.overlay.set_detail(ts!(
                    "正在聆听… 松开快捷键结束",
                    "Listening… release the hotkey to finish"
                ));
            }
            EngineEvent::Level(level) => {
                if self.session_active {
                    self.overlay.set_level(level);
                }
            }
            EngineEvent::Transcript(text) => {
                if self.session_active {
                    self.overlay.kind = OverlayKind::Listening;
                    self.overlay.set_transcript(text);
                    self.overlay.set_detail(ts!(
                        "正在聆听… 松开快捷键结束",
                        "Listening… release the hotkey to finish"
                    ));
                }
            }
            EngineEvent::Recognizing => {
                if !self.session_active {
                    return;
                }
                let title = if self.overlay.title.is_empty()
                    || self.overlay.kind == OverlayKind::Listening
                {
                    ts!("正在识别…", "Recognizing…").to_string()
                } else {
                    self.overlay.title.clone()
                };
                self.overlay.present(
                    OverlayKind::Working,
                    title,
                    ts!("正在识别语音…", "Recognizing speech…"),
                    None,
                );
            }
            EngineEvent::Refining => {
                self.overlay.present(
                    OverlayKind::Working,
                    self.overlay.title.clone(),
                    ts!("LLM 纠错中…", "LLM cleanup running…"),
                    None,
                );
            }
            EngineEvent::Done { text, warning } => {
                self.session_active = false;
                let detail = warning.unwrap_or_else(|| {
                    ts!("已插入当前输入框", "Inserted into the focused field").to_string()
                });
                self.overlay.present(
                    OverlayKind::Success,
                    text,
                    detail,
                    Some(Duration::from_millis(1400)),
                );
            }
            EngineEvent::Error(err) => {
                self.session_active = false;
                self.overlay.present(
                    OverlayKind::Error,
                    ts!("未能插入文字", "Could not insert the text"),
                    err,
                    Some(Duration::from_secs(5)),
                );
            }
        }
    }

    fn request_quit(&mut self, ctx: &egui::Context) {
        if self.exiting {
            return;
        }
        self.exiting = true;
        self.engine.shutdown();
        self.platform.shutdown();
        let settings_id = egui::ViewportId::from_hash_of(SETTINGS_VIEWPORT_NAME);
        ctx.send_viewport_cmd_to(settings_id, egui::ViewportCommand::Close);
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);

        // eframe normally tears itself down promptly after the root Close
        // command. A hidden root viewport plus a transient secondary window can
        // delay that shutdown on some Windows versions, so guarantee termination
        // after all Win32 resources have already been cleaned up above.
        let _ = std::thread::Builder::new()
            .name("voxkey-exit".to_string())
            .spawn(|| {
                std::thread::sleep(Duration::from_millis(800));
                std::process::exit(0);
            });
    }

    fn save_settings(&mut self) {
        let draft = &mut self.settings.draft;
        draft.api_base_url = draft.api_base_url.trim().to_string();
        draft.model = draft.model.trim().to_string();
        draft.hotkey = draft.hotkey.trim().to_string();

        if !(draft.api_base_url.starts_with("http://")
            || draft.api_base_url.starts_with("https://"))
        {
            self.settings.message = Some((
                false,
                ts!(
                    "API Base URL 必须以 http:// 或 https:// 开头",
                    "API Base URL must start with http:// or https://"
                )
                .to_string(),
            ));
            return;
        }
        if draft.model.is_empty() {
            self.settings.message = Some((
                false,
                ts!("Model 不能为空", "Model must not be empty").to_string(),
            ));
            return;
        }
        let spec = match parse_hotkey(&draft.hotkey) {
            Ok(spec) => spec,
            Err(err) => {
                self.settings.message =
                    Some((false, trf!("快捷键无效：{err}", "Invalid hotkey: {err}")));
                return;
            }
        };

        let next = draft.clone();
        match next.save() {
            Ok(()) => {
                self.config = next.clone();
                self.platform.set_hotkey(spec);
                self.apply_tray_state();
                self.settings.draft = next;
                self.settings.message = Some((
                    true,
                    ts!(
                        "已保存。API Key 已使用 Windows DPAPI 加密。",
                        "Saved. The API key is encrypted with Windows DPAPI."
                    )
                    .to_string(),
                ));
            }
            Err(err) => {
                self.settings.message =
                    Some((false, trf!("保存失败：{err}", "Cannot save: {err}")));
            }
        }
    }

    fn start_llm_test(&mut self) {
        if self.settings.testing {
            return;
        }
        let cfg = llm::RefineConfig {
            enabled: true,
            base_url: self.settings.draft.api_base_url.clone(),
            api_key: self.settings.draft.api_key.clone(),
            model: self.settings.draft.model.clone(),
        };
        self.settings.testing = true;
        self.settings.message = None;
        let tx = self.ui_tx.clone();
        let ctx = self.ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("voxkey-llm-test".to_string())
            .spawn(move || {
                let result = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(llm::test_connection(&cfg)),
                    Err(err) => Err(trf!(
                        "无法创建网络运行时: {err}",
                        "Cannot create the network runtime: {err}"
                    )),
                };
                let _ = tx.send(UiEvent::LlmTestResult(result));
                ctx.request_repaint();
            });
        if let Err(err) = spawn {
            self.settings.testing = false;
            self.settings.message = Some((
                false,
                trf!(
                    "无法启动测试线程: {err}",
                    "Cannot start the test thread: {err}"
                ),
            ));
        }
    }

    fn draw_settings(&mut self, ctx: &egui::Context) -> SettingsAction {
        let viewport_id = egui::ViewportId::from_hash_of(SETTINGS_VIEWPORT_NAME);
        let builder = egui::ViewportBuilder::default()
            .with_title(settings_window_title())
            .with_inner_size([520.0, 780.0])
            .with_min_inner_size([440.0, 560.0])
            .with_resizable(true)
            .with_decorations(true);

        let mut close_requested = false;
        let action = ctx.show_viewport_immediate(viewport_id, builder, |ui, _class| {
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                close_requested = true;
            }
            settings::draw(ui, &mut self.settings, &mut self.update)
        });

        // eframe creates the viewport's native window after the UI callback
        // returns, so the icon is applied on the first frame in which the
        // window can actually be found. The class icons set for the overlay are
        // normally enough; this also covers windows that already exist when the
        // class icon changes.
        if !self.settings_icon_applied
            && platform::apply_icon_to_window_titled(settings_window_title())
        {
            self.settings_icon_applied = true;
        }

        if self.focus_settings {
            ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Focus);
            self.focus_settings = false;
        }

        if close_requested {
            self.show_settings = false;
            self.settings_icon_applied = false;
            // Closing without Save must not leave a previewed language active.
            crate::i18n::reapply(self.config.ui_language);
            ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Close);
        }

        action
    }

    fn configure_overlay(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if self.overlay_configured {
            return;
        }

        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = frame.window_handle()
            && let RawWindowHandle::Win32(win32) = handle.as_raw()
        {
            let raw = win32.hwnd.get();
            let hwnd = windows::Win32::Foundation::HWND(raw as *mut std::ffi::c_void);
            platform::configure_overlay_window(hwnd);
            self.overlay_hwnd_raw = Some(raw);
            self.overlay_configured = true;
            return;
        }

        self.configure_attempts += 1;
        if self.configure_attempts < 100 {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn update_overlay_window(&mut self, ctx: &egui::Context) {
        let desired_width = self.overlay.desired_width(ctx);
        if !self.overlay_os_visible {
            self.overlay_width = desired_width;
        } else {
            self.overlay_width += (desired_width - self.overlay_width) * 0.28;
        }

        if self.exiting {
            return;
        }

        if self.overlay.is_visible() && !self.overlay_os_visible {
            let (left, top, work_w, work_h) =
                platform::primary_work_area().unwrap_or((0, 0, 1920, 1080));
            let ppp = ctx.pixels_per_point();
            let width = self.overlay_width;
            let x = (left as f32 + (work_w as f32 - width * ppp) / 2.0) / ppp;
            let y = (top + work_h) as f32 / ppp - WINDOW_HEIGHT - 34.0;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(
                width,
                WINDOW_HEIGHT,
            )));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(x, y)));
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            self.previous_foreground = platform::foreground_window().map(|hwnd| hwnd.0 as isize);
            self.overlay_style_refresh = 12;
            self.overlay_os_visible = true;
            self.overlay_sent_width = width;
            ctx.request_repaint();
        } else if self.overlay_os_visible
            && (self.overlay_width - self.overlay_sent_width).abs() > 1.0
        {
            let (left, top, work_w, work_h) =
                platform::primary_work_area().unwrap_or((0, 0, 1920, 1080));
            let ppp = ctx.pixels_per_point();
            let width = self.overlay_width;
            let x = (left as f32 + (work_w as f32 - width * ppp) / 2.0) / ppp;
            let y = (top + work_h) as f32 / ppp - WINDOW_HEIGHT - 34.0;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(
                width,
                WINDOW_HEIGHT,
            )));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(x, y)));
            self.overlay_sent_width = width;
            ctx.request_repaint();
        }
    }
}

impl eframe::App for VoxKeyApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().clamp(0.0, 0.05);
        self.last_frame = now;

        self.configure_overlay(&ctx, frame);
        self.process_events(&ctx);
        if self.exiting {
            return;
        }

        let closed = self.overlay.advance(dt);
        if closed && self.overlay_os_visible {
            // The OS window stays visible (and fully transparent) so winit
            // keeps processing hotkey repaint requests. Only the capsule
            // content is hidden.
            self.previous_foreground = None;
        }

        // winit may rewrite extended window styles when it shows the window.
        // Reapply the non-activating style for a few frames after each show
        // and put keyboard focus back on the app that was focused before.
        if self.overlay_os_visible && self.overlay_style_refresh > 0 {
            if let Some(raw) = self.overlay_hwnd_raw {
                let hwnd = windows::Win32::Foundation::HWND(raw as *mut std::ffi::c_void);
                platform::configure_overlay_window(hwnd);
                if platform::foreground_window() == Some(hwnd)
                    && let Some(previous) = self.previous_foreground
                    && previous != raw
                {
                    platform::set_foreground_window(windows::Win32::Foundation::HWND(
                        previous as *mut std::ffi::c_void,
                    ));
                }
            }
            self.overlay_style_refresh = self.overlay_style_refresh.saturating_sub(1);
        }

        self.update_overlay_window(&ctx);

        if self.overlay.is_visible() {
            self.overlay.render(ui);
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        if self.show_settings {
            let action = self.draw_settings(&ctx);
            if action.test {
                self.start_llm_test();
            }
            if action.save {
                self.save_settings();
            }
            if action.check_update {
                self.start_update_check();
            }
            if action.download_update {
                self.start_update_download();
            }
            if action.open_release
                && let Some(info) = &self.update.info
            {
                platform::open_url(&info.page_url);
            }
            if action.import_model {
                self.start_model_import();
            }
            if action.open_model_dir {
                let dir = model::model_dir();
                platform::open_url(&dir.display().to_string());
            }
            if action.reload_model {
                self.reload_model();
            }
            if self.settings.testing {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            if self.update.checking || self.update.downloading {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }

        if self.overlay.is_fading() || self.show_settings {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
}

impl Drop for VoxKeyApp {
    fn drop(&mut self) {
        self.engine.shutdown();
        self.platform.shutdown();
    }
}
