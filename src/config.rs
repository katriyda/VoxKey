use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::hotkey::{parse_hotkey, DEFAULT_HOTKEY};
use crate::secrets;

pub const DEFAULT_API_BASE: &str = "https://api.openai.com/v1";
pub const DEFAULT_MODEL: &str = "gpt-4o-mini";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en")]
    En,
    #[serde(rename = "zh-TW")]
    ZhTw,
    #[serde(rename = "ja")]
    Ja,
    #[serde(rename = "ko")]
    Ko,
}

impl Default for Language {
    fn default() -> Self {
        Self::ZhCn
    }
}

impl Language {
    pub const ALL: [Language; 5] = [
        Language::ZhCn,
        Language::En,
        Language::ZhTw,
        Language::Ja,
        Language::Ko,
    ];

    /// Language token consumed by the SenseVoice model.
    pub fn asr_code(self) -> &'static str {
        match self {
            Language::ZhCn | Language::ZhTw => "zh",
            Language::En => "en",
            Language::Ja => "ja",
            Language::Ko => "ko",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Language::ZhCn => "简体中文",
            Language::En => "English",
            Language::ZhTw => "繁體中文",
            Language::Ja => "日本語",
            Language::Ko => "한국어",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Language::ZhCn => "ZH",
            Language::En => "EN",
            Language::ZhTw => "TW",
            Language::Ja => "JA",
            Language::Ko => "KO",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub language: Language,
    pub hotkey: String,
    pub llm_enabled: bool,
    pub api_base_url: String,
    pub api_key: String,
    pub model: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: Language::ZhCn,
            hotkey: DEFAULT_HOTKEY.to_string(),
            llm_enabled: false,
            api_base_url: DEFAULT_API_BASE.to_string(),
            api_key: String::new(),
            model: DEFAULT_MODEL.to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct DiskConfig {
    language: Language,
    hotkey: String,
    llm_enabled: bool,
    api_base_url: String,
    model: String,
    /// DPAPI-protected key, hex encoded. `None` means no key is stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key_enc: Option<String>,
}

impl Default for DiskConfig {
    fn default() -> Self {
        Self {
            language: Language::ZhCn,
            hotkey: DEFAULT_HOTKEY.to_string(),
            llm_enabled: false,
            api_base_url: DEFAULT_API_BASE.to_string(),
            model: DEFAULT_MODEL.to_string(),
            api_key_enc: None,
        }
    }
}

pub fn data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("VOXKEY_DATA_DIR") {
        return PathBuf::from(path);
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata).join("VoxKey");
    }
    PathBuf::from(".voxkey")
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

impl Config {
    /// Loads configuration from `%APPDATA%\VoxKey\config.json`.
    ///
    /// A missing or corrupt file yields defaults. Decryption errors are
    /// reported on stderr and result in an empty in-memory API key.
    pub fn load() -> Self {
        let disk = match fs::read_to_string(config_path()) {
            Ok(text) => match serde_json::from_str::<DiskConfig>(&text) {
                Ok(disk) => disk,
                Err(err) => {
                    eprintln!("VoxKey: invalid config.json: {err}");
                    DiskConfig::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => DiskConfig::default(),
            Err(err) => {
                eprintln!("VoxKey: cannot read config.json: {err}");
                DiskConfig::default()
            }
        };

        let api_key = disk
            .api_key_enc
            .as_deref()
            .and_then(|hex| match secrets::unprotect(hex) {
                Ok(key) => Some(key),
                Err(err) => {
                    eprintln!("VoxKey: cannot decrypt stored API key: {err}");
                    None
                }
            })
            .unwrap_or_default();

        let hotkey = if parse_hotkey(&disk.hotkey).is_ok() {
            disk.hotkey
        } else {
            DEFAULT_HOTKEY.to_string()
        };

        Self {
            language: disk.language,
            hotkey,
            llm_enabled: disk.llm_enabled,
            api_base_url: non_empty_or(disk.api_base_url, DEFAULT_API_BASE),
            api_key,
            model: non_empty_or(disk.model, DEFAULT_MODEL),
        }
    }

    /// Writes configuration atomically, protecting the API key with Windows
    /// DPAPI (current user scope). An empty key is stored as "no key at all",
    /// which is the only way to clear a previously saved secret.
    pub fn save(&self) -> Result<(), String> {
        let api_key_enc = if self.api_key.trim().is_empty() {
            None
        } else {
            Some(secrets::protect(self.api_key.trim())?)
        };

        let disk = DiskConfig {
            language: self.language,
            hotkey: self.hotkey.clone(),
            llm_enabled: self.llm_enabled,
            api_base_url: self.api_base_url.trim().to_string(),
            model: self.model.trim().to_string(),
            api_key_enc,
        };

        let dir = data_dir();
        fs::create_dir_all(&dir).map_err(|e| format!("无法创建配置目录: {e}"))?;
        let path = config_path();
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(&disk)
            .map_err(|e| format!("无法序列化配置: {e}"))?;
        fs::write(&tmp, json).map_err(|e| format!("无法写入配置: {e}"))?;

        // std::fs::rename does not replace an existing file on Windows.
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
        fs::rename(&tmp, &path).map_err(|e| format!("无法保存配置: {e}"))?;
        Ok(())
    }
}

fn non_empty_or(value: String, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_serialization_is_stable() {
        assert_eq!(
            serde_json::to_string(&Language::ZhTw).unwrap(),
            "\"zh-TW\""
        );
        let lang: Language = serde_json::from_str("\"ja\"").unwrap();
        assert_eq!(lang, Language::Ja);
    }

    #[test]
    fn defaults_are_sane() {
        let cfg = Config::default();
        assert_eq!(cfg.language, Language::ZhCn);
        assert_eq!(parse_hotkey(&cfg.hotkey).unwrap().label, "右 Ctrl");
        assert!(!cfg.llm_enabled);
        assert!(cfg.api_key.is_empty());
    }
}
