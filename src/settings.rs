//! Settings window contents.

use egui::{Color32, RichText};

use crate::config::{Config, Language, DEFAULT_API_BASE, DEFAULT_MODEL};
use crate::hotkey::parse_hotkey;

#[derive(Default)]
pub struct SettingsAction {
    pub test: bool,
    pub save: bool,
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

pub fn draw(ctx: &egui::Context, state: &mut SettingsState) -> SettingsAction {
    let mut action = SettingsAction::default();
    let theme_bg = Color32::from_rgb(18, 20, 27);
    let muted = Color32::from_rgb(151, 159, 176);

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme_bg)
                .inner_margin(egui::Margin::same(20)),
        )
        .show(ctx, |ui| {
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
