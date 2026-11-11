use std::path::PathBuf;

use crate::config::Language;
use crate::update::ReleaseInfo;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    SetLanguage(Language),
    ToggleLlm,
    OpenSettings,
    CheckUpdate,
    Quit,
}

#[derive(Debug)]
pub enum EngineEvent {
    Ready,
    Status(String),
    Download {
        downloaded: u64,
        total: u64,
        label: String,
    },
    RecordingStarted,
    Level(f32),
    Transcript(String),
    Recognizing,
    Refining,
    Done {
        text: String,
        warning: Option<String>,
    },
    Error(String),
}

#[derive(Debug)]
pub enum UpdateEvent {
    /// `Ok(None)` means the running build is already current.
    CheckFinished(Result<Option<ReleaseInfo>, String>),
    Progress {
        downloaded: u64,
        total: u64,
    },
    Downloaded(PathBuf),
    Failed(String),
}

#[derive(Debug)]
pub enum UiEvent {
    HotkeyDown,
    HotkeyUp,
    Tray(TrayAction),
    Engine(EngineEvent),
    LlmTestResult(Result<String, String>),
    Update(UpdateEvent),
    PlatformError(String),
}
