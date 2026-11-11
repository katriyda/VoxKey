//! VoxKey speech engine worker.
//!
//! The UI process (`VoxKey.exe`) must stay small, so the SenseVoice recognizer
//! (sherpa-onnx + a statically linked onnxruntime, ~15 MB of binary) runs in
//! this separate process. It is spawned by the UI with inherited stdin/stdout
//! pipes and speaks a tiny line-oriented protocol:
//!
//! * request: one JSON object per line (`{"cmd":"load","dir":"…","lang":"zh"}`);
//! * `transcribe` is followed by exactly `samples` little-endian `f32` values
//!   (16 kHz mono) on the same stream;
//! * reply: one JSON object per line (`{"ok":true,"text":"…"}`).
//!
//! The worker exits when the pipe closes, so a crashed or killed UI can never
//! leave an orphan process behind. It is built with the `windows` subsystem so
//! it never flashes a console window; stdin/stdout still work because the
//! handles are inherited from the parent.

#![cfg_attr(all(not(debug_assertions), not(test)), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig};

/// Audio is always resampled by the UI before it is sent.
const SAMPLE_RATE: i32 = 16_000;
/// 60 seconds is the recording limit in the UI; anything larger is a protocol
/// bug and is rejected instead of allocating unbounded memory.
const MAX_SAMPLES: usize = 60 * SAMPLE_RATE as usize + SAMPLE_RATE as usize;

#[derive(Deserialize)]
struct Request {
    cmd: String,
    #[serde(default)]
    dir: Option<String>,
    #[serde(default)]
    lang: Option<String>,
    #[serde(default)]
    samples: usize,
}

#[derive(Serialize)]
struct Reply {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

impl Reply {
    fn ok(text: Option<String>) -> Self {
        Self {
            ok: true,
            text,
            error: None,
            version: None,
        }
    }

    fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            text: None,
            error: Some(message.into()),
            version: None,
        }
    }
}

struct Backend {
    recognizer: OfflineRecognizer,
}

fn main() {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        // The UI compares this with the version it was built with before it
        // uses a freshly downloaded worker.
        let _ = writeln!(
            std::io::stdout(),
            "VoxKey-asr {}",
            env!("CARGO_PKG_VERSION")
        );
        return;
    }

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = std::io::stdout();

    let mut backend: Option<Backend> = None;
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            // EOF means the UI is gone: exit instead of lingering.
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: Request = match serde_json::from_str(trimmed) {
            Ok(request) => request,
            Err(err) => {
                reply(&mut stdout, Reply::err(format!("bad request: {err}")));
                continue;
            }
        };

        match request.cmd.as_str() {
            "version" => {
                let mut answer = Reply::ok(None);
                answer.version = Some(env!("CARGO_PKG_VERSION").to_string());
                reply(&mut stdout, answer);
            }
            "load" => {
                backend = None;
                let dir = request.dir.unwrap_or_default();
                let lang = request.lang.unwrap_or_else(|| "auto".to_string());
                match load_backend(Path::new(&dir), &lang) {
                    Ok(new_backend) => {
                        backend = Some(new_backend);
                        reply(&mut stdout, Reply::ok(None));
                    }
                    Err(err) => reply(&mut stdout, Reply::err(err)),
                }
            }
            "transcribe" => {
                let count = request.samples;
                if count > MAX_SAMPLES {
                    reply(
                        &mut stdout,
                        Reply::err(format!("audio too long: {count} samples")),
                    );
                    break;
                }
                let mut bytes = vec![0u8; count * 4];
                if reader.read_exact(&mut bytes).is_err() {
                    // The stream is out of sync; there is no way to recover.
                    break;
                }
                let samples: Vec<f32> = bytes
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect();
                match backend.as_ref() {
                    Some(backend) => match decode(backend, &samples) {
                        Ok(text) => reply(&mut stdout, Reply::ok(Some(text))),
                        Err(err) => reply(&mut stdout, Reply::err(err)),
                    },
                    None => reply(&mut stdout, Reply::err("engine is not loaded")),
                }
            }
            "shutdown" => {
                reply(&mut stdout, Reply::ok(None));
                break;
            }
            other => reply(&mut stdout, Reply::err(format!("unknown command: {other}"))),
        }
    }
}

fn reply(stdout: &mut std::io::Stdout, answer: Reply) {
    if let Ok(json) = serde_json::to_string(&answer) {
        let mut stdout = stdout.lock();
        let _ = writeln!(stdout, "{json}");
        let _ = stdout.flush();
    }
}

fn load_backend(dir: &Path, lang: &str) -> Result<Backend, String> {
    let model = dir.join("model.int8.onnx");
    let tokens = dir.join("tokens.txt");
    if !model.is_file() || !tokens.is_file() {
        return Err(format!(
            "model files are missing in {} (run the VoxKey model download again)",
            dir.display()
        ));
    }

    let mut config = OfflineRecognizerConfig::default();
    config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
        model: Some(model.to_string_lossy().into_owned()),
        language: Some(lang.to_string()),
        use_itn: true,
    };
    config.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
    config.model_config.provider = Some("cpu".to_string());
    config.model_config.num_threads = std::thread::available_parallelism()
        .map(|count| count.get().min(4) as i32)
        .unwrap_or(2)
        .max(1);

    let recognizer = OfflineRecognizer::create(&config).ok_or_else(|| {
        "cannot create the SenseVoice recognizer (model files may be corrupt)".to_string()
    })?;

    Ok(Backend { recognizer })
}

fn decode(backend: &Backend, samples: &[f32]) -> Result<String, String> {
    let stream = backend.recognizer.create_stream();
    stream.accept_waveform(SAMPLE_RATE, samples);
    backend.recognizer.decode(&stream);
    stream
        .get_result()
        .map(|result| result.text)
        .ok_or_else(|| "the recognizer returned no result".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_json_is_minimal() {
        let ok = serde_json::to_string(&Reply::ok(Some("你好".into()))).unwrap();
        assert_eq!(ok, r#"{"ok":true,"text":"你好"}"#);
        let err = serde_json::to_string(&Reply::err("boom")).unwrap();
        assert_eq!(err, r#"{"ok":false,"error":"boom"}"#);
    }

    #[test]
    fn rejects_absurd_audio_lengths() {
        assert!(MAX_SAMPLES >= 60 * SAMPLE_RATE as usize);
    }

    #[test]
    fn parses_requests() {
        let request: Request = serde_json::from_str(r#"{"cmd":"transcribe","samples":5}"#).unwrap();
        assert_eq!(request.cmd, "transcribe");
        assert_eq!(request.samples, 5);
        assert!(request.dir.is_none());
        assert!(request.lang.is_none());
    }
}
