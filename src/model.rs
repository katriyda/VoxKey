//! Model cache: resumable first-run download, integrity verification and
//! offline import.
//!
//! VoxKey uses the multilingual SenseVoice int8 model from sherpa-onnx:
//! Chinese (Mandarin), English, Japanese, Korean and Cantonese.
//!
//! Downloads are pinned to one upstream revision and verified by SHA-256. A
//! partial download is kept as `<file>.part` and resumed with a `Range`
//! request, so an interrupted 239 MB transfer does not start over. Every
//! network attempt has a stall timeout: a socket that stops delivering bytes
//! is treated as a failed attempt, which is what used to leave the progress
//! overlay frozen at 100%.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::i18n::{trf, ts};
use crate::logging;
use crate::sha256;

const REPO: &str = "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17";
const REVISION: &str = "2365baeacb507f821a0c8120fcee3d484dba7a07";

pub const MODEL_FILE: &str = "model.int8.onnx";
pub const TOKENS_FILE: &str = "tokens.txt";
const MODEL_SIZE: u64 = 239_233_841;
const TOKENS_SIZE: u64 = 315_894;
/// SHA-256 of the pinned `model.int8.onnx` (matches the upstream LFS pointer).
const MODEL_SHA256: &str = "c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51";
/// SHA-256 of the pinned `tokens.txt`.
const TOKENS_SHA256: &str = "f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc";

const MIRRORS: [&str; 2] = ["https://huggingface.co", "https://hf-mirror.com"];
const PART_SUFFIX: &str = ".part";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const REPORT_INTERVAL: Duration = Duration::from_millis(350);
/// How many times each mirror is retried (a retry resumes from the bytes that
/// are already on disk). Total attempts are `MIRROR_ROUNDS * MIRRORS.len()`.
const MIRROR_ROUNDS: u32 = 3;

/// One file of the pinned model revision.
#[derive(Clone, Copy, Debug)]
pub struct ModelFile {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

pub const FILES: [ModelFile; 2] = [
    ModelFile {
        name: MODEL_FILE,
        size: MODEL_SIZE,
        sha256: MODEL_SHA256,
    },
    ModelFile {
        name: TOKENS_FILE,
        size: TOKENS_SIZE,
        sha256: TOKENS_SHA256,
    },
];

/// Result of scanning the manual-import folder.
#[derive(Debug)]
pub enum ImportOutcome {
    /// Both files were verified and installed.
    Imported { message: String },
    /// The folder does not exist yet, or is empty: the caller should show the
    /// user where to put the files.
    Empty { dir: PathBuf },
}

pub fn total_bytes() -> u64 {
    FILES.iter().map(|spec| spec.size).sum()
}

pub fn model_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("VOXKEY_MODEL_DIR") {
        return PathBuf::from(path);
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local)
            .join("VoxKey")
            .join("models")
            .join("sensevoice-int8");
    }
    PathBuf::from(".voxkey-models")
}

/// Folder users can drop a manually downloaded model into. Importing from here
/// needs no network and no UI beyond one tray click.
pub fn inbox_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("VOXKEY_IMPORT_DIR") {
        return PathBuf::from(path);
    }
    match std::env::var_os("LOCALAPPDATA") {
        Some(local) => PathBuf::from(local)
            .join("VoxKey")
            .join("models")
            .join("inbox"),
        None => model_dir().join("inbox"),
    }
}

#[allow(dead_code)]
pub fn model_paths() -> (PathBuf, PathBuf) {
    let dir = model_dir();
    (dir.join(MODEL_FILE), dir.join(TOKENS_FILE))
}

/// Bytes of complete, correctly sized files currently on disk.
pub fn cached_bytes() -> u64 {
    let dir = model_dir();
    FILES
        .iter()
        .filter(|spec| valid_file(&dir.join(spec.name), spec.size))
        .map(|spec| spec.size)
        .sum()
}

/// One-line description for the settings window and diagnostics.
pub fn cache_status() -> String {
    let dir = model_dir();
    let mut parts = Vec::new();
    for spec in FILES {
        let path = dir.join(spec.name);
        match fs::metadata(&path) {
            Ok(meta) if meta.is_file() => {
                if meta.len() == spec.size {
                    parts.push(format!("{} ✓", spec.name));
                } else {
                    parts.push(trf!(
                        "{} 不完整（{:.1} / {:.1} MB）",
                        "{} incomplete ({:.1} / {:.1} MB)",
                        spec.name,
                        meta.len() as f64 / 1_000_000.0,
                        spec.size as f64 / 1_000_000.0
                    ));
                }
            }
            _ => parts.push(trf!("{} 缺失", "{} missing", spec.name)),
        }
    }
    trf!(
        "{} · 共 {:.1} / {:.1} MB",
        "{} · {:.1} / {:.1} MB total",
        parts.join(ts!("，", ", ")),
        cached_bytes() as f64 / 1_000_000.0,
        total_bytes() as f64 / 1_000_000.0
    )
}

/// The exact upstream URLs, for the "copy links" button and the README.
pub fn download_links() -> String {
    let mut text = String::new();
    for mirror in MIRRORS {
        for spec in FILES {
            text.push_str(&format!(
                "{mirror}/{REPO}/resolve/{REVISION}/{}\n",
                spec.name
            ));
        }
    }
    text
}

/// Downloads missing files into the cache. Returns the directory containing
/// `model.int8.onnx` and `tokens.txt`.
pub fn ensure_model(
    runtime: &tokio::runtime::Runtime,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<PathBuf, String> {
    let dir = model_dir();
    fs::create_dir_all(&dir).map_err(|e| {
        trf!(
            "无法创建模型缓存目录 {}：{e}",
            "Cannot create model cache directory {}: {e}",
            dir.display()
        )
    })?;
    let total = total_bytes();

    // A manual copy always beats a slow network: check the import folder first
    // so users behind a blocked CDN only need to place two files.
    if cached_bytes() < total {
        match import_from_inbox(&mut *progress) {
            Ok(ImportOutcome::Imported { message }) => {
                logging::info(&format!("已从导入目录安装模型：{message}"));
            }
            Ok(ImportOutcome::Empty { .. }) => {}
            Err(err) => logging::warn(&format!("导入目录中的文件不可用：{err}")),
        }
    }

    let mut done = cached_bytes();
    if done >= total {
        progress(total, total, ts!("模型就绪", "Model ready"));
        return Ok(dir);
    }

    let client = download_client()?;
    runtime.block_on(download_all(
        &client,
        &dir,
        total,
        &mut done,
        &mut *progress,
    ))?;

    if !model_files_present() {
        return Err(trf!(
            "模型文件校验失败。可手动下载后导入（{}），或打开 {} 重试。",
            "Model file verification failed. Download it manually and import it ({}), or open {} to retry.",
            inbox_dir().display(),
            dir.display()
        ));
    }
    progress(total, total, ts!("模型就绪", "Model ready"));
    Ok(dir)
}

fn download_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("VoxKey/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| {
            trf!(
                "创建下载客户端失败：{e}",
                "Failed to create download client: {e}"
            )
        })
}

async fn download_all(
    client: &reqwest::Client,
    dir: &Path,
    total: u64,
    done: &mut u64,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<(), String> {
    for (index, spec) in FILES.iter().enumerate() {
        let dest = dir.join(spec.name);
        if valid_file(&dest, spec.size) {
            continue;
        }
        let base = *done;
        download_file(
            client,
            spec,
            index,
            &dest,
            base,
            total,
            done,
            &mut *progress,
        )
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn download_file(
    client: &reqwest::Client,
    spec: &ModelFile,
    index: usize,
    dest: &Path,
    base: u64,
    total: u64,
    done: &mut u64,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<(), String> {
    let part = dest.with_file_name(format!("{}{PART_SUFFIX}", spec.name));
    let label = trf!(
        "下载 {}（{}/{}）",
        "Downloading {} ({}/{})",
        spec.name,
        index + 1,
        FILES.len()
    );
    let mut last_error = ts!("未知错误", "unknown error").to_string();

    for round in 0..MIRROR_ROUNDS {
        for mirror in MIRRORS {
            let url = format!("{mirror}/{REPO}/resolve/{REVISION}/{}", spec.name);
            match try_once(
                client, &url, spec, &part, dest, base, total, done, progress, &label,
            )
            .await
            {
                Ok(()) => {
                    logging::info(&format!("已从 {mirror} 获取 {}", spec.name));
                    return Ok(());
                }
                Err(err) => {
                    logging::warn(&format!("{url} 尝试失败：{err}"));
                    last_error = err;
                }
            }
        }
        if round + 1 < MIRROR_ROUNDS {
            let wait_seconds = 1u64 << round;
            progress(
                *done,
                total,
                &trf!(
                    "连接不稳，{wait_seconds} 秒后重试…",
                    "Connection unstable, retrying in {wait_seconds}s…"
                ),
            );
            tokio::time::sleep(Duration::from_secs(wait_seconds)).await;
        }
    }

    Err(trf!(
        "无法下载 {}：{last_error}。已尝试 {} 与 {}，临时文件保留在 {}，下次启动会继续。",
        "Cannot download {}: {last_error}. Tried {} and {}; the partial file is kept at {} and will resume on next start.",
        spec.name,
        MIRRORS[0],
        MIRRORS[1],
        part.display()
    ))
}

#[allow(clippy::too_many_arguments)]
async fn try_once(
    client: &reqwest::Client,
    url: &str,
    spec: &ModelFile,
    part: &Path,
    dest: &Path,
    base: u64,
    total: u64,
    done: &mut u64,
    progress: &mut dyn FnMut(u64, u64, &str),
    label: &str,
) -> Result<(), String> {
    use reqwest::StatusCode;
    use reqwest::header::{CONTENT_RANGE, RANGE};
    use tokio::io::AsyncWriteExt;

    let mut offset = part_len(part);
    if offset > spec.size {
        logging::warn(&format!("{} 的临时文件超出预期大小，已重置", spec.name));
        let _ = fs::remove_file(part);
        offset = 0;
    }
    *done = base + offset;

    // The part is already complete: only verification and installation are left.
    if offset == spec.size {
        progress(*done, total, &trf!("校验 {}", "Verifying {}", spec.name));
        if sha256::file_matches(part, spec.sha256).map_err(|err| {
            trf!(
                "无法读取 {} 校验完整性：{err}",
                "Cannot read {} to verify integrity: {err}",
                part.display()
            )
        })? {
            install(part, dest)?;
            *done = base + spec.size;
            progress(*done, total, label);
            return Ok(());
        }
        logging::warn(&format!("{} 的临时文件校验失败，重新下载", spec.name));
        let _ = fs::remove_file(part);
        offset = 0;
        *done = base;
    }

    let mut request = client.get(url);
    if offset > 0 {
        request = request.header(RANGE, format!("bytes={offset}-"));
    }
    let mut response = request
        .send()
        .await
        .map_err(|err| trf!("连接失败：{err}", "Connection failed: {err}"))?;

    let status = response.status();
    if status == StatusCode::PARTIAL_CONTENT {
        if offset > 0 {
            let start = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_content_range_start);
            // A mirror that omits Content-Range is tolerated: the SHA-256
            // check after the transfer catches a wrong starting offset.
            if let Some(start) = start
                && start != offset
            {
                let _ = fs::remove_file(part);
                *done = base;
                return Err(trf!(
                    "续传位置不一致（期望 {offset} 字节，服务器返回 {start}），已重置",
                    "Resume position mismatch (expected {offset} bytes, server returned {start}); file reset"
                ));
            }
        }
    } else if status == StatusCode::RANGE_NOT_SATISFIABLE {
        let _ = fs::remove_file(part);
        *done = base;
        return Err(ts!(
            "续传请求被拒绝，已重置临时文件",
            "Resume request rejected; temporary file reset"
        )
        .to_string());
    } else if status == StatusCode::OK {
        if offset > 0 {
            logging::warn(&format!("{} 忽略了续传请求，从头下载", spec.name));
            offset = 0;
            *done = base;
        }
        if let Some(length) = response.content_length()
            && length != spec.size
        {
            return Err(trf!(
                "远端文件大小异常（期望 {} 字节，实际 {length} 字节）",
                "Remote file has unexpected size (expected {} bytes, got {length} bytes)",
                spec.size
            ));
        }
    } else {
        return Err(format!("HTTP {status}"));
    }

    let mut file = if offset > 0 {
        tokio::fs::OpenOptions::new()
            .write(true)
            .append(true)
            .open(part)
            .await
            .map_err(|err| {
                trf!(
                    "无法打开临时文件：{err}",
                    "Cannot open temporary file: {err}"
                )
            })?
    } else {
        tokio::fs::File::create(part).await.map_err(|err| {
            trf!(
                "无法创建临时文件：{err}",
                "Cannot create temporary file: {err}"
            )
        })?
    };

    progress(*done, total, label);
    let mut received = offset;
    let mut last_report = Instant::now();

    loop {
        let chunk = match tokio::time::timeout(STALL_TIMEOUT, response.chunk()).await {
            Err(_) => {
                let _ = file.flush().await;
                *done = base + received;
                return Err(trf!(
                    "连接停滞超过 {} 秒（已收 {} / {} 字节，下次启动自动续传）",
                    "Connection stalled for over {} seconds (received {} / {} bytes; will resume on next start)",
                    STALL_TIMEOUT.as_secs(),
                    received,
                    spec.size
                ));
            }
            Ok(Ok(None)) => break,
            Ok(Ok(Some(chunk))) => chunk,
            Ok(Err(err)) => {
                let _ = file.flush().await;
                *done = base + received;
                return Err(trf!(
                    "下载中断：{err}（已收 {received} / {} 字节，下次启动自动续传）",
                    "Download interrupted: {err} (received {received} / {} bytes; will resume on next start)",
                    spec.size
                ));
            }
        };

        received += chunk.len() as u64;
        if received > spec.size {
            let _ = fs::remove_file(part);
            *done = base;
            return Err(ts!(
                "远端文件比预期更大，已放弃",
                "Remote file is larger than expected; giving up"
            )
            .to_string());
        }
        if let Err(err) = file.write_all(&chunk).await {
            let _ = fs::remove_file(part);
            *done = base;
            return Err(trf!(
                "写入临时文件失败：{err}",
                "Failed to write temporary file: {err}"
            ));
        }
        *done = base + received;
        if last_report.elapsed() >= REPORT_INTERVAL {
            progress(*done, total, label);
            last_report = Instant::now();
        }
    }

    if let Err(err) = file.flush().await {
        *done = base + received;
        return Err(trf!(
            "写入临时文件失败：{err}",
            "Failed to write temporary file: {err}"
        ));
    }
    drop(file);

    if received != spec.size {
        *done = base + received;
        return Err(trf!(
            "连接提前结束（{received} / {} 字节），下次启动自动续传",
            "Connection ended early ({received} / {} bytes; will resume on next start)",
            spec.size
        ));
    }

    progress(*done, total, &trf!("校验 {}", "Verifying {}", spec.name));
    if !sha256::file_matches(part, spec.sha256).map_err(|err| {
        trf!(
            "无法读取 {} 校验完整性：{err}",
            "Cannot read {} to verify integrity: {err}",
            part.display()
        )
    })? {
        let _ = fs::remove_file(part);
        *done = base;
        return Err(trf!(
            "{} 的 SHA-256 与上游不一致（期望 {}），已删除临时文件",
            "{} SHA-256 does not match upstream (expected {}); temporary file removed",
            spec.name,
            spec.sha256
        ));
    }

    install(part, dest)?;
    *done = base + spec.size;
    progress(*done, total, label);
    Ok(())
}

fn install(part: &Path, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        fs::remove_file(dest).map_err(|err| {
            trf!(
                "无法替换旧的 {}：{err}",
                "Cannot replace the old {}: {err}",
                dest.display()
            )
        })?;
    }
    fs::rename(part, dest).map_err(|err| {
        trf!(
            "无法移动模型文件到 {}：{err}",
            "Cannot move model file to {}: {err}",
            dest.display()
        )
    })
}

fn part_len(part: &Path) -> u64 {
    fs::metadata(part)
        .map(|meta| if meta.is_file() { meta.len() } else { 0 })
        .unwrap_or(0)
}

/// Parses the start offset out of a `Content-Range` header.
///
/// Servers disagree on the spelling: the RFC form is `bytes 100-199/1234`
/// while several CDNs emit `bytes=100-199/1234`, so both are accepted.
fn parse_content_range_start(value: &str) -> Option<u64> {
    let rest = value.trim().strip_prefix("bytes")?.trim();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim();
    let range = rest.split('/').next()?.trim();
    let start = range.split('-').next()?.trim();
    start.parse().ok()
}

fn valid_file(path: &Path, expected_size: u64) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.len() == expected_size)
        .unwrap_or(false)
}

fn model_files_present() -> bool {
    let dir = model_dir();
    FILES
        .iter()
        .all(|spec| valid_file(&dir.join(spec.name), spec.size))
}

// -------------------------------------------------------------------------
// Offline import
// -------------------------------------------------------------------------

/// Scans the import folder and installs a manually downloaded model.
pub fn import_from_inbox(
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<ImportOutcome, String> {
    let dir = inbox_dir();
    if let Err(err) = fs::create_dir_all(&dir) {
        return Err(trf!(
            "无法创建导入目录 {}：{err}",
            "Cannot create import directory {}: {err}",
            dir.display()
        ));
    }
    import_from_dir(&dir, progress)
}

/// Imports `model*.onnx` and `tokens.txt` from an arbitrary folder (the tray
/// action and the startup auto-import both end up here).
pub fn import_from_dir(
    dir: &Path,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<ImportOutcome, String> {
    let candidates = collect_candidates(dir);
    if candidates.is_empty() {
        if contains_any_file(dir) {
            return Err(trf!(
                "在 {} 下没有找到 model*.onnx 或 tokens.txt。若下载的是 zip 压缩包，请先解压再导入。",
                "No model*.onnx or tokens.txt found under {}. If you downloaded a zip archive, extract it first, then import.",
                dir.display()
            ));
        }
        return Ok(ImportOutcome::Empty {
            dir: dir.to_path_buf(),
        });
    }
    import_candidates(&candidates, &model_dir(), dir, progress)
}

fn import_candidates(
    candidates: &[PathBuf],
    dest_dir: &Path,
    source_dir: &Path,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<ImportOutcome, String> {
    fs::create_dir_all(dest_dir).map_err(|err| {
        trf!(
            "无法创建模型缓存目录 {}：{err}",
            "Cannot create model cache directory {}: {err}",
            dest_dir.display()
        )
    })?;
    let total = total_bytes();
    let mut done = 0u64;

    for spec in FILES {
        progress(done, total, &trf!("导入 {}", "Importing {}", spec.name));
        let Some(source) = candidates.iter().find(|path| matches_spec(path, &spec)) else {
            return Err(trf!(
                "{} 里缺少 {}（{:.1} MB / {} 字节）。请把 {} 和 {} 都放进去后再导入。",
                "{} is missing {} ({:.1} MB / {} bytes). Put both {} and {} there, then import again.",
                source_dir.display(),
                spec.name,
                spec.size as f64 / 1_000_000.0,
                spec.size,
                MODEL_FILE,
                TOKENS_FILE
            ));
        };
        let actual_size = fs::metadata(source).map(|meta| meta.len()).unwrap_or(0);
        if actual_size != spec.size {
            return Err(size_mismatch_message(source, &spec, actual_size));
        }
        progress(done, total, &trf!("校验 {}", "Verifying {}", spec.name));
        if !sha256::file_matches(source, spec.sha256).map_err(|err| {
            trf!(
                "无法读取 {}：{err}",
                "Cannot read {}: {err}",
                source.display()
            )
        })? {
            return Err(trf!(
                "{} 的 SHA-256 与上游不一致，文件可能没下载完整。期望 {}。",
                "{} SHA-256 does not match upstream; the file is probably incomplete. Expected {}.",
                source.display(),
                spec.sha256
            ));
        }
        copy_file(source, &dest_dir.join(spec.name))?;
        done += spec.size;
        progress(done, total, &trf!("已导入 {}", "Imported {}", spec.name));
    }

    progress(total, total, ts!("模型就绪", "Model ready"));
    Ok(ImportOutcome::Imported {
        message: trf!(
            "已导入 {MODEL_FILE} 与 {TOKENS_FILE}。",
            "Imported {MODEL_FILE} and {TOKENS_FILE}."
        ),
    })
}

fn size_mismatch_message(path: &Path, spec: &ModelFile, actual: u64) -> String {
    if spec.name == MODEL_FILE && actual == 937_617_178 {
        return trf!(
            "{} 是 fp32 版的 model.onnx（约 937 MB），VoxKey 需要 int8 版 {}（{:.1} MB）。请换用 int8 文件。",
            "{} is the fp32 model.onnx (~937 MB); VoxKey needs the int8 {} ({:.1} MB). Please use the int8 file.",
            path.display(),
            MODEL_FILE,
            spec.size as f64 / 1_000_000.0
        );
    }
    trf!(
        "{} 大小不符：期望 {} 字节（{:.1} MB），实际 {} 字节（{:.1} MB），文件可能没有下载完整。",
        "{} size mismatch: expected {} bytes ({:.1} MB), got {} bytes ({:.1} MB); the file is probably incomplete.",
        path.display(),
        spec.size,
        spec.size as f64 / 1_000_000.0,
        actual,
        actual as f64 / 1_000_000.0
    )
}

fn copy_file(source: &Path, dest: &Path) -> Result<(), String> {
    let temp = dest.with_file_name(format!(
        "{}.importing",
        dest.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("model")
    ));
    fs::copy(source, &temp).map_err(|err| {
        trf!(
            "复制 {} 失败：{err}",
            "Failed to copy {}: {err}",
            source.display()
        )
    })?;
    if dest.exists() {
        fs::remove_file(dest).map_err(|err| {
            trf!(
                "无法替换旧的 {}：{err}",
                "Cannot replace the old {}: {err}",
                dest.display()
            )
        })?;
    }
    fs::rename(&temp, dest).map_err(|err| {
        trf!(
            "无法安装 {}：{err}",
            "Cannot install {}: {err}",
            dest.display()
        )
    })
}

fn collect_candidates(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 3 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, depth + 1, out);
            } else if is_candidate(&path) {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, 0, &mut out);
    out
}

fn contains_any_file(dir: &Path) -> bool {
    fn walk(dir: &Path, depth: usize) -> bool {
        if depth > 3 {
            return false;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, depth + 1)
            } else {
                true
            }
        })
    }
    walk(dir, 0)
}

fn is_candidate(path: &Path) -> bool {
    let name = file_name_lower(path);
    name.ends_with(".onnx") || name.ends_with("tokens.txt")
}

fn matches_spec(path: &Path, spec: &ModelFile) -> bool {
    let name = file_name_lower(path);
    if spec.name == TOKENS_FILE {
        name.ends_with("tokens.txt")
    } else {
        name.ends_with(".onnx") && name.contains("model")
    }
}

fn file_name_lower(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("voxkey-model-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    #[ignore = "reads the 239 MB model from the local cache"]
    fn cached_model_matches_pinned_sha256() {
        let (model, tokens) = model_paths();
        assert!(model.is_file(), "cache missing at {}", model.display());
        assert!(
            sha256::file_matches(&model, FILES[0].sha256).expect("hash model"),
            "model.int8.onnx does not match the pinned SHA-256"
        );
        assert!(
            sha256::file_matches(&tokens, FILES[1].sha256).expect("hash tokens"),
            "tokens.txt does not match the pinned SHA-256"
        );
    }

    #[test]
    #[ignore = "copies the 239 MB cached model into a temporary folder"]
    fn import_copies_verified_cache() {
        let (model, tokens) = model_paths();
        if !model.is_file() || !tokens.is_file() {
            return;
        }
        let source = temp_dir("real-src");
        let dest = temp_dir("real-dest");
        fs::copy(&model, source.join(MODEL_FILE)).unwrap();
        fs::copy(&tokens, source.join(TOKENS_FILE)).unwrap();

        let mut progress = |_: u64, _: u64, _: &str| {};
        let candidates = collect_candidates(&source);
        match import_candidates(&candidates, &dest, &source, &mut progress) {
            Ok(ImportOutcome::Imported { message }) => {
                assert!(message.contains(MODEL_FILE), "{message}");
            }
            Err(err) => panic!("import failed: {err}"),
            Ok(ImportOutcome::Empty { .. }) => panic!("unexpected empty source"),
        }
        assert!(valid_file(&dest.join(MODEL_FILE), MODEL_SIZE));
        assert!(valid_file(&dest.join(TOKENS_FILE), TOKENS_SIZE));

        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(dest);
    }

    #[test]
    #[ignore = "requires unrestricted network access to huggingface.co"]
    fn downloads_tokens_from_upstream() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let dir = temp_dir("net-dest");
        let spec = FILES[1];
        let part = dir.join(format!("{}{PART_SUFFIX}", spec.name));
        let dest = dir.join(spec.name);
        let client = download_client().expect("client");
        let url = format!("{}/{REPO}/resolve/{REVISION}/{}", MIRRORS[0], spec.name);
        let mut done = 0u64;
        let total = total_bytes();
        let mut progress = |_: u64, _: u64, _: &str| {};

        runtime
            .block_on(try_once(
                &client,
                &url,
                &spec,
                &part,
                &dest,
                0,
                total,
                &mut done,
                &mut progress,
                "下载 tokens",
            ))
            .expect("download tokens.txt");

        assert!(valid_file(&dest, TOKENS_SIZE));
        assert!(sha256::file_matches(&dest, spec.sha256).expect("hash"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn parses_content_range_headers() {
        assert_eq!(parse_content_range_start("bytes 100-199/1234"), Some(100));
        assert_eq!(parse_content_range_start("bytes=0-315893/315894"), Some(0));
        assert_eq!(
            parse_content_range_start("bytes 239233841-239233840/239233841"),
            Some(239_233_841)
        );
        assert_eq!(parse_content_range_start("garbage"), None);
        assert_eq!(parse_content_range_start(""), None);
    }

    #[test]
    fn cache_path_has_expected_files() {
        let (model, tokens) = model_paths();
        assert_eq!(model.file_name().unwrap(), MODEL_FILE);
        assert_eq!(tokens.file_name().unwrap(), TOKENS_FILE);
    }

    #[test]
    fn pinned_metadata_matches_published_sizes() {
        assert_eq!(FILES[0].name, "model.int8.onnx");
        assert_eq!(FILES[0].size, 239_233_841);
        assert_eq!(FILES[0].sha256.len(), 64);
        assert_eq!(FILES[1].name, "tokens.txt");
        assert_eq!(FILES[1].size, 315_894);
        assert_eq!(FILES[1].sha256.len(), 64);
        assert_eq!(total_bytes(), 239_549_735);
    }

    #[test]
    fn download_links_point_at_the_pinned_revision() {
        let links = download_links();
        assert_eq!(links.lines().count(), 4);
        for line in links.lines() {
            assert!(line.contains(REVISION), "{line}");
            assert!(line.contains("huggingface.co") || line.contains("hf-mirror.com"));
        }
    }

    #[test]
    fn candidate_scan_ignores_unrelated_files() {
        let dir = temp_dir("candidates");
        fs::write(dir.join("model.int8.onnx"), b"x").unwrap();
        fs::write(dir.join("tokens.txt"), b"x").unwrap();
        fs::write(dir.join("readme.txt"), b"x").unwrap();
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("nested").join("MODEL.INT8.ONNX"), b"x").unwrap();

        let candidates = collect_candidates(&dir);
        assert_eq!(candidates.len(), 3, "{candidates:?}");
        assert!(candidates.iter().any(|path| matches_spec(path, &FILES[0])));
        assert!(candidates.iter().any(|path| matches_spec(path, &FILES[1])));
        assert!(!candidates.iter().any(|path| path.ends_with("readme.txt")));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn import_rejects_truncated_files() {
        let source = temp_dir("truncated-src");
        let dest = temp_dir("truncated-dest");
        fs::write(source.join(MODEL_FILE), b"partial").unwrap();
        fs::write(source.join(TOKENS_FILE), b"partial").unwrap();

        let mut progress = |_: u64, _: u64, _: &str| {};
        let candidates = collect_candidates(&source);
        let error = import_candidates(&candidates, &dest, &source, &mut progress)
            .expect_err("truncated files must be rejected");
        assert!(error.contains("大小不符"), "{error}");
        assert!(error.contains(MODEL_FILE), "{error}");

        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(dest);
    }

    #[test]
    fn import_reports_missing_file_with_path_and_size() {
        let source = temp_dir("missing-src");
        let dest = temp_dir("missing-dest");
        // Only tokens.txt is present, so the first expectation
        // (model.int8.onnx) is the one reported as missing.
        fs::write(source.join(TOKENS_FILE), b"partial").unwrap();

        let mut progress = |_: u64, _: u64, _: &str| {};
        let candidates = collect_candidates(&source);
        let error = import_candidates(&candidates, &dest, &source, &mut progress)
            .expect_err("missing model.int8.onnx must be reported");
        assert!(error.contains("缺少"), "{error}");
        assert!(error.contains(MODEL_FILE), "{error}");
        assert!(error.contains("239233841"), "{error}");

        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(dest);
    }
}
