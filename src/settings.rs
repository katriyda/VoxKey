//! Settings window contents.

use egui::{Color32, RichText};

use crate::config::{Config, DEFAULT_API_BASE, DEFAULT_MODEL, Language};
use crate::hotkey::parse_hotkey;
use crate::update::{self, UpdateUiState};

#[derive(Default)]
pub struct SettingsAction {
    pub test: bool,
    pub save: bool,
    pub check_update: bool,
    pub download_update: bool,
    pub open_release: bool,
}

pub struct SettingsState {
    pub draft: Config,
    pub show_key: bool,
    pub testing: bool,
    pub message: Option<(bool, String)>,
}

impl SettingsState {
    pub fn from_config(config: &Config) -> Self {
        Self {
            draft: config.clone(),
            show_key: false,
            testing: false,
            message: None,
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
                            RichText::new("本地语音输入")
                                .size(12.0)
                                .color(muted),
                        );
                    });
                    ui.label(
                        RichText::new("按住快捷键录音，松开后识别并插入到当前输入框。")
                            .color(muted)
                            .size(12.0),
                    );
                    ui.add_space(12.0);

                    section(ui, "语音");
                    ui.horizontal(|ui| {
                        ui.label("识别语言");
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
                    ui.label("按住说话快捷键");
                    let preview = parse_hotkey(&state.draft.hotkey);
                    ui.add_sized(
                        [ui.available_width(), 26.0],
                        egui::TextEdit::singleline(&mut state.draft.hotkey)
                            .hint_text("right_ctrl 或 ctrl+shift+space"),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui.small_button("右 Ctrl").clicked() {
                            state.draft.hotkey = "right_ctrl".to_string();
                        }
                        if ui.small_button("Ctrl + Shift + 空格").clicked() {
                            state.draft.hotkey = "ctrl+shift+space".to_string();
                        }
                        if ui.small_button("Fn / F13").clicked() {
                            state.draft.hotkey = "fn".to_string();
                        }
                        if ui.small_button("恢复默认").clicked() {
                            state.draft.hotkey = crate::hotkey::DEFAULT_HOTKEY.to_string();
                        }
                    });
                    match preview {
                        Ok(spec) => {
                            ui.label(
                                RichText::new(format!("当前组合：{}", spec.label))
                                    .size(11.0)
                                    .color(Color32::from_rgb(124, 231, 174)),
                            );
                        }
                        Err(err) => {
                            ui.label(
                                RichText::new(format!("快捷键无效：{err}"))
                                    .size(11.0)
                                    .color(Color32::from_rgb(255, 150, 140)),
                            );
                        }
                    }
                    ui.label(
                        RichText::new(
                            "多数笔记本的 Fn 键由键盘固件拦截，Windows 收不到；可改用 F13 映射或其他自定义组合。",
                        )
                        .size(10.5)
                        .color(muted),
                    );
                    ui.add_space(12.0);

                    section(ui, "LLM 纠错");
                    ui.checkbox(
                        &mut state.draft.llm_enabled,
                        "启用保守纠错（只修复明显的语音识别错误）",
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
                                .hint_text("sk-…（可留空）"),
                        );
                        ui.checkbox(&mut state.show_key, "显示");
                        if ui.button("清空").clicked() {
                            state.draft.api_key.clear();
                        }
                    });
                    ui.label(
                        RichText::new("API Key 使用 Windows DPAPI 加密后保存；清空会删除已保存的密钥。")
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
                        RichText::new(
                            "纠错请求只发送转录文本；系统提示词禁止润色、改写或删除正确内容。测试会发起一次最小 API 请求。",
                        )
                        .size(10.5)
                        .color(muted),
                    );

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if state.testing {
                            ui.spinner();
                            ui.label("正在测试连接…");
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
                    section(ui, "更新");
                    ui.horizontal(|ui| {
                        ui.label(format!("当前版本 v{}", update::current_version()));
                        if update_state.checking {
                            ui.spinner();
                            ui.label(RichText::new("正在检查…").size(11.0).color(muted));
                        } else if ui.button("检查更新").clicked() {
                            action.check_update = true;
                        }
                    });
                    if let Some(info) = &update_state.info {
                        ui.add_space(5.0);
                        ui.label(
                            RichText::new(format!("发现新版本 v{}", info.version))
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
                            let (done, total) =
                                update_state.progress.unwrap_or((0, info.asset_size.max(1)));
                            let fraction = if total > 0 {
                                done as f32 / total as f32
                            } else {
                                0.0
                            };
                            ui.add(
                                egui::ProgressBar::new(fraction)
                                    .desired_width(ui.available_width())
                                    .text(format!(
                                        "下载中 {:.1} / {:.1} MB",
                                        done as f64 / 1_000_000.0,
                                        total as f64 / 1_000_000.0
                                    )),
                            );
                        } else {
                            ui.horizontal(|ui| {
                                if ui.button("下载并重启更新").clicked() {
                                    action.download_update = true;
                                }
                                if ui.button("在 GitHub 查看").clicked() {
                                    action.open_release = true;
                                }
                            });
                        }
                        ui.label(
                            RichText::new(
                                "下载并校验新版 VoxKey.exe 后，会自动替换当前程序并重启。若程序所在目录不可写，会明确提示失败，不会留下半个更新。",
                            )
                            .size(10.5)
                            .color(muted),
                        );
                    } else if !update_state.checking {
                        ui.label(
                            RichText::new(format!("更新来源：{}", update::releases_page()))
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
                    ui.label(
                        RichText::new(format!(
                            "模型缓存：{}",
                            crate::model::model_dir().display()
                        ))
                        .size(10.0)
                        .color(muted),
                    );
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
