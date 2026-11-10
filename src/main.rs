#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod clipboard;
mod config;
mod engine;
mod events;
mod hotkey;
mod llm;
mod model;
mod overlay;
mod platform;
mod secrets;
mod settings;
mod zh;

use eframe::egui;
use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

fn main() {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        std::process::exit(0);
    }

    let config = config::Config::load();
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("VoxKey Overlay")
            .with_inner_size([360.0, overlay::WINDOW_HEIGHT])
            .with_resizable(false)
            .with_decorations(false)
            .with_transparent(true)
            .with_visible(false)
            .with_active(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_mouse_passthrough(true)
            .with_has_shadow(false)
            .with_drag_and_drop(false),
        renderer: eframe::Renderer::Glow,
        multisampling: 0,
        vsync: true,
        centered: false,
        persist_window: false,
        ..Default::default()
    };

    let result = eframe::run_native(
        "VoxKey Overlay",
        native_options,
        Box::new(move |cc| {
            let app = app::VoxKeyApp::new(cc, config)?;
            Ok(Box::new(app))
        }),
    );

    if let Err(err) = result {
        show_error(&format!("VoxKey 启动失败：{err}"));
    }
}

fn show_error(message: &str) {
    let text = message.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let title = "VoxKey".encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}
