//! Two-language UI strings.
//!
//! VoxKey is Chinese-first but must stay usable on Windows installations that
//! have no CJK font: in that case the UI falls back to English so no label
//! renders as tofu. Because the executable ships no font (see `fonts.rs`), the
//! decision is taken at startup and can be overridden in the settings window.
//!
//! Strings are written inline as `ts!(中文, English)` so a translation always
//! sits next to its use site and there is no key table to keep in sync. The
//! switch is a relaxed atomic read; every string is re-evaluated per frame, so
//! changing the language in the settings window updates the UI immediately.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

/// Language of the user interface, independent from the recognition language.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UiLanguage {
    /// Chinese when a CJK font is available, English otherwise.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "zh")]
    Zh,
    #[serde(rename = "en")]
    En,
}

impl UiLanguage {
    pub const ALL: [UiLanguage; 3] = [UiLanguage::Auto, UiLanguage::Zh, UiLanguage::En];

    pub fn label(self) -> &'static str {
        match self {
            UiLanguage::Auto => s("自动", "Auto"),
            UiLanguage::Zh => s("简体中文", "Chinese (Simplified)"),
            UiLanguage::En => "English",
        }
    }
}

static ENGLISH: AtomicBool = AtomicBool::new(false);
/// Whether a CJK-capable face was loaded at startup; the settings window uses
/// it to re-resolve `Auto` when the user picks another UI language.
static CJK_AVAILABLE: AtomicBool = AtomicBool::new(false);

pub fn set_english(value: bool) {
    ENGLISH.store(value, Ordering::Relaxed);
}

pub fn english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

pub fn cjk_available() -> bool {
    CJK_AVAILABLE.load(Ordering::Relaxed)
}

/// Applies a UI-language choice without restarting; used by the settings
/// window so the language switch is visible immediately.
pub fn reapply(configured: UiLanguage) {
    set_english(resolve(configured, cjk_available()));
}

/// Picks one of two static strings.
pub fn s(zh: &'static str, en: &'static str) -> &'static str {
    pick(english(), zh, en)
}

fn pick(english: bool, zh: &'static str, en: &'static str) -> &'static str {
    if english { en } else { zh }
}

/// Resolves the configured UI language. A requested Chinese UI is silently
/// switched to English when no CJK-capable font could be loaded, because
/// rendering Chinese with a Latin-only face produces unreadable boxes.
pub fn resolve(configured: UiLanguage, cjk_available: bool) -> bool {
    match configured {
        UiLanguage::En => true,
        UiLanguage::Zh => !cjk_available,
        UiLanguage::Auto => !cjk_available,
    }
}

pub fn apply(configured: UiLanguage, cjk_available: bool) {
    CJK_AVAILABLE.store(cjk_available, Ordering::Relaxed);
    set_english(resolve(configured, cjk_available));
}

/// `ts!("中文", "English")` -> `&'static str`.
macro_rules! ts {
    ($zh:literal, $en:literal $(,)?) => {
        $crate::i18n::s($zh, $en)
    };
}

/// `trf!("下载 {done} MB", "Downloading {done} MB", done = done)` style helper
/// for formatted strings, e.g. `trf!("无法保存设置：{err}", "Cannot save: {err}")`.
macro_rules! trf {
    ($zh:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::english() {
            ::std::format!($en $(, $arg)*)
        } else {
            ::std::format!($zh $(, $arg)*)
        }
    };
}

pub(crate) use trf;
pub(crate) use ts;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_languages_win_unless_fonts_are_missing() {
        assert!(resolve(UiLanguage::En, true));
        assert!(!resolve(UiLanguage::Zh, true));
        assert!(resolve(UiLanguage::Zh, false));
        assert!(!resolve(UiLanguage::Auto, true));
        assert!(resolve(UiLanguage::Auto, false));
    }

    #[test]
    fn selects_the_current_language() {
        // Tests never flip the process-wide flag: other modules assert the
        // default Chinese strings, and the harness runs tests in parallel.
        assert_eq!(pick(false, "你好", "hello"), "你好");
        assert_eq!(pick(true, "你好", "hello"), "hello");
        assert_eq!(pick(true, "共 {n} 项", "{n} items"), "{n} items");
    }

    #[test]
    fn serialization_is_stable() {
        assert_eq!(
            serde_json::to_string(&UiLanguage::Auto).unwrap(),
            "\"auto\""
        );
        assert_eq!(serde_json::to_string(&UiLanguage::Zh).unwrap(), "\"zh\"");
        assert_eq!(serde_json::to_string(&UiLanguage::En).unwrap(), "\"en\"");
    }
}
