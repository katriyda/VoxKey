//! Background recording, local SenseVoice recognition, optional refinement and
//! text injection. Everything that can block lives here; the egui thread only
//! receives small state updates.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig};

use crate::audio::{resample_linear, Recorder};
use crate::clipboard;
use crate::config::Language;
use crate::events::{EngineEvent, UiEvent};
use crate::llm::{self, RefineConfig};
use crate::{model, zh};

const TARGET_RATE: u32 = 16_000;
const MAX_RECORDING_SECONDS: u64 = 60;
const MIN_RECORDING_SECONDS: f32 = 0.35;
const MIN_VOICE_RMS: f32 = 0.008;
const SOFT_SINGLE_CHAR_RMS: f32 = 0.025;

pub enum EngineCommand {
    Start {
        language: Language,
        refine: RefineConfig,
    },
    Stop,
    Shutdown,
}

pub struct EngineHandle {
    tx: Sender<EngineCommand>,
    _join: Option<JoinHandle<()>>,
}

impl EngineHandle {
    pub fn start(&self, language: Language, refine: RefineConfig) {
        let _ = self.tx.send(EngineCommand::Start { language, refine });
    }

    pub fn stop(&self) {
        let _ = self.tx.send(EngineCommand::Stop);
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(EngineCommand::Shutdown);
    }
}

pub fn spawn(
    initial_language: Language,
    ui_tx: Sender<UiEvent>,
    ctx: egui::Context,
) -> Result<EngineHandle, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let join = std::thread::Builder::new()
        .name("voxkey-engine".to_string())
        .spawn(move || run_engine(initial_language, rx, ui_tx, ctx))
        .map_err(|e| format!("无法启动语音线程: {e}"))?;
    Ok(EngineHandle {
        tx,
        _join: Some(join),
    })
}

fn emit(tx: &Sender<UiEvent>, ctx: &egui::Context, event: EngineEvent) {
    if let EngineEvent::Error(ref message) = event {
        crate::logging::error(message);
    }
    let _ = tx.send(UiEvent::Engine(event));
    ctx.request_repaint();
}

struct Backend {
    recognizer: OfflineRecognizer,
    language: Language,
}

struct Session {
    recorder: Option<Recorder>,
    language: Language,
    refine: RefineConfig,
    next_level: Instant,
    next_partial: Instant,
    last_partial_samples: usize,
}

struct RuntimeState {
    backend: Option<Backend>,
    model_dir: Option<PathBuf>,
    backend_error: Option<String>,
}

fn run_engine(
    initial_language: Language,
    rx: Receiver<EngineCommand>,
    tx: Sender<UiEvent>,
    ctx: egui::Context,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            emit(
                &tx,
                &ctx,
                EngineEvent::Error(format!("无法创建网络运行时: {e}")),
            );
            return;
        }
    };

    emit(
        &tx,
        &ctx,
        EngineEvent::Status("正在准备本地语音模型…".to_string()),
    );

    let model_dir = {
        let mut progress = |downloaded: u64, total: u64, label: &str| {
            emit(
                &tx,
                &ctx,
                EngineEvent::Download {
                    downloaded,
                    total,
                    label: label.to_string(),
                },
            );
        };
        model::ensure_model(&runtime, &mut progress)
    };

    let mut state = RuntimeState {
        backend: None,
        model_dir: None,
        backend_error: None,
    };

    match model_dir {
        Ok(dir) => {
            state.model_dir = Some(dir.clone());
            emit(
                &tx,
                &ctx,
                EngineEvent::Status("正在加载语音模型…".to_string()),
            );
            match load_backend(&dir, initial_language) {
                Ok(backend) => {
                    state.backend = Some(backend);
                    emit(&tx, &ctx, EngineEvent::Ready);
                }
                Err(err) => {
                    state.backend_error = Some(err.clone());
                    emit(&tx, &ctx, EngineEvent::Error(err));
                }
            }
        }
        Err(err) => {
            state.backend_error = Some(err.clone());
            emit(&tx, &ctx, EngineEvent::Error(err));
        }
    }

    let mut session: Option<Session> = None;

    loop {
        if session.is_none() {
            match rx.recv() {
                Ok(EngineCommand::Start { language, refine }) => {
                    match start_session(&mut state, language, refine, &tx, &ctx) {
                        Ok(new_session) => session = Some(new_session),
                        Err(err) => emit(&tx, &ctx, EngineEvent::Error(err)),
                    }
                }
                Ok(EngineCommand::Stop) => {}
                Ok(EngineCommand::Shutdown) | Err(_) => break,
            }
            continue;
        }

        // A recording is active. Poll commands while capturing.
        let mut stop_requested = false;
        let mut shutdown_requested = false;
        loop {
            match rx.try_recv() {
                Ok(EngineCommand::Stop) => {
                    stop_requested = true;
                    break;
                }
                Ok(EngineCommand::Shutdown) => {
                    shutdown_requested = true;
                    break;
                }
                Ok(EngineCommand::Start { .. }) => {
                    // Ignore key bounce / a second press during a session.
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    shutdown_requested = true;
                    break;
                }
            }
        }

        if shutdown_requested {
            if let Some(mut current) = session.take() {
                if let Some(recorder) = current.recorder.take() {
                    let _ = recorder.stop();
                }
            }
            break;
        }

        let Some(current) = session.as_mut() else {
            continue;
        };

        let recorder = current.recorder.as_ref().expect("active recorder");
        let now = Instant::now();
        if now >= current.next_level {
            current.next_level = now + Duration::from_millis(50);
            emit(&tx, &ctx, EngineEvent::Level(recorder.level()));
        }

        if now >= current.next_partial {
            current.next_partial = now + Duration::from_millis(1300);
            let samples = recorder.snapshot();
            let sample_rate = recorder.sample_rate();
            let new_samples = samples.len().saturating_sub(current.last_partial_samples);
            if new_samples > sample_rate as usize / 4 && rms(&samples) >= MIN_VOICE_RMS {
                current.last_partial_samples = samples.len();
                if duration_seconds(samples.len(), sample_rate) >= 0.8 {
                    let Some(backend) = state.backend.as_ref() else {
                        continue;
                    };
                    match decode_samples(backend, &samples, sample_rate, current.language) {
                        Ok(text) if has_meaningful_text(&text) => {
                            emit(&tx, &ctx, EngineEvent::Transcript(text));
                        }
                        Ok(_) => {}
                        Err(err) => eprintln!("VoxKey: partial decode failed: {err}"),
                    }
                }
            }
        }

        let sample_count = current
            .recorder
            .as_ref()
            .map(|r| r.sample_count())
            .unwrap_or(0);
        let sample_rate = current
            .recorder
            .as_ref()
            .map(|r| r.sample_rate())
            .unwrap_or(TARGET_RATE);
        if duration_seconds(sample_count, sample_rate) >= MAX_RECORDING_SECONDS as f32 {
            emit(
                &tx,
                &ctx,
                EngineEvent::Status("已达到 60 秒上限，正在识别…".to_string()),
            );
            stop_requested = true;
        }

        if stop_requested || shutdown_requested {
            let current = session.take().expect("active session");
            let result = finish_session(&runtime, current, &mut state, &tx, &ctx);
            if let Err(err) = result {
                emit(&tx, &ctx, EngineEvent::Error(err));
            }
            continue;
        }

        // Avoid busy-waiting while the audio callback collects data.
        std::thread::sleep(Duration::from_millis(25));
    }

    if let Some(mut current) = session {
        if let Some(recorder) = current.recorder.take() {
            let _ = recorder.stop();
        }
    }
}

fn start_session(
    state: &mut RuntimeState,
    language: Language,
    refine: RefineConfig,
    tx: &Sender<UiEvent>,
    ctx: &egui::Context,
) -> Result<Session, String> {
    let Some(dir) = state.model_dir.as_ref() else {
        return Err(state
            .backend_error
            .clone()
            .unwrap_or_else(|| "语音模型尚未就绪，请重启应用后重试".to_string()));
    };

    if state.backend.is_none() {
        match load_backend(dir, language) {
            Ok(backend) => {
                state.backend = Some(backend);
                emit(tx, ctx, EngineEvent::Ready);
            }
            Err(err) => {
                state.backend_error = Some(err.clone());
                return Err(err);
            }
        }
    }

    if state
        .backend
        .as_ref()
        .map(|backend| backend.language != language)
        .unwrap_or(true)
    {
        emit(
            tx,
            ctx,
            EngineEvent::Status(format!("正在加载{}模型…", language.label())),
        );
        let backend = load_backend(dir, language)?;
        state.backend = Some(backend);
    }

    emit(tx, ctx, EngineEvent::Status("正在聆听…".to_string()));
    let recorder = Recorder::start().map_err(|e| format!("麦克风不可用: {e}"))?;
    emit(tx, ctx, EngineEvent::RecordingStarted);

    let now = Instant::now();
    Ok(Session {
        recorder: Some(recorder),
        language,
        refine,
        next_level: now,
        next_partial: now + Duration::from_millis(950),
        last_partial_samples: 0,
    })
}

fn finish_session(
    runtime: &tokio::runtime::Runtime,
    mut session: Session,
    state: &mut RuntimeState,
    tx: &Sender<UiEvent>,
    ctx: &egui::Context,
) -> Result<(), String> {
    let recorder = session
        .recorder
        .take()
        .ok_or_else(|| "录音状态异常".to_string())?;
    let (samples, sample_rate, audio_error) = recorder.stop();
    if let Some(err) = audio_error {
        eprintln!("VoxKey: audio error while recording: {err}");
    }

    if duration_seconds(samples.len(), sample_rate) < MIN_RECORDING_SECONDS {
        return Err("没有检测到语音，请按住快捷键说话".to_string());
    }

    // SenseVoice will happily "recognize" room noise as a character or
    // punctuation mark. Require some real acoustic energy before trusting it.
    if rms(&samples) < MIN_VOICE_RMS {
        return Err("没有检测到语音，请按住快捷键说话".to_string());
    }

    let Some(backend) = state.backend.as_ref() else {
        return Err("语音模型未就绪".to_string());
    };

    emit(tx, ctx, EngineEvent::Recognizing);
    let mut text = decode_samples(backend, &samples, sample_rate, session.language)?;
    if !has_meaningful_text(&text) {
        return Err("没有识别出文字，请再试一次".to_string());
    }
    // A single character decoded from a faint recording is almost always
    // noise; ask for a clearer retry instead of inserting garbage.
    if meaningful_char_count(&text) <= 1 && rms(&samples) < SOFT_SINGLE_CHAR_RMS {
        return Err("没有识别出文字，请再试一次".to_string());
    }

    let mut warnings = Vec::new();
    if session.refine.enabled {
        emit(tx, ctx, EngineEvent::Refining);
        match runtime.block_on(llm::refine_async(&session.refine, &text)) {
            Ok(refined) => {
                text = postprocess(session.language, &refined);
            }
            Err(err) => {
                warnings.push(format!("LLM 纠错已跳过：{err}"));
            }
        }
    }

    if !has_meaningful_text(&text) {
        return Err("纠错后的文本为空，已取消插入".to_string());
    }

    emit(tx, ctx, EngineEvent::Status("正在插入文字…".to_string()));
    match clipboard::inject_text(&text) {
        Ok(restore_warning) => {
            if let Some(warning) = restore_warning {
                warnings.push(warning);
            }
            emit(
                tx,
                ctx,
                EngineEvent::Done {
                    text,
                    warning: if warnings.is_empty() {
                        None
                    } else {
                        Some(warnings.join("；"))
                    },
                },
            );
            Ok(())
        }
        Err(err) => Err(format!("无法插入文字：{err}")),
    }
}

fn decode_samples(
    backend: &Backend,
    samples: &[f32],
    sample_rate: u32,
    language: Language,
) -> Result<String, String> {
    let audio = resample_linear(samples, sample_rate, TARGET_RATE);
    let stream = backend.recognizer.create_stream();
    stream.accept_waveform(TARGET_RATE as i32, &audio);
    backend.recognizer.decode(&stream);
    let raw = stream
        .get_result()
        .ok_or_else(|| "识别器没有返回结果".to_string())?
        .text;
    Ok(postprocess(language, &raw))
}

fn postprocess(language: Language, raw: &str) -> String {
    let cleaned = strip_sense_voice_tags(raw).trim().to_string();
    if language == Language::ZhTw {
        zh::to_traditional(&cleaned)
    } else {
        cleaned
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

fn meaningful_char_count(text: &str) -> usize {
    text.chars().filter(|ch| ch.is_alphanumeric()).count()
}

fn has_meaningful_text(text: &str) -> bool {
    text.chars().any(|ch| ch.is_alphanumeric())
}

fn strip_sense_voice_tags(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("<|") {
        result.push_str(&rest[..start]);
        let after_start = &rest[start + 2..];
        if let Some(end) = after_start.find("|>") {
            rest = &after_start[end + 2..];
        } else {
            rest = "";
            break;
        }
    }
    result.push_str(rest);
    result
}

fn load_backend(dir: &Path, language: Language) -> Result<Backend, String> {
    let model = dir.join("model.int8.onnx");
    let tokens = dir.join("tokens.txt");
    if !model.is_file() || !tokens.is_file() {
        return Err(format!("模型文件缺失，请删除缓存后重启：{}", dir.display()));
    }

    let mut config = OfflineRecognizerConfig::default();
    config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
        model: Some(model.to_string_lossy().into_owned()),
        language: Some(language.asr_code().to_string()),
        use_itn: true,
    };
    config.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
    config.model_config.provider = Some("cpu".to_string());
    config.model_config.num_threads = std::thread::available_parallelism()
        .map(|count| count.get().min(4) as i32)
        .unwrap_or(2)
        .max(1);

    let recognizer = OfflineRecognizer::create(&config)
        .ok_or_else(|| "无法初始化 SenseVoice 识别器（模型文件可能损坏）".to_string())?;

    Ok(Backend {
        recognizer,
        language,
    })
}

fn duration_seconds(samples: usize, sample_rate: u32) -> f32 {
    if sample_rate == 0 {
        0.0
    } else {
        samples as f32 / sample_rate as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_model_tags() {
        assert_eq!(
            strip_sense_voice_tags("<|zh|><|NEUTRAL|><|Speech|>你好，世界。"),
            "你好，世界。"
        );
        assert_eq!(strip_sense_voice_tags("没有标签"), "没有标签");
    }

    #[test]
    fn postprocesses_traditional() {
        assert_eq!(
            postprocess(Language::ZhTw, "这是语音输入。"),
            "這是語音輸入。"
        );
        assert_eq!(postprocess(Language::En, "hello"), "hello");
    }

    #[test]
    fn duration_math() {
        assert!((duration_seconds(16_000, 16_000) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn punctuation_only_is_not_text() {
        assert!(!has_meaningful_text(""));
        assert!(!has_meaningful_text("。"));
        assert!(!has_meaningful_text("，！？…"));
        assert!(has_meaningful_text("你好"));
        assert!(has_meaningful_text("hello"));
        assert!(has_meaningful_text("こんにちは"));
    }

    #[test]
    fn rms_distinguishes_silence_from_speech() {
        assert_eq!(rms(&[]), 0.0);
        assert!(rms(&vec![0.0; 1000]) < 0.001);
        assert!((rms(&vec![0.1; 1000]) - 0.1).abs() < 1e-5);
        assert_eq!(meaningful_char_count("。"), 0);
        assert_eq!(meaningful_char_count("我。"), 1);
        assert_eq!(meaningful_char_count("你好"), 2);
    }
}
