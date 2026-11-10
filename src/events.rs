use crate::config::Language;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    SetLanguage(Language),
    ToggleLlm,
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
pub enum UiEvent {
    HotkeyDown,
    HotkeyUp,
    Tray(TrayAction),
    Engine(EngineEvent),
    LlmTestResult(Result<String, String>),
    PlatformError(String),
    Quit,
}
