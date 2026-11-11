use std::path::PathBuf;

use crate::config::Language;
use crate::model::ImportOutcome;
use crate::update::ReleaseInfo;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    SetLanguage(Language),
    ToggleLlm,
    ImportModel,
    OpenModelDir,
    ReloadModel,
    CheckUpdate,
    OpenSettings,
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
    /// The local model could not be prepared. Distinct from `Error` so the UI
    /// can explain the model problem instead of claiming an insertion failed.
    ModelError(String),
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
    CheckFinished(Result<Option<ReleaseInfo>, String>),
    Progress { downloaded: u64, total: u64 },
    Downloaded(PathBuf),
    Failed(String),
}

#[derive(Debug)]
pub enum UiEvent {
    HotkeyDown,
    HotkeyUp,
    Tray(TrayAction),
    Engine(EngineEvent),
    ModelImportDone(Result<ImportOutcome, String>),
    LlmTestResult(Result<String, String>),
    Update(UpdateEvent),
    PlatformError(String),
}
