use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Pos2, Vec2};

use crate::config::Config;
use crate::engine::{self, EngineHandle};
use crate::events::{EngineEvent, TrayAction, UiEvent};
use crate::hotkey::parse_hotkey;
use crate::llm;
use crate::overlay::{Overlay, OverlayKind, WINDOW_HEIGHT};
use crate::platform::{self, Platform, TrayState};
use crate::settings::{self, SettingsAction, SettingsState};

const SETTINGS_VIEWPORT_NAME: &str = "voxkey-settings";
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../assets/voxkey.ico");

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
    settings: SettingsState,
    last_frame: Instant,
    exiting: bool,
}

impl VoxKeyApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: Config,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        install_fonts_and_style(&cc.egui_ctx);

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
            TRAY_ICON_BYTES,
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
            settings: SettingsState::from_config(&Config::default()),
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
                        Ok(reply) => (true, format!("连接成功：{reply}")),
                        Err(err) => (false, format!("测试失败：{err}")),
                    });
                }
                UiEvent::PlatformError(err) => {
                    self.overlay.present(
                        OverlayKind::Error,
                        "VoxKey 启动异常",
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
                self.overlay.set_detail("模型尚未就绪，请稍候…");
            } else {
                self.overlay.present(
                    OverlayKind::Error,
                    "语音模型尚未就绪",
                    "正在准备本地模型，请稍后再试",
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
            .unwrap_or_else(|_| "快捷键".to_string());
        self.overlay.present(
            OverlayKind::Listening,
            "正在聆听…",
            format!("按住 {key} 说话，松开后插入"),
            None,
        );
    }

    fn on_hotkey_up(&mut self) {
        if !self.session_active || self.exiting {
            return;
        }
        self.engine.stop();
        if self.overlay.title == "正在聆听…" || self.overlay.title.is_empty() {
            self.overlay.title = "正在识别…".to_string();
        }
        self.overlay.kind = OverlayKind::Working;
        self.overlay.set_detail("正在识别…");
    }

    fn on_tray_action(&mut self, ctx: &egui::Context, action: TrayAction) {
        match action {
            TrayAction::OpenSettings => {
                if !self.show_settings {
                    self.settings.reset_from(&self.config);
                    self.show_settings = true;
                }
                self.focus_settings = true;
                ctx.request_repaint();
            }
            TrayAction::ToggleLlm => {
                self.config.llm_enabled = !self.config.llm_enabled;
                self.apply_tray_state();
                if let Err(err) = self.config.save() {
                    self.overlay.present(
                        OverlayKind::Error,
                        "无法保存设置",
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
                            "无法保存设置",
                            err,
                            Some(Duration::from_secs(5)),
                        );
                    }
                }
            }
            TrayAction::Quit => self.request_quit(ctx),
        }
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
                if self.overlay.kind == OverlayKind::Download {
                    self.overlay.schedule_hide(Duration::from_millis(650));
                }
            }
            EngineEvent::Status(text) => {
                if self.session_active {
                    self.overlay.set_detail(text);
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
                let percent = if total > 0 {
                    (downloaded as f64 / total as f64 * 100.0).round() as u32
                } else {
                    0
                };
                let detail = if total > 0 {
                    format!(
                        "{label} · {percent}% · {:.1} / {:.1} MB",
                        downloaded as f64 / 1_000_000.0,
                        total as f64 / 1_000_000.0
                    )
                } else {
                    format!("{label} · {:.1} MB", downloaded as f64 / 1_000_000.0)
                };
                self.overlay.present(
                    OverlayKind::Download,
                    "正在下载本地语音模型",
                    detail,
                    None,
                );
            }
            EngineEvent::RecordingStarted => {
                self.overlay.kind = OverlayKind::Listening;
                self.overlay.set_detail("正在聆听… 松开快捷键结束");
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
                    self.overlay.set_detail("正在聆听… 松开快捷键结束");
                }
            }
            EngineEvent::Recognizing => {
                if !self.session_active {
                    return;
                }
                let title = if self.overlay.title.is_empty()
                    || self.overlay.title == "正在聆听…"
                {
                    "正在识别…".to_string()
                } else {
                    self.overlay.title.clone()
                };
                self.overlay
                    .present(OverlayKind::Working, title, "正在识别语音…", None);
            }
            EngineEvent::Refining => {
                self.overlay
                    .present(OverlayKind::Working, self.overlay.title.clone(), "LLM 纠错中…", None);
            }
            EngineEvent::Done { text, warning } => {
                self.session_active = false;
                let detail = warning.unwrap_or_else(|| "已插入当前输入框".to_string());
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
                    "未能插入文字",
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
                "API Base URL 必须以 http:// 或 https:// 开头".to_string(),
            ));
            return;
        }
        if draft.model.is_empty() {
            self.settings.message = Some((false, "Model 不能为空".to_string()));
            return;
        }
        let spec = match parse_hotkey(&draft.hotkey) {
            Ok(spec) => spec,
            Err(err) => {
                self.settings.message = Some((false, format!("快捷键无效：{err}")));
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
                    "已保存。API Key 已使用 Windows DPAPI 加密。".to_string(),
                ));
            }
            Err(err) => {
                self.settings.message = Some((false, format!("保存失败：{err}")));
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
                    Err(err) => Err(format!("无法创建网络运行时: {err}")),
                };
                let _ = tx.send(UiEvent::LlmTestResult(result));
                ctx.request_repaint();
            });
        if let Err(err) = spawn {
            self.settings.testing = false;
            self.settings.message = Some((false, format!("无法启动测试线程: {err}")));
        }
    }

    fn draw_settings(&mut self, ctx: &egui::Context) -> SettingsAction {
        let viewport_id = egui::ViewportId::from_hash_of(SETTINGS_VIEWPORT_NAME);
        let builder = egui::ViewportBuilder::default()
            .with_title("VoxKey 设置")
            .with_inner_size([520.0, 690.0])
            .with_min_inner_size([440.0, 540.0])
            .with_resizable(true)
            .with_decorations(true);

        let mut close_requested = false;
        let action = ctx.show_viewport_immediate(viewport_id, builder, |ctx, _class| {
            if ctx.input(|input| input.viewport().close_requested()) {
                close_requested = true;
            }
            settings::draw(ctx, &mut self.settings)
        });

        if self.focus_settings {
            ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Focus);
            self.focus_settings = false;
        }

        if close_requested {
            self.show_settings = false;
            ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Close);
        }

        action
    }

    fn configure_overlay(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if self.overlay_configured {
            return;
        }

        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = frame.window_handle() {
            if let RawWindowHandle::Win32(win32) = handle.as_raw() {
                let raw = win32.hwnd.get();
                let hwnd = windows::Win32::Foundation::HWND(raw as *mut std::ffi::c_void);
                platform::configure_overlay_window(hwnd);
                self.overlay_hwnd_raw = Some(raw);
                self.overlay_configured = true;
                return;
            }
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
            let (left, top, work_w, work_h) = platform::primary_work_area()
                .unwrap_or((0, 0, 1920, 1080));
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
            let (left, top, work_w, work_h) = platform::primary_work_area()
                .unwrap_or((0, 0, 1920, 1080));
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
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().clamp(0.0, 0.05);
        self.last_frame = now;

        self.configure_overlay(ctx, frame);
        self.process_events(ctx);
        if self.exiting {
            return;
        }

        let closed = self.overlay.advance(dt);
        if closed && self.overlay_os_visible {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.overlay_os_visible = false;
            self.previous_foreground = None;
        }

        // winit may rewrite extended window styles when it shows the window.
        // Reapply the non-activating style for a few frames after each show
        // and put keyboard focus back on the app that was focused before.
        if self.overlay_os_visible && self.overlay_style_refresh > 0 {
            if let Some(raw) = self.overlay_hwnd_raw {
                let hwnd = windows::Win32::Foundation::HWND(raw as *mut std::ffi::c_void);
                platform::configure_overlay_window(hwnd);
                if platform::foreground_window() == Some(hwnd) {
                    if let Some(previous) = self.previous_foreground {
                        if previous != raw {
                            platform::set_foreground_window(
                                windows::Win32::Foundation::HWND(
                                    previous as *mut std::ffi::c_void,
                                ),
                            );
                        }
                    }
                }
            }
            self.overlay_style_refresh = self.overlay_style_refresh.saturating_sub(1);
        }

        self.update_overlay_window(ctx);

        if self.overlay.is_visible() {
            self.overlay.render(ctx);
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        if self.show_settings {
            let action = self.draw_settings(ctx);
            if action.test {
                self.start_llm_test();
            }
            if action.save {
                self.save_settings();
            }
            if self.settings.testing {
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

fn install_fonts_and_style(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "voxkey_cjk".to_string(),
        std::sync::Arc::new(FontData::from_static(include_bytes!(
            "../assets/NotoSansCJKsc-Regular.otf"
        ))),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "voxkey_cjk".to_string());
    }
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = Color32::from_rgb(18, 20, 27);
    style.visuals.window_fill = Color32::from_rgb(18, 20, 27);
    style.visuals.extreme_bg_color = Color32::from_rgb(12, 14, 19);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(34, 38, 49);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(44, 49, 63);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(54, 60, 77);
    style.visuals.selection.bg_fill = Color32::from_rgb(53, 101, 79);
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    ctx.set_style(style);

    ctx.style_mut(|style| {
        style.text_styles.insert(
            egui::TextStyle::Body,
            FontId::proportional(13.0),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            FontId::proportional(13.0),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            FontId::proportional(10.5),
        );
    });
}
