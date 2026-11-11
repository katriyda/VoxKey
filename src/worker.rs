//! The SenseVoice engine process: acquisition, supervision and IPC.
//!
//! Keeping sherpa-onnx + onnxruntime (≈15 MB of binary) in the UI executable
//! made the "single file" 41 MB. Instead the engine runs as `VoxKey-asr.exe`,
//! downloaded on first use into `%LOCALAPPDATA%\VoxKey\bin\` — next to the
//! 229 MB model it already downloads — and spoken to over stdin/stdout.
//!
//! * `ensure` returns the path of a usable worker (env override, already
//!   installed, a sibling build for development, or a fresh download).
//! * `Worker` spawns it with `CREATE_NO_WINDOW`, loads the recognizer and
//!   streams 16 kHz mono `f32` audio per request.
//! * The worker exits when the pipe closes, so a killed UI leaves no orphan.

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::i18n::trf;
use crate::logging;

/// Repository the worker and the UI are released from together.
pub const REPO: &str = "katriyda/VoxKey";
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// SHA-256 of the `VoxKey-asr.exe` built by the same CI run as this UI. CI
/// exports it before compiling the UI, so a release build always has it.
const ASR_SHA256: Option<&str> = option_env!("VOXKEY_ASR_SHA256");
/// GitHub is the canonical source; the proxy accepts `Range` as well.
const MIRRORS: [&str; 2] = [
    "https://github.com",
    "https://ghfast.top/https://github.com",
];
const ASSET_NAME: &str = "VoxKey-asr.exe";
const PART_SUFFIX: &str = ".part";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const REPORT_INTERVAL: Duration = Duration::from_millis(350);
const MIRROR_ROUNDS: u32 = 2;
const MIN_SIZE: u64 = 1_000_000;
/// `CREATE_NO_WINDOW`: the worker is a console-subsystem binary, and spawning
/// it from a GUI process would otherwise flash a black window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn install_dir() -> PathBuf {
    crate::config::data_dir().join("bin")
}

/// The worker build for the running UI version. The version is part of the
/// file name so an update never has to overwrite a still-running worker.
pub fn exe_path() -> PathBuf {
    install_dir().join(format!("VoxKey-asr-{VERSION}.exe"))
}

/// Returns a usable worker binary, downloading it when necessary.
///
/// `progress(downloaded, total, label)` drives the same overlay as the model
/// download; `total` is 0 until the server reports the asset size.
pub fn ensure(
    runtime: &tokio::runtime::Runtime,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("VOXKEY_ASR_EXE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(trf!(
            "VOXKEY_ASR_EXE 指向的文件不存在：{}",
            "VOXKEY_ASR_EXE points at a missing file: {}",
            path.display()
        ));
    }

    let installed = exe_path();
    if installed.is_file() {
        return Ok(installed);
    }

    // Development builds are typically run from `target/release`, where the
    // worker was just built next to the UI. Release builds carry a pinned
    // checksum and therefore always download the CI-built worker.
    if ASR_SHA256.is_none()
        && let Some(sibling) = development_worker()
    {
        logging::warn(&format!(
            "no pinned worker checksum in this build, using {}",
            sibling.display()
        ));
        return Ok(sibling);
    }

    download(runtime, &installed, progress)?;
    Ok(installed)
}

fn development_worker() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    let dir = current.parent()?;
    let candidate = dir.join("VoxKey-asr.exe");
    candidate.is_file().then_some(candidate)
}

/// Removes workers from earlier versions and stale partial downloads. Called
/// once at startup; files locked by a still-running worker are skipped.
pub fn cleanup_old_versions() {
    let current = exe_path();
    let keep = [
        current
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_string(),
        format!(
            "{}{PART_SUFFIX}",
            current
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("VoxKey-asr")
        ),
    ];
    let Ok(entries) = std::fs::read_dir(install_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == current {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // Keep the running version's binary and its resumable partial download;
        // everything else (older builds, abandoned partials) can go.
        if keep.iter().any(|kept| kept == name) {
            continue;
        }
        if name.starts_with("VoxKey-asr") && (name.ends_with(".exe") || name.ends_with(PART_SUFFIX))
        {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn download(
    runtime: &tokio::runtime::Runtime,
    target: &Path,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<(), String> {
    let Some(pinned) = ASR_SHA256 else {
        return Err(trf!(
            "这个版本没有内置语音引擎的校验值，无法自动下载。请把 VoxKey-asr.exe 放到 {}，或设置 VOXKEY_ASR_EXE。",
            "This build has no pinned checksum for the speech engine, so it cannot download one. Put VoxKey-asr.exe into {}, or set VOXKEY_ASR_EXE.",
            install_dir().display()
        ));
    };

    std::fs::create_dir_all(install_dir()).map_err(|err| {
        trf!(
            "无法创建语音引擎目录 {}：{err}",
            "Cannot create the engine directory {}: {err}",
            install_dir().display()
        )
    })?;

    runtime.block_on(download_async(target, pinned, progress))
}

async fn download_async(
    target: &Path,
    pinned: &str,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<(), String> {
    let part = target.with_extension("exe.part");
    let label = crate::i18n::s("语音引擎", "Speech engine").to_string();
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("VoxKey/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|err| format!("cannot create the download client: {err}"))?;

    let mut last_error = String::from("unknown error");
    for round in 0..MIRROR_ROUNDS {
        for mirror in MIRRORS {
            let url = format!("{mirror}/{REPO}/releases/download/v{VERSION}/{ASSET_NAME}");
            match try_once(&client, &url, &part, target, pinned, progress, &label).await {
                Ok(()) => {
                    logging::info(&format!("speech engine downloaded from {mirror}"));
                    return Ok(());
                }
                Err(err) => {
                    logging::warn(&format!("{url} failed: {err}"));
                    last_error = err;
                }
            }
        }
        if round + 1 < MIRROR_ROUNDS {
            progress(
                std::fs::metadata(&part).map(|meta| meta.len()).unwrap_or(0),
                0,
                crate::i18n::s("连接不稳，稍后重试…", "connection unstable, retrying…"),
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    Err(trf!(
        "无法下载语音引擎：{last_error}。已尝试 GitHub 与镜像，临时文件保留在 {}。",
        "Cannot download the speech engine: {last_error}. GitHub and the mirror were both tried; the partial file stays at {}.",
        part.display()
    ))
}

async fn try_once(
    client: &reqwest::Client,
    url: &str,
    part: &Path,
    target: &Path,
    pinned: &str,
    progress: &mut dyn FnMut(u64, u64, &str),
    label: &str,
) -> Result<(), String> {
    use reqwest::StatusCode;
    use reqwest::header::{CONTENT_RANGE, RANGE};
    use tokio::io::AsyncWriteExt;

    let mut offset = part
        .metadata()
        .map(|meta| if meta.is_file() { meta.len() } else { 0 })
        .unwrap_or(0);
    if offset > 0 && offset < MIN_SIZE {
        // A tiny partial file cannot be a prefix of a real build; assume the
        // earlier attempt died before the first byte was useful.
        let _ = std::fs::remove_file(part);
        offset = 0;
    }

    let mut request = client.get(url);
    if offset > 0 {
        request = request.header(RANGE, format!("bytes={offset}-"));
    }
    let mut response = request
        .send()
        .await
        .map_err(|err| format!("connection failed: {err}"))?;

    let status = response.status();
    let total;
    if status == StatusCode::PARTIAL_CONTENT {
        if offset > 0 {
            let start = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_content_range_start);
            if let Some(start) = start
                && start != offset
            {
                let _ = std::fs::remove_file(part);
                return Err(format!(
                    "resume offset mismatch (expected {offset}, got {start}), restarted"
                ));
            }
        }
        total = response
            .content_length()
            .map(|length| length + offset)
            .unwrap_or(0);
    } else if status == StatusCode::RANGE_NOT_SATISFIABLE {
        let _ = std::fs::remove_file(part);
        return Err("resume request rejected, partial file dropped".to_string());
    } else if status == StatusCode::OK {
        if offset > 0 {
            logging::warn("the server ignored the resume request, restarting");
            offset = 0;
        }
        total = response.content_length().unwrap_or(0);
    } else {
        return Err(format!("HTTP {status}"));
    }

    if total > 0 && total < MIN_SIZE {
        return Err(format!("asset is implausibly small ({total} bytes)"));
    }

    let mut file = if offset > 0 {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(part)
            .await
            .map_err(|err| format!("cannot open the partial file: {err}"))?
    } else {
        tokio::fs::File::create(part)
            .await
            .map_err(|err| format!("cannot create the partial file: {err}"))?
    };

    progress(offset, total, label);
    let mut received = offset;
    let mut last_report = Instant::now();

    loop {
        let chunk = match tokio::time::timeout(STALL_TIMEOUT, response.chunk()).await {
            Err(_) => {
                let _ = file.flush().await;
                return Err(format!(
                    "stalled for {}s at {received} bytes (resumes next start)",
                    STALL_TIMEOUT.as_secs()
                ));
            }
            Ok(Ok(None)) => break,
            Ok(Ok(Some(chunk))) => chunk,
            Ok(Err(err)) => {
                let _ = file.flush().await;
                return Err(format!("download interrupted at {received} bytes: {err}"));
            }
        };
        received += chunk.len() as u64;
        if let Err(err) = file.write_all(&chunk).await {
            let _ = std::fs::remove_file(part);
            return Err(format!("cannot write the partial file: {err}"));
        }
        if last_report.elapsed() >= REPORT_INTERVAL {
            progress(received, total, label);
            last_report = Instant::now();
        }
    }

    if let Err(err) = file.flush().await {
        return Err(format!("cannot write the partial file: {err}"));
    }
    drop(file);

    if received == 0 {
        return Err("the server sent no data".to_string());
    }
    if total > 0 && received != total {
        return Err(format!(
            "download incomplete ({received} / {total} bytes), resumes next start"
        ));
    }

    progress(
        received,
        total,
        crate::i18n::s("校验语音引擎", "verifying the engine"),
    );
    if !crate::sha256::file_matches(part, pinned)
        .map_err(|err| format!("cannot read the downloaded worker for verification: {err}"))?
    {
        let _ = std::fs::remove_file(part);
        return Err("the speech engine does not match its pinned SHA-256".to_string());
    }

    let head = {
        use std::io::Read;
        let mut file = std::fs::File::open(part)
            .map_err(|err| format!("cannot read the downloaded worker: {err}"))?;
        let mut head = [0u8; 1024];
        let read = file
            .read(&mut head)
            .map_err(|err| format!("cannot read the downloaded worker: {err}"))?;
        head[..read].to_vec()
    };
    if crate::update::pe_machine(&head) != Some(0x8664) {
        let _ = std::fs::remove_file(part);
        return Err("the speech engine is not an x86-64 Windows executable".to_string());
    }

    if target.exists() {
        let _ = std::fs::remove_file(target);
    }
    std::fs::rename(part, target)
        .map_err(|err| format!("cannot install the speech engine: {err}"))?;
    Ok(())
}

fn parse_content_range_start(value: &str) -> Option<u64> {
    let rest = value.trim().strip_prefix("bytes")?.trim();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim();
    let range = rest.split('/').next()?.trim();
    let start = range.split('-').next()?.trim();
    start.parse().ok()
}

// -------------------------------------------------------------------------
// IPC
// -------------------------------------------------------------------------

#[derive(Deserialize)]
struct Reply {
    ok: bool,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

impl Reply {
    fn into_result(self) -> Result<Option<String>, String> {
        if self.ok {
            Ok(self.text)
        } else {
            Err(self
                .error
                .unwrap_or_else(|| "the speech engine reported an error".to_string()))
        }
    }
}

pub struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    /// Recognition language the worker currently has loaded.
    pub language: Option<String>,
}

impl Worker {
    pub fn spawn(exe: &Path) -> Result<Self, String> {
        let mut command = Command::new(exe);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        let mut child = command.spawn().map_err(|err| {
            trf!(
                "无法启动语音引擎：{err}",
                "Cannot start the speech engine: {err}"
            )
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "the speech engine has no stdin pipe".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "the speech engine has no stdout pipe".to_string())?;
        logging::info("speech engine started");
        Ok(Self {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            language: None,
        })
    }

    /// Loads the recognizer for `language` (a SenseVoice token such as `zh`).
    /// Reloading costs about a second, so it happens only on a language change.
    pub fn load(&mut self, dir: &Path, language: &str) -> Result<(), String> {
        let dir = dir.display().to_string();
        self.send(&serde_json::json!({
            "cmd": "load",
            "dir": dir,
            "lang": language,
        }))?;
        self.read()?.into_result()?;
        self.language = Some(language.to_string());
        Ok(())
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
        let header = serde_json::json!({
            "cmd": "transcribe",
            "samples": samples.len(),
        });
        {
            let stdin = self
                .stdin
                .as_mut()
                .ok_or_else(|| "the speech engine is closed".to_string())?;
            writeln!(stdin, "{header}").map_err(|err| pipe_error(&err))?;
            let mut buffer = Vec::with_capacity(4096);
            for chunk in samples.chunks(1024) {
                buffer.clear();
                for sample in chunk {
                    buffer.extend_from_slice(&sample.to_le_bytes());
                }
                stdin.write_all(&buffer).map_err(|err| pipe_error(&err))?;
            }
            stdin.flush().map_err(|err| pipe_error(&err))?;
        }
        let reply = self.read()?;
        let text = reply.into_result()?.unwrap_or_default();
        Ok(text)
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn send(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "the speech engine is closed".to_string())?;
        writeln!(stdin, "{value}").map_err(|err| pipe_error(&err))?;
        stdin.flush().map_err(|err| pipe_error(&err))
    }

    fn read(&mut self) -> Result<Reply, String> {
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .map_err(|err| pipe_error(&err))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Err("the speech engine exited unexpectedly".to_string());
        }
        serde_json::from_str(trimmed)
            .map_err(|err| format!("cannot parse the speech engine reply: {err}"))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // The worker is a child of this process and exits on EOF, but killing
        // it outright also covers a half-dead pipe after an update.
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.stdin = None;
    }
}

fn pipe_error(err: &std::io::Error) -> String {
    trf!(
        "语音引擎通信失败：{err}",
        "Cannot talk to the speech engine: {err}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_range_headers() {
        assert_eq!(parse_content_range_start("bytes 100-199/1234"), Some(100));
        assert_eq!(parse_content_range_start("bytes=0-315893/315894"), Some(0));
        assert_eq!(parse_content_range_start("garbage"), None);
    }

    #[test]
    fn install_path_is_versioned() {
        let path = exe_path();
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            format!("VoxKey-asr-{VERSION}.exe")
        );
    }

    #[test]
    fn replies_are_parsed() {
        let reply: Reply = serde_json::from_str(r#"{"ok":true,"text":"你好"}"#).unwrap();
        assert_eq!(reply.into_result().unwrap().as_deref(), Some("你好"));
        let reply: Reply = serde_json::from_str(r#"{"ok":false,"error":"pong"}"#).unwrap();
        assert_eq!(reply.into_result().unwrap_err(), "pong");
    }
}
