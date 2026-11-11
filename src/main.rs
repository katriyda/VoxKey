#![cfg_attr(all(not(debug_assertions), not(test)), windows_subsystem = "windows")]

mod app;
mod audio;
mod clipboard;
mod config;
mod engine;
mod events;
mod hotkey;
mod llm;
mod logging;
mod model;
mod overlay;
mod platform;
mod secrets;
mod settings;
mod sha256;
mod update;
mod zh;

use eframe::egui;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::PCWSTR;

fn main() {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        // The updater runs the freshly downloaded build with `--version` as a
        // self-check, so this line must be machine-readable. A release build is
        // a GUI-subsystem executable without its own console; writing to a
        // closed stdout must not panic.
        use std::io::Write;
        let _ = writeln!(std::io::stdout(), "VoxKey {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    update::cleanup_stale_files();

    let config = config::Config::load();
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("VoxKey Overlay")
            .with_inner_size([360.0, overlay::WINDOW_HEIGHT])
            .with_resizable(false)
            .with_decorations(false)
            .with_transparent(true)
            // Keep the window visible at all times. Hiding a winit window
            // stops it from servicing cross-thread repaint requests on
            // Windows, which made the first hotkey after a long idle period
            // unreliable. The window is fully transparent and click-through
            // while the capsule is not shown.
            .with_visible(true)
            .with_active(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_has_shadow(false)
            .with_drag_and_drop(false),
        renderer: eframe::Renderer::Glow,
        multisampling: 0,
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
        let message = format!("VoxKey 启动失败：{err}");
        logging::error(&message);
        show_error(&message);
    }
}

fn show_error(message: &str) {
    let text = message
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<u16>>();
    let title = "VoxKey"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<u16>>();
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}
