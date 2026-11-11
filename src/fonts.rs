//! Fonts loaded from the Windows font directory.
//!
//! The 16 MB Noto Sans CJK face used to be embedded in the executable, which
//! made one third of `VoxKey.exe` font data. Windows already ships CJK faces on
//! every installation whose UI language (or optional features) include Chinese,
//! so they are read at runtime instead:
//!
//! * the Latin base face comes from `segoeui.ttf` (part of every Windows 10/11);
//! * the CJK fallback face comes from Microsoft YaHei / SimSun / DengXian.
//!
//! No font file is written to disk and nothing is copied next to the exe. When
//! no CJK face exists the UI switches to English (see `i18n::apply`) instead of
//! rendering empty boxes. `--features embed-cjk-font` restores the embedded
//! Noto face for builds that must render identically on every machine.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{FontData, FontDefinitions, FontFamily};

/// Latin UI faces, in preference order. `segoeui.ttf` is the Windows UI font
/// and is present on every Windows 10/11 installation.
const UI_FILES: [&str; 3] = ["segoeui.ttf", "tahoma.ttf", "arial.ttf"];

/// CJK faces, in preference order: Microsoft YaHei (modern, hinted for UI),
/// SimSun (legacy but ubiquitous), DengXian (Windows 10+).
const CJK_FILES: [&str; 5] = [
    "msyh.ttc",
    "msyhl.ttc",
    "simsun.ttc",
    "Deng.ttf",
    "simhei.ttf",
];

const UI_KEY: &str = "voxkey-latin";
const CJK_KEY: &str = "voxkey-cjk";

/// Loads the UI fonts and installs the VoxKey palette. Returns whether a
/// CJK-capable face was found; the caller uses this to resolve `Auto`.
pub fn install(ctx: &eframe::egui::Context) -> bool {
    let mut fonts = FontDefinitions::default();

    let latin = read_first(&UI_FILES).map(FontData::from_owned);
    if let Some(data) = latin {
        fonts.font_data.insert(UI_KEY.into(), Arc::new(data));
        prepend(&mut fonts, FontFamily::Proportional, UI_KEY);
        prepend(&mut fonts, FontFamily::Monospace, UI_KEY);
    }

    let cjk = cjk_font();
    let cjk_available = cjk.is_some();
    if let Some(data) = cjk {
        fonts.font_data.insert(CJK_KEY.into(), Arc::new(data));
        // The CJK face also carries Latin glyphs, so it alone is enough when
        // the Latin face could not be read.
        append(&mut fonts, FontFamily::Proportional, CJK_KEY);
        append(&mut fonts, FontFamily::Monospace, CJK_KEY);
    }

    ctx.set_fonts(fonts);
    apply_style(ctx);

    cjk_available
}

fn cjk_font() -> Option<FontData> {
    #[cfg(feature = "embed-cjk-font")]
    {
        return Some(FontData::from_static(include_bytes!(
            "../assets/NotoSansCJKsc-Regular.otf"
        )));
    }
    #[cfg(not(feature = "embed-cjk-font"))]
    {
        read_first(&CJK_FILES).map(FontData::from_owned)
    }
}

/// Font families are fallback lists: an earlier entry wins for a given glyph,
/// so the Latin face is prepended and the CJK face is appended behind it.
fn prepend(fonts: &mut FontDefinitions, family: FontFamily, key: &str) {
    let list = fonts.families.entry(family).or_default();
    if let Some(position) = list.iter().position(|existing| existing == key) {
        list.remove(position);
    }
    list.insert(0, key.to_string());
}

fn append(fonts: &mut FontDefinitions, family: FontFamily, key: &str) {
    let list = fonts.families.entry(family).or_default();
    if !list.iter().any(|existing| existing == key) {
        list.push(key.to_string());
    }
}

fn windows_fonts_dir() -> PathBuf {
    std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts")
}

fn read_first(files: &[&str]) -> Option<Vec<u8>> {
    let dir = windows_fonts_dir();
    files
        .iter()
        .map(|file| dir.join(file))
        .find(|path| path.is_file())
        .and_then(|path| std::fs::read(path).ok())
}

fn apply_style(ctx: &eframe::egui::Context) {
    use eframe::egui::{Color32, FontId, Vec2};

    // egui 0.37 stores one `Style` per theme, so the palette and text styles
    // are applied through `all_styles_mut` to cover every theme.
    ctx.all_styles_mut(|style| {
        style.visuals = eframe::egui::Visuals::dark();
        style.visuals.panel_fill = Color32::from_rgb(18, 20, 27);
        style.visuals.window_fill = Color32::from_rgb(18, 20, 27);
        style.visuals.extreme_bg_color = Color32::from_rgb(12, 14, 19);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(34, 38, 49);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(44, 49, 63);
        style.visuals.widgets.active.bg_fill = Color32::from_rgb(54, 60, 77);
        style.visuals.selection.bg_fill = Color32::from_rgb(53, 101, 79);
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 5.0);
        style
            .text_styles
            .insert(eframe::egui::TextStyle::Body, FontId::proportional(13.0));
        style
            .text_styles
            .insert(eframe::egui::TextStyle::Button, FontId::proportional(13.0));
        style
            .text_styles
            .insert(eframe::egui::TextStyle::Small, FontId::proportional(10.5));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_always_ships_a_latin_face() {
        let dir = windows_fonts_dir();
        assert!(
            read_first(&UI_FILES).is_some(),
            "no Latin UI font under {}",
            dir.display()
        );
    }

    #[test]
    fn cjk_lookup_reports_presence_without_panicking() {
        // This machine may or may not have a CJK face; the function must be
        // total either way.
        let _ = cjk_font();
    }
}
