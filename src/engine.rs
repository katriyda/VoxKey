//! Background recording, local SenseVoice recognition (delegated to the
//! `VoxKey-asr` worker process), optional refinement and text injection.
//! Everything that can block lives here; the egui thread only receives small
//! state updates.
//!
//! The worker is what keeps `VoxKey.exe` small: it owns sherpa-onnx and the
//! statically linked onnxruntime, is downloaded next to the model on first use,
//! and is fed 16 kHz mono audio over a pipe. Recognition quality is unchanged,
//! because it is the same model and the same decoding code as before.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::audio::{Recorder, resample_linear};
use crate::clipboard;
use crate::config::Language;
use crate::events::{EngineEvent, UiEvent};
use crate::i18n::{trf, ts};
use crate::llm::{self, RefineConfig};
use crate::{model, worker, zh};

use crate::worker::Worker;

const TARGET_RATE: u32 = 16_000;
const MAX_RECORDING_SECONDS: u64 = 60;
const MIN_RECORDING_SECONDS: f32 = 0.35;
const MIN_VOICE_RMS: f32 = 0.008;
const SOFT_SINGLE_CHAR_RMS: f32 = 0.025;

/// How often the live preview is refreshed while the hotkey is held. SenseVoice
/// decodes at roughly 4% of real time (measured: 40 ms per second of audio on a
/// 4-thread int8 CPU run), so a quarter-second cadence keeps the preview within
/// one short utterance of what the user just said.
const PARTIAL_INTERVAL: Duration = Duration::from_millis(250);
/// Energy analysis frame for pause detection.
const VAD_FRAME_MS: u32 = 10;
/// A pause this long finishes the current preview segment: it is decoded once
/// and kept as text, so the live preview only re-decodes the current phrase.
const SEGMENT_SILENCE_MS: u32 = 280;
/// Shortest closed segment worth decoding on its own.
const SEGMENT_MIN_SECONDS: f32 = 0.15;
/// Longest open phrase before it is committed anyway. Continuous speech with
/// no pause otherwise makes the per-preview decode cost grow without bound.
const MAX_OPEN_SEGMENT_SECONDS: f32 = 8.0;
/// Do not decode a tail shorter than this; it only produces noise or "嗯".
const PREVIEW_MIN_SECONDS: f32 = 0.30;

pub enum EngineCommand {
    Start {
        language: Language,
        refine: RefineConfig,
    },
    Stop,
    ReloadModel,
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

    /// Re-runs engine and model preparation (download, import, verification)
    /// without restarting the process.
    pub fn reload_model(&self) {
        let _ = self.tx.send(EngineCommand::ReloadModel);
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
        .map_err(|e| {
            trf!(
                "无法启动语音线程: {e}",
                "Cannot start the speech thread: {e}"
            )
        })?;
    Ok(EngineHandle {
        tx,
        _join: Some(join),
    })
}

fn emit(tx: &Sender<UiEvent>, ctx: &egui::Context, event: EngineEvent) {
    match &event {
        EngineEvent::Error(message) | EngineEvent::ModelError(message) => {
            crate::logging::error(message);
        }
        _ => {}
    }
    let _ = tx.send(UiEvent::Engine(event));
    ctx.request_repaint();
}

struct Session {
    recorder: Option<Recorder>,
    language: Language,
    refine: RefineConfig,
    next_level: Instant,
    next_partial: Instant,
    partial: PartialState,
}

/// Live-preview state. The recording is cut at pauses: every closed segment is
/// decoded once and appended to `committed`; only the open phrase is decoded
/// again on each refresh, which is what keeps the preview real-time for long
/// dictation instead of re-decoding the entire buffer.
#[derive(Default)]
struct PartialState {
    /// Absolute raw sample index already scanned for pauses.
    scan: usize,
    /// Absolute raw sample index where the open phrase starts.
    segment_start: usize,
    /// Length of the current run of quiet frames.
    silence_run: usize,
    /// Whether the open phrase contains any non-quiet frame.
    has_speech: bool,
    /// Text of segments that already ended in a pause.
    committed: String,
    /// Last text sent to the overlay, to avoid repainting identical content.
    last_emitted: String,
}

impl PartialState {
    /// Scans newly captured raw audio (`chunk`, starting at absolute `base`)
    /// and returns the ranges of phrases that just ended in a pause.
    fn scan_pauses(
        &mut self,
        chunk: &[f32],
        base: usize,
        frame: usize,
        silence_frames: usize,
    ) -> Vec<(usize, usize)> {
        debug_assert!(frame > 0);
        let mut closed = Vec::new();
        let frames = chunk.len() / frame;
        for index in 0..frames {
            let start = base + index * frame;
            let frame_samples = &chunk[index * frame..(index + 1) * frame];
            if rms(frame_samples) < MIN_VOICE_RMS {
                self.silence_run += frame;
                if self.has_speech && self.silence_run >= silence_frames * frame {
                    let end = start + frame - self.silence_run;
                    closed.push((self.segment_start, end));
                    self.segment_start = start + frame;
                    self.silence_run = 0;
                    self.has_speech = false;
                }
            } else {
                self.silence_run = 0;
                self.has_speech = true;
            }
        }
        self.scan = base + frames * frame;
        closed
    }

    /// Commits the open phrase when it grew past `max_samples` without a pause,
    /// so a continuous speaker never makes each preview slower than real time.
    fn take_forced(&mut self, total: usize, max_samples: usize) -> Option<(usize, usize)> {
        if self.has_speech && total.saturating_sub(self.segment_start) >= max_samples {
            let range = (self.segment_start, total);
            self.segment_start = total;
            self.silence_run = 0;
            self.has_speech = false;
            Some(range)
        } else {
            None
        }
    }
}

struct RuntimeState {
    worker: Option<Worker>,
    worker_exe: Option<PathBuf>,
    model_dir: Option<PathBuf>,
    loaded_language: Option<Language>,
    engine_error: Option<String>,
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
                EngineEvent::Error(trf!(
                    "无法创建网络运行时: {e}",
                    "Cannot create the network runtime: {e}"
                )),
            );
            return;
        }
    };

    let mut state = RuntimeState {
        worker: None,
        worker_exe: None,
        model_dir: None,
        loaded_language: None,
        engine_error: None,
    };

    if let Err(err) = prepare_model(&mut state, &runtime, initial_language, &tx, &ctx) {
        state.engine_error = Some(err.clone());
        emit(&tx, &ctx, EngineEvent::ModelError(err));
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
                Ok(EngineCommand::ReloadModel) => {
                    let language = state.loaded_language.unwrap_or(initial_language);
                    if let Err(err) = prepare_model(&mut state, &runtime, language, &tx, &ctx) {
                        state.engine_error = Some(err.clone());
                        emit(&tx, &ctx, EngineEvent::ModelError(err));
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
                Ok(EngineCommand::ReloadModel) => {
                    // The current recording wins; the tray action can be used
                    // again afterwards.
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    shutdown_requested = true;
                    break;
                }
            }
        }

        if shutdown_requested {
            if let Some(mut current) = session.take()
                && let Some(recorder) = current.recorder.take()
            {
                let _ = recorder.stop();
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
            refresh_preview(&mut state, current, &tx, &ctx);
            current.next_partial = Instant::now() + PARTIAL_INTERVAL;
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
                EngineEvent::Status(
                    ts!(
                        "已达到 60 秒上限，正在识别…",
                        "60 second limit reached, recognizing…",
                    )
                    .to_string(),
                ),
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

    if let Some(mut current) = session
        && let Some(recorder) = current.recorder.take()
    {
        let _ = recorder.stop();
    }
}

/// Refreshes the overlay preview without re-decoding the whole recording.
///
/// Audio is cut at pauses: every closed phrase is decoded once and kept, and
/// only the phrase currently being spoken is decoded on each refresh. The
/// per-refresh cost stays bounded no matter how long the user holds the key,
/// which is what turns the previous ~1.3 second refresh into a near-real-time
/// one.
fn refresh_preview(
    state: &mut RuntimeState,
    session: &mut Session,
    tx: &Sender<UiEvent>,
    ctx: &egui::Context,
) {
    let recorder = session.recorder.as_ref().expect("active recorder");
    let rate = recorder.sample_rate();
    let total = recorder.sample_count();
    let frame = (rate as usize * VAD_FRAME_MS as usize / 1000).max(1);
    let silence_frames =
        ((rate as usize * SEGMENT_SILENCE_MS as usize) / 1000 / frame).max(1);
    let max_open = (rate as f64 * MAX_OPEN_SEGMENT_SECONDS as f64) as usize;

    let scan = session.partial.scan;
    let chunk = recorder.snapshot_range(scan, total);
    let mut ranges = session.partial.scan_pauses(&chunk, scan, frame, silence_frames);
    if let Some(range) = session.partial.take_forced(total, max_open) {
        ranges.push(range);
    }

    for (start, end) in ranges {
        if end <= start {
            continue;
        }
        let raw = recorder.snapshot_range(start, end);
        if rms(&raw) < MIN_VOICE_RMS {
            continue;
        }
        let audio = resample_linear(&raw, rate, TARGET_RATE);
        if duration_seconds(audio.len(), TARGET_RATE) < SEGMENT_MIN_SECONDS {
            continue;
        }
        match transcribe(state, &audio, TARGET_RATE, session.language) {
            Ok(text) if has_meaningful_text(&text) => {
                session.partial.committed = join_preview(&session.partial.committed, &text);
            }
            Ok(_) => {}
            Err(err) => eprintln!("VoxKey: partial decode failed: {err}"),
        }
    }

    // The phrase still being spoken: decode it and show it after the text that
    // is already committed. It is capped by a pause or by `max_open`, so the
    // cost of one refresh stays roughly constant. When the user pauses, the
    // just-closed phrase is emitted on its own, without waiting for more speech.
    let mut text = session.partial.committed.clone();
    if session.partial.has_speech {
        let start = session.partial.segment_start;
        if total > start {
            let raw = recorder.snapshot_range(start, total);
            if rms(&raw) >= MIN_VOICE_RMS {
                let audio = resample_linear(&raw, rate, TARGET_RATE);
                if duration_seconds(audio.len(), TARGET_RATE) >= PREVIEW_MIN_SECONDS {
                    match transcribe(state, &audio, TARGET_RATE, session.language) {
                        Ok(tail) if has_meaningful_text(&tail) => {
                            text = join_preview(&session.partial.committed, &tail);
                        }
                        Ok(_) => {}
                        Err(err) => eprintln!("VoxKey: partial decode failed: {err}"),
                    }
                }
            }
        }
    }

    if has_meaningful_text(&text) && text != session.partial.last_emitted {
        session.partial.last_emitted = text.clone();
        emit(tx, ctx, EngineEvent::Transcript(text));
    }
}

/// Prepares the worker binary and the model, then loads the recognizer.
/// `ModelError` is emitted by the caller so every failure path ends up in the
/// same UI state.
fn prepare_model(
    state: &mut RuntimeState,
    runtime: &tokio::runtime::Runtime,
    language: Language,
    tx: &Sender<UiEvent>,
    ctx: &egui::Context,
) -> Result<(), String> {
    emit(
        tx,
        ctx,
        EngineEvent::Status(
            ts!(
                "正在准备本地语音引擎…",
                "Preparing the local speech engine…",
            )
            .to_string(),
        ),
    );

    let mut progress = |downloaded: u64, total: u64, label: &str| {
        emit(
            tx,
            ctx,
            EngineEvent::Download {
                downloaded,
                total,
                label: label.to_string(),
            },
        );
    };

    let exe = worker::ensure(runtime, &mut progress).map_err(|err| {
        trf!(
            "语音引擎不可用：{err}",
            "The speech engine is unavailable: {err}"
        )
    })?;
    state.worker_exe = Some(exe.clone());

    emit(
        tx,
        ctx,
        EngineEvent::Status(
            ts!("正在准备本地语音模型…", "Preparing the local speech model…",).to_string(),
        ),
    );
    let dir = model::ensure_model(runtime, &mut progress)?;

    // Once the files are on disk the overlay must stop looking like a stalled
    // download, even while the recognizer is still loading.
    emit(
        tx,
        ctx,
        EngineEvent::Status(ts!("正在加载语音模型…", "Loading the speech model…").to_string()),
    );

    state.worker = None;
    let mut engine = Worker::spawn(&exe)?;
    engine.load(&dir, language.asr_code())?;

    state.worker = Some(engine);
    state.loaded_language = Some(language);
    state.model_dir = Some(dir);
    state.engine_error = None;
    emit(tx, ctx, EngineEvent::Ready);
    Ok(())
}

fn start_session(
    state: &mut RuntimeState,
    language: Language,
    refine: RefineConfig,
    tx: &Sender<UiEvent>,
    ctx: &egui::Context,
) -> Result<Session, String> {
    if state.model_dir.is_none() {
        return Err(state.engine_error.clone().unwrap_or_else(|| {
            ts!(
                "语音引擎尚未就绪，请重启应用后重试",
                "The speech engine is not ready yet; restart VoxKey and try again",
            )
            .to_string()
        }));
    }

    // A crashed or killed worker is transparently restarted on the next press.
    let dead = match state.worker.as_mut() {
        Some(worker) => !worker.is_alive(),
        None => true,
    };
    if dead {
        emit(
            tx,
            ctx,
            EngineEvent::Status(ts!("正在加载语音模型…", "Loading the speech model…").to_string()),
        );
        restart_worker(state, language, tx, ctx)?;
    } else if state.loaded_language != Some(language) {
        emit(
            tx,
            ctx,
            EngineEvent::Status(
                trf!("正在加载{}模型…", "Loading the {} model…", language.label()).to_string(),
            ),
        );
        let dir = state
            .model_dir
            .clone()
            .ok_or_else(|| "语音模型未就绪".to_string())?;
        let worker = state
            .worker
            .as_mut()
            .ok_or_else(|| "语音引擎未运行".to_string())?;
        worker.load(&dir, language.asr_code())?;
        state.loaded_language = Some(language);
    }

    emit(
        tx,
        ctx,
        EngineEvent::Status(ts!("正在聆听…", "Listening…").to_string()),
    );
    let recorder = Recorder::start()
        .map_err(|e| trf!("麦克风不可用: {e}", "The microphone is unavailable: {e}"))?;
    emit(tx, ctx, EngineEvent::RecordingStarted);

    let now = Instant::now();
    Ok(Session {
        recorder: Some(recorder),
        language,
        refine,
        next_level: now,
        next_partial: now + Duration::from_millis(250),
        partial: PartialState::default(),
    })
}

fn restart_worker(
    state: &mut RuntimeState,
    language: Language,
    tx: &Sender<UiEvent>,
    ctx: &egui::Context,
) -> Result<(), String> {
    let exe = state
        .worker_exe
        .clone()
        .ok_or_else(|| "语音引擎尚未安装".to_string())?;
    let dir = state
        .model_dir
        .clone()
        .ok_or_else(|| "语音模型未就绪".to_string())?;
    state.worker = None;
    let mut engine = Worker::spawn(&exe)?;
    engine.load(&dir, language.asr_code())?;
    state.worker = Some(engine);
    state.loaded_language = Some(language);
    emit(tx, ctx, EngineEvent::Ready);
    Ok(())
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
        .ok_or_else(|| ts!("录音状态异常", "Recorder state is invalid").to_string())?;
    let (samples, sample_rate, audio_error) = recorder.stop();
    if let Some(err) = audio_error {
        eprintln!("VoxKey: audio error while recording: {err}");
    }

    if duration_seconds(samples.len(), sample_rate) < MIN_RECORDING_SECONDS {
        return Err(ts!(
            "没有检测到语音，请按住快捷键说话",
            "No speech detected; hold the hotkey and speak",
        )
        .to_string());
    }

    // SenseVoice will happily "recognize" room noise as a character or
    // punctuation mark. Require some real acoustic energy before trusting it.
    if rms(&samples) < MIN_VOICE_RMS {
        return Err(ts!(
            "没有检测到语音，请按住快捷键说话",
            "No speech detected; hold the hotkey and speak",
        )
        .to_string());
    }

    if state.worker.is_none() {
        return Err(ts!("语音模型未就绪", "The speech model is not ready").to_string());
    }

    emit(tx, ctx, EngineEvent::Recognizing);
    let mut text = transcribe(state, &samples, sample_rate, session.language)?;
    if !has_meaningful_text(&text) {
        return Err(ts!(
            "没有识别出文字，请再试一次",
            "No text was recognized; please try again",
        )
        .to_string());
    }
    // A single character decoded from a faint recording is almost always
    // noise; ask for a clearer retry instead of inserting garbage.
    if meaningful_char_count(&text) <= 1 && rms(&samples) < SOFT_SINGLE_CHAR_RMS {
        return Err(ts!(
            "没有识别出文字，请再试一次",
            "No text was recognized; please try again",
        )
        .to_string());
    }

    let mut warnings = Vec::new();
    if session.refine.enabled {
        emit(tx, ctx, EngineEvent::Refining);
        match runtime.block_on(llm::refine_async(&session.refine, &text)) {
            Ok(refined) => {
                text = postprocess(session.language, &refined);
            }
            Err(err) => {
                warnings.push(trf!(
                    "LLM 纠错已跳过：{err}",
                    "LLM refinement was skipped: {err}"
                ));
            }
        }
    }

    if !has_meaningful_text(&text) {
        return Err(ts!(
            "纠错后的文本为空，已取消插入",
            "The refined text was empty, so nothing was inserted",
        )
        .to_string());
    }

    emit(
        tx,
        ctx,
        EngineEvent::Status(ts!("正在插入文字…", "Inserting text…").to_string()),
    );
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
        Err(err) => Err(trf!("无法插入文字：{err}", "Cannot insert the text: {err}")),
    }
}

fn transcribe(
    state: &mut RuntimeState,
    samples: &[f32],
    sample_rate: u32,
    language: Language,
) -> Result<String, String> {
    let audio = resample_linear(samples, sample_rate, TARGET_RATE);
    let worker = state
        .worker
        .as_mut()
        .ok_or_else(|| ts!("语音引擎未运行", "The speech engine is not running").to_string())?;
    let raw = match worker.transcribe(&audio) {
        Ok(raw) => raw,
        Err(err) => {
            // The pipe is broken: drop the child so the next press restarts it.
            state.worker = None;
            state.loaded_language = None;
            return Err(err);
        }
    };
    Ok(postprocess(language, &raw))
}

/// Concatenates committed preview text and the phrase being decoded. Latin
/// words need a separating space; CJK text and punctuation do not.
fn join_preview(prefix: &str, tail: &str) -> String {
    let prefix = prefix.trim_end();
    let tail = tail.trim_start();
    if prefix.is_empty() {
        return tail.to_string();
    }
    if tail.is_empty() {
        return prefix.to_string();
    }
    let last = prefix.chars().next_back().unwrap_or(' ');
    let first = tail.chars().next().unwrap_or(' ');
    let need_space = last.is_ascii()
        && !last.is_ascii_whitespace()
        && first.is_ascii_alphanumeric();
    if need_space {
        format!("{prefix} {tail}")
    } else {
        format!("{prefix}{tail}")
    }
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
    }

    #[test]
    fn preview_join_needs_spaces_only_between_latin_words() {
        assert_eq!(join_preview("", "你好"), "你好");
        assert_eq!(join_preview("你好，", "世界。"), "你好，世界。");
        assert_eq!(join_preview("Hello", "world"), "Hello world");
        assert_eq!(join_preview("Hello.", "World"), "Hello. World");
        assert_eq!(join_preview("你好", ""), "你好");
    }

    fn frames(count: usize, energy: f32, frame: usize) -> Vec<f32> {
        vec![energy; count * frame]
    }

    #[test]
    fn pauses_close_the_current_segment() {
        let frame = 160; // 10 ms at 16 kHz
        let silence_frames = 28; // 280 ms
        let mut state = PartialState::default();
        let mut chunk = frames(10, 0.1, frame);
        chunk.extend(frames(30, 0.0, frame));
        let closed = state.scan_pauses(&chunk, 0, frame, silence_frames);
        assert_eq!(closed, vec![(0, 10 * frame)]);
        assert_eq!(state.segment_start, 38 * frame);
        assert!(!state.has_speech);
        assert_eq!(state.scan, 40 * frame);
    }

    #[test]
    fn leading_silence_is_not_a_segment() {
        let frame = 160;
        let mut state = PartialState::default();
        let chunk = frames(30, 0.0, frame);
        let closed = state.scan_pauses(&chunk, 0, frame, 28);
        assert!(closed.is_empty());
        assert!(!state.has_speech);
    }

    #[test]
    fn continuous_speech_is_committed_at_the_cap() {
        let frame = 160;
        let mut state = PartialState::default();
        state.scan_pauses(&frames(20, 0.1, frame), 0, frame, 28);
        let cap = 30 * frame;
        let forced = state.take_forced(40 * frame, cap);
        assert_eq!(forced, Some((0, 40 * frame)));
        assert_eq!(state.segment_start, 40 * frame);
        assert!(!state.has_speech);
        assert_eq!(state.take_forced(50 * frame, cap), None);
    }
}
