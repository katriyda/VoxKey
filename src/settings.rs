//! Settings window contents.

use egui::{Color32, RichText};

use crate::config::{Config, DEFAULT_API_BASE, DEFAULT_MODEL, Language};
use crate::hotkey::parse_hotkey;
use crate::i18n::{self, UiLanguage, trf, ts};
use crate::update::{self, UpdateUiState};

#[derive(Default)]
pub struct SettingsAction {
    pub test: bool,
    pub save: bool,
    pub check_update: bool,
    pub download_update: bool,
    pub open_release: bool,
    pub import_model: bool,
    pub open_model_dir: bool,
    pub reload_model: bool,
}

pub struct SettingsState {
    pub draft: Config,
    pub show_key: bool,
    pub testing: bool,
    pub message: Option<(bool, String)>,
    pub model_message: Option<(bool, String)>,
}

impl SettingsState {
    pub fn from_config(config: &Config) -> Self {
        Self {
            draft: config.clone(),
            show_key: false,
            testing: false,
            message: None,
            model_message: None,
        }
    }

    pub fn reset_from(&mut self, config: &Config) {
        self.draft = config.clone();
        self.testing = false;
        self.message = None;
    }
}

pub fn draw(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    update_state: &mut UpdateUiState,
) -> SettingsAction {
    let mut action = SettingsAction::default();
    let theme_bg = Color32::from_rgb(18, 20, 27);
    let muted = Color32::from_rgb(151, 159, 176);

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme_bg)
                .inner_margin(egui::Margin::same(20)),
        )
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("VoxKey")
                                .size(24.0)
                                .strong()
                                .color(Color32::from_rgb(124, 231, 174)),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(ts!("本地语音输入", "Local voice input"))
                                .size(12.0)
                                .color(muted),
                        );
                    });
                    ui.label(
                        RichText::new(ts!(
                            "按住快捷键录音，松开后识别并插入到当前输入框。",
                            "Hold the hotkey to record; release to recognize and insert into the focused field.",
                        ))
                        .color(muted)
                        .size(12.0),
                    );
                    ui.add_space(12.0);

                    section(ui, ts!("界面", "Interface"));
                    ui.horizontal(|ui| {
                        ui.label(ts!("界面语言", "Interface language"));
                        let before = state.draft.ui_language;
                        egui::ComboBox::from_id_salt("ui_language")
                            .selected_text(state.draft.ui_language.label())
                            .width(180.0)
                            .show_ui(ui, |ui| {
                                for language in UiLanguage::ALL {
                                    ui.selectable_value(
                                        &mut state.draft.ui_language,
                                        language,
                                        language.label(),
                                    );
                                }
                            });
                        if state.draft.ui_language != before {
                            // Preview immediately; persisted with Save.
                            i18n::reapply(state.draft.ui_language);
                        }
                    });
                    ui.label(
                        RichText::new(ts!(
                            "自动：系统有中文字体时显示中文，否则显示英文。",
                            "Auto: Chinese when the system has a CJK font, English otherwise.",
                        ))
                        .size(10.5)
                        .color(muted),
                    );
                    ui.add_space(12.0);

                    section(ui, ts!("语音", "Voice"));
                    ui.horizontal(|ui| {
                        ui.label(ts!("识别语言", "Recognition language"));
                        egui::ComboBox::from_id_salt("language")
                            .selected_text(state.draft.language.label())
                            .width(160.0)
                            .show_ui(ui, |ui| {
                                for language in Language::ALL {
                                    ui.selectable_value(
                                        &mut state.draft.language,
                                        language,
                                        language.label(),
                                    );
                                }
                            });
                    });
                    ui.add_space(6.0);
                    ui.label(ts!("按住说话快捷键", "Hold-to-talk hotkey"));
                    let preview = parse_hotkey(&state.draft.hotkey);
                    ui.add_sized(
                        [ui.available_width(), 26.0],
                        egui::TextEdit::singleline(&mut state.draft.hotkey)
                            .hint_text(ts!(
                                "right_ctrl 或 ctrl+shift+space",
                                "right_ctrl or ctrl+shift+space",
                            )),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui.small_button(ts!("右 Ctrl", "Right Ctrl")).clicked() {
                            state.draft.hotkey = "right_ctrl".to_string();
                        }
                        if ui
                            .small_button(ts!("Ctrl + Shift + 空格", "Ctrl + Shift + Space"))
                            .clicked()
                        {
                            state.draft.hotkey = "ctrl+shift+space".to_string();
                        }
                        if ui.small_button("Fn / F13").clicked() {
                            state.draft.hotkey = "fn".to_string();
                        }
                        if ui.small_button(ts!("恢复默认", "Reset to default")).clicked() {
                            state.draft.hotkey = crate::hotkey::DEFAULT_HOTKEY.to_string();
                        }
                    });
                    match preview {
                        Ok(spec) => {
                            ui.label(
                                RichText::new(trf!(
                                    "当前组合：{}",
                                    "Current chord: {}",
                                    spec.label
                                ))
                                .size(11.0)
                                .color(Color32::from_rgb(124, 231, 174)),
                            );
                        }
                        Err(err) => {
                            ui.label(
                                RichText::new(trf!(
                                    "快捷键无效：{err}",
                                    "Invalid hotkey: {err}"
                                ))
                                .size(11.0)
                                .color(Color32::from_rgb(255, 150, 140)),
                            );
                        }
                    }
                    ui.label(
                        RichText::new(ts!(
                            "多数笔记本的 Fn 键由键盘固件拦截，Windows 收不到；可改用 F13 映射或其他自定义组合。",
                            "On most laptops the Fn key is intercepted by the keyboard firmware and never reaches Windows; map F13 or use another combination.",
                        ))
                        .size(10.5)
                        .color(muted),
                    );
                    ui.add_space(12.0);

                    section(ui, ts!("LLM 纠错", "LLM cleanup"));
                    ui.checkbox(
                        &mut state.draft.llm_enabled,
                        ts!(
                            "启用保守纠错（只修复明显的语音识别错误）",
                            "Enable conservative cleanup (fix only obvious recognition errors)",
                        ),
                    );
                    ui.add_space(4.0);
                    ui.label("API Base URL");
                    ui.add_sized(
                        [ui.available_width(), 26.0],
                        egui::TextEdit::singleline(&mut state.draft.api_base_url)
                            .hint_text(DEFAULT_API_BASE),
                    );
                    ui.label("API Key");
                    ui.horizontal(|ui| {
                        let width = (ui.available_width() - 130.0).max(120.0);
                        ui.add_sized(
                            [width, 26.0],
                            egui::TextEdit::singleline(&mut state.draft.api_key)
                                .password(!state.show_key)
                                .hint_text("sk-…"),
                        );
                        ui.checkbox(&mut state.show_key, ts!("显示", "Show"));
                        if ui.button(ts!("清空", "Clear")).clicked() {
                            state.draft.api_key.clear();
                        }
                    });
                    ui.label(
                        RichText::new(ts!(
                            "API Key 使用 Windows DPAPI 加密后保存；清空会删除已保存的密钥。",
                            "The API key is stored encrypted with Windows DPAPI; clearing removes the saved key.",
                        ))
                        .size(10.5)
                        .color(muted),
                    );
                    ui.label("Model");
                    ui.add_sized(
                        [ui.available_width(), 26.0],
                        egui::TextEdit::singleline(&mut state.draft.model)
                            .hint_text(DEFAULT_MODEL),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(ts!(
                            "纠错请求只发送转录文本；系统提示词禁止润色、改写或删除正确内容。测试会发起一次最小 API 请求。",
                            "Only the transcript is sent for cleanup; the system prompt forbids polishing, rewriting or deleting correct content. Test sends one minimal request.",
                        ))
                        .size(10.5)
                        .color(muted),
                    );

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if state.testing {
                            ui.spinner();
                            ui.label(ts!("正在测试连接…", "Testing the connection…"));
                        } else if ui.button("Test").clicked() {
                            action.test = true;
                        }
                        if ui.button("Save").clicked() {
                            action.save = true;
                        }
                    });

                    if let Some((ok, message)) = &state.message {
                        let color = if *ok {
                            Color32::from_rgb(124, 231, 174)
                        } else {
                            Color32::from_rgb(255, 150, 140)
                        };
                        ui.add_space(7.0);
                        ui.label(RichText::new(message).size(11.5).color(color));
                    }

                    ui.add_space(14.0);
                    section(ui, ts!("更新", "Updates"));
                    ui.horizontal(|ui| {
                        ui.label(trf!(
                            "当前版本 v{}",
                            "Current version v{}",
                            update::current_version()
                        ));
                        if update_state.checking {
                            ui.spinner();
                            ui.label(
                                RichText::new(ts!("正在检查…", "Checking…"))
                                    .size(11.0)
                                    .color(muted),
                            );
                        } else if ui
                            .button(ts!("检查更新", "Check for updates"))
                            .clicked()
                        {
                            action.check_update = true;
                        }
                    });
                    if let Some(info) = &update_state.info {
                        ui.add_space(5.0);
                        ui.label(
                            RichText::new(trf!(
                                "发现新版本 v{}",
                                "New version v{} available",
                                info.version
                            ))
                            .size(13.0)
                            .strong()
                            .color(Color32::from_rgb(124, 231, 174)),
                        );
                        if !info.notes.trim().is_empty() {
                            egui::ScrollArea::vertical()
                                .id_salt("update_notes")
                                .max_height(120.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(info.notes.trim()).size(10.5).color(muted),
                                    );
                                });
                        }
                        ui.add_space(4.0);
                        if update_state.downloading {
                            let (done, total) = update_state
                                .progress
                                .unwrap_or((0, info.asset_size.max(1)));
                            let fraction = if total > 0 {
                                done as f32 / total as f32
                            } else {
                                0.0
                            };
                            ui.add(
                                egui::ProgressBar::new(fraction)
                                    .desired_width(ui.available_width())
                                    .text(trf!(
                                        "下载中 {:.1} / {:.1} MB",
                                        "Downloading {:.1} / {:.1} MB",
                                        done as f64 / 1_000_000.0,
                                        total as f64 / 1_000_000.0
                                    )),
                            );
                        } else {
                            ui.horizontal(|ui| {
                                if ui
                                    .button(ts!("下载并重启更新", "Download and restart"))
                                    .clicked()
                                {
                                    action.download_update = true;
                                }
                                if ui.button(ts!("在 GitHub 查看", "View on GitHub")).clicked() {
                                    action.open_release = true;
                                }
                            });
                        }
                        ui.label(
                            RichText::new(ts!(
                                "下载并校验新版 VoxKey.exe 后，会自动替换当前程序并重启。若当前程序所在目录不可写，会提示失败而不是留下半个更新。",
                                "The new VoxKey.exe is downloaded and verified, then it replaces the current program and restarts. If the program directory is not writable you get an error instead of a half-applied update.",
                            ))
                            .size(10.5)
                            .color(muted),
                        );
                    } else if !update_state.checking {
                        ui.label(
                            RichText::new(trf!(
                                "更新来源：{}",
                                "Update source: {}",
                                update::releases_page()
                            ))
                            .size(10.0)
                            .color(muted),
                        );
                    }

                    if let Some((ok, message)) = &update_state.message {
                        let color = if *ok {
                            Color32::from_rgb(124, 231, 174)
                        } else {
                            Color32::from_rgb(255, 150, 140)
                        };
                        ui.add_space(7.0);
                        ui.label(RichText::new(message).size(11.5).color(color));
                    }

                    ui.add_space(10.0);
                    section(ui, ts!("本地模型", "Local model"));
                    ui.label(RichText::new(crate::model::cache_status()).size(11.5));
                    ui.label(
                        RichText::new(trf!(
                            "缓存目录：{}",
                            "Cache folder: {}",
                            crate::model::model_dir().display()
                        ))
                        .size(10.0)
                        .color(muted),
                    );
                    ui.label(
                        RichText::new(trf!(
                            "手动导入目录：{}",
                            "Manual import folder: {}",
                            crate::model::inbox_dir().display()
                        ))
                        .size(10.0)
                        .color(muted),
                    );
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .button(ts!("打开模型目录", "Open model folder"))
                            .clicked()
                        {
                            action.open_model_dir = true;
                        }
                        if ui
                            .button(ts!("导入本地模型", "Import local model"))
                            .clicked()
                        {
                            action.import_model = true;
                        }
                        if ui
                            .button(ts!(
                                "重新检查/重试下载",
                                "Re-check / retry download",
                            ))
                            .clicked()
                        {
                            action.reload_model = true;
                        }
                        if ui
                            .small_button(ts!("复制下载链接", "Copy download links"))
                            .clicked()
                        {
                            ui.ctx().copy_text(crate::model::download_links());
                            state.model_message = Some((
                                true,
                                ts!(
                                    "已复制 4 条直链（Hugging Face 与 hf-mirror）",
                                    "Copied 4 direct links (Hugging Face and hf-mirror)",
                                )
                                .to_string(),
                            ));
                        }
                    });
                    ui.label(
                        RichText::new(ts!(
                            "下载卡住或网络受限时：用浏览器/下载器取回 model.int8.onnx 与 tokens.txt，放进“手动导入目录”，再点“导入本地模型”。具体步骤见 docs/offline-model.md。",
                            "If the download stalls or the network is restricted: fetch model.int8.onnx and tokens.txt with a browser or downloader, put them into the manual import folder, then click “Import local model”. See docs/offline-model.md.",
                        ))
                        .size(10.0)
                        .color(muted),
                    );
                    if let Some((ok, message)) = &state.model_message {
                        let color = if *ok {
                            Color32::from_rgb(124, 231, 174)
                        } else {
                            Color32::from_rgb(255, 150, 140)
                        };
                        ui.add_space(7.0);
                        ui.label(RichText::new(message).size(11.5).color(color));
                    }
                });
        });

    action
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(
        RichText::new(title)
            .strong()
            .size(14.0)
            .color(Color32::from_rgb(124, 231, 174)),
    );
    ui.separator();
    ui.add_space(5.0);
}
