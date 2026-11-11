//! In-app update from GitHub Releases.
//!
//! VoxKey ships as a single `VoxKey.exe`. The update path is therefore
//! deliberately small and self-contained:
//!
//! 1. `check_blocking` asks the GitHub Releases API for the latest release and
//!    compares its tag with the version compiled into the running binary.
//! 2. `download_blocking` downloads the release asset into
//!    `%APPDATA%\VoxKey\updates\`, throttles progress callbacks, then verifies
//!    the file: exact size, PE header for x86-64, and a `--version` self-check
//!    of the downloaded executable against the release tag.
//! 3. `install_and_restart` stages the new binary next to the current one,
//!    renames the running image (Windows allows renaming a running executable),
//!    swaps in the new file, launches it, and lets the caller quit the old
//!    process.
//!
//! Everything here runs on a background thread with its own current-thread
//! Tokio runtime, mirroring `llm.rs` / `model.rs`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::config;

const API_LATEST: &str = "https://api.github.com/repos/katriyda/VoxKey/releases/latest";
const RELEASES_PAGE: &str = "https://github.com/katriyda/VoxKey/releases";
const USER_AGENT: &str = concat!("VoxKey/", env!("CARGO_PKG_VERSION"));
/// Release assets are the ~40 MB single-file executable. Anything smaller is
/// not a plausible VoxKey build and is rejected before it is written to disk.
const MIN_BINARY_SIZE: u64 = 1_000_000;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn releases_page() -> &'static str {
    RELEASES_PAGE
}

#[derive(Clone, Debug)]
pub struct ReleaseInfo {
    /// Normalized version, e.g. `0.1.1` (tag `v0.1.1`).
    pub version: String,
    pub notes: String,
    pub page_url: String,
    pub asset_url: String,
    pub asset_size: u64,
    /// GitHub-provided `sha256:<hex>` digest of the asset, when available.
    pub asset_digest: Option<String>,
}

/// UI-side update state. Owned by the app, rendered by the settings window.
#[derive(Default)]
pub struct UpdateUiState {
    pub checking: bool,
    pub downloading: bool,
    pub info: Option<ReleaseInfo>,
    pub progress: Option<(u64, u64)>,
    pub message: Option<(bool, String)>,
}

#[derive(serde::Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(serde::Deserialize)]
struct ApiAsset {
    name: String,
    size: u64,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

fn http_runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("无法创建网络运行时: {e}"))
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("创建更新客户端失败: {e}"))
}

/// Asks GitHub for the newest release. `Ok(None)` means the running build is
/// already current (or the remote tag cannot be parsed as a version).
pub fn check_blocking() -> Result<Option<ReleaseInfo>, String> {
    let runtime = http_runtime()?;
    runtime.block_on(check_async())
}

async fn check_async() -> Result<Option<ReleaseInfo>, String> {
    let client = http_client()?;
    let response = client
        .get(API_LATEST)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("无法连接 GitHub：{e}"))?;

    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("仓库还没有发布任何 Release。{RELEASES_PAGE}"));
    }
    if !status.is_success() {
        let hint = if status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        {
            "（GitHub API 请求频率受限，请稍后再试）"
        } else {
            ""
        };
        return Err(format!("GitHub API 返回 HTTP {status}{hint}"));
    }

    let release: ApiRelease = response
        .json()
        .await
        .map_err(|e| format!("无法解析 Release 信息：{e}"))?;

    let Some(version) = normalize_version(&release.tag_name) else {
        return Err(format!("无法识别 Release 版本号：{}", release.tag_name));
    };
    if !is_newer(&version, current_version()) {
        return Ok(None);
    }

    let asset = select_asset(&release.assets)
        .ok_or_else(|| "最新 Release 中没有 VoxKey.exe 资产".to_string())?;
    if asset.size < MIN_BINARY_SIZE {
        return Err(format!(
            "发布资产大小异常（{} 字节），已中止更新",
            asset.size
        ));
    }

    Ok(Some(ReleaseInfo {
        version,
        notes: release.body.unwrap_or_default(),
        page_url: release.html_url,
        asset_url: asset.browser_download_url.clone(),
        asset_size: asset.size,
        asset_digest: asset.digest.clone(),
    }))
}

fn select_asset(assets: &[ApiAsset]) -> Option<&ApiAsset> {
    assets
        .iter()
        .find(|asset| asset.name.eq_ignore_ascii_case("VoxKey.exe"))
        .or_else(|| {
            assets.iter().find(|asset| {
                let lower = asset.name.to_ascii_lowercase();
                lower.ends_with(".exe") && lower.contains("voxkey")
            })
        })
}

/// Downloads and verifies the release asset. Returns the path of the verified
/// executable inside `%APPDATA%\VoxKey\updates\`.
pub fn download_blocking(
    info: &ReleaseInfo,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, String> {
    let runtime = http_runtime()?;
    runtime.block_on(download_async(info, progress))
}

async fn download_async(
    info: &ReleaseInfo,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, String> {
    use tokio::io::AsyncWriteExt;

    let dir = updates_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("无法创建更新目录 {}：{e}", dir.display()))?;
    let target = dir.join(format!("VoxKey-{}.exe", info.version));
    let part = dir.join(format!("VoxKey-{}.exe.part", info.version));
    let _ = tokio::fs::remove_file(&part).await;

    let client = http_client()?;
    let mut response = client
        .get(&info.asset_url)
        .send()
        .await
        .map_err(|e| format!("下载失败：{e}"))?;
    if !response.status().is_success() {
        return Err(format!("下载失败：HTTP {}", response.status()));
    }
    let reported = response.content_length().unwrap_or(info.asset_size);
    if reported != info.asset_size {
        return Err(format!(
            "远端文件大小异常（期望 {}，实际 {reported}）",
            info.asset_size
        ));
    }

    let mut file = tokio::fs::File::create(&part)
        .await
        .map_err(|e| format!("无法创建临时文件：{e}"))?;
    let total = info.asset_size;
    let mut downloaded = 0u64;
    let mut last_report = Instant::now();
    progress(0, total);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("下载中断：{e}"))?
    {
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("写入更新文件失败：{e}"))?;
        downloaded += chunk.len() as u64;
        if last_report.elapsed() >= PROGRESS_INTERVAL {
            progress(downloaded, total);
            last_report = Instant::now();
        }
    }
    file.flush()
        .await
        .map_err(|e| format!("写入更新文件失败：{e}"))?;
    drop(file);

    if downloaded != total {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(format!(
            "下载不完整（期望 {total} 字节，实际 {downloaded} 字节）"
        ));
    }
    progress(total, total);

    tokio::fs::rename(&part, &target)
        .await
        .map_err(|e| format!("无法保存更新文件：{e}"))?;

    if let Err(err) = verify_binary(&target, info) {
        let _ = std::fs::remove_file(&target);
        return Err(err);
    }
    Ok(target)
}

/// Verifies a downloaded build before it is allowed to replace the running
/// executable: size, PE identity, and its own `--version` self-check.
pub fn verify_binary(path: &Path, info: &ReleaseInfo) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("无法读取更新文件：{e}"))?;
    if meta.len() != info.asset_size {
        return Err(format!(
            "更新文件大小不一致（期望 {}，实际 {}）",
            info.asset_size,
            meta.len()
        ));
    }
    if meta.len() < MIN_BINARY_SIZE {
        return Err("更新文件过小，不是有效的 VoxKey 程序".to_string());
    }

    let mut head = [0u8; 1024];
    {
        use std::io::Read;
        let mut file = std::fs::File::open(path).map_err(|e| format!("无法读取更新文件：{e}"))?;
        let read = file
            .read(&mut head)
            .map_err(|e| format!("无法读取更新文件头：{e}"))?;
        if read < 0x40 {
            return Err("更新文件头不完整".to_string());
        }
    }
    match pe_machine(&head) {
        Some(0x8664) => {}
        Some(machine) => {
            return Err(format!(
                "更新文件不是 x86-64 程序（PE machine 0x{machine:04X}）"
            ));
        }
        None => return Err("更新文件不是有效的 Windows 可执行文件".to_string()),
    }

    if let Some(digest) = info
        .asset_digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .map(|hex| hex.trim().to_ascii_lowercase())
    {
        let actual = sha256_file(path)?;
        if actual != digest {
            return Err(format!(
                "更新文件 SHA-256 校验失败（期望 {digest}，实际 {actual}）"
            ));
        }
    }

    let output = std::process::Command::new(path)
        .arg("--version")
        .output()
        .map_err(|e| format!("无法运行新版本自检：{e}"))?;
    if !output.status.success() {
        return Err(format!(
            "新版本自检失败（退出码 {:?}）",
            output.status.code()
        ));
    }
    let reported_version = String::from_utf8_lossy(&output.stdout);
    let normalized = reported_version
        .split_whitespace()
        .nth(1)
        .and_then(normalize_version)
        .ok_or_else(|| {
            format!(
                "新版本自检输出异常：{}",
                reported_version.trim()
            )
        })?;
    if normalized != info.version {
        return Err(format!(
            "新版本自检版本号不一致（期望 {}，实际 {normalized}）",
            info.version
        ));
    }
    Ok(())
}

/// Replaces the running executable with `new_exe` and launches the new build.
///
/// Windows lets a running image be renamed, so no helper process or batch
/// script is needed. On any failure the original file is restored.
pub fn install_and_restart(new_exe: &Path) -> Result<(), String> {
    let current = std::env::current_exe().map_err(|e| format!("无法定位当前程序：{e}"))?;
    let current = std::fs::canonicalize(&current).unwrap_or(current);
    let parent = current
        .parent()
        .ok_or_else(|| "无法定位程序所在目录".to_string())?;

    let staged = current.with_extension("exe.new");
    let backup = current.with_extension("exe.old");
    let _ = std::fs::remove_file(&staged);
    std::fs::copy(new_exe, &staged).map_err(|e| {
        format!(
            "无法写入新版本到 {}（可能没有写权限）：{e}",
            parent.display()
        )
    })?;

    // A leftover backup from a previous update may still be locked by the old
    // process for a moment; replacing it is best effort.
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(&current, &backup).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("无法替换正在运行的程序：{e}")
    })?;

    if let Err(err) = std::fs::rename(&staged, &current) {
        let _ = std::fs::rename(&backup, &current);
        let _ = std::fs::remove_file(&staged);
        return Err(format!("无法启用新版本：{err}"));
    }

    match std::process::Command::new(&current).spawn() {
        Ok(_) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&current);
            let _ = std::fs::rename(&backup, &current);
            Err(format!("新版本启动失败，已回滚到原版本：{err}"))
        }
    }
}

/// Best-effort removal of leftovers from a previous update. Called once at
/// startup; the old process may still hold `VoxKey.exe.old` briefly.
pub fn cleanup_stale_files() {
    let Ok(current) = std::env::current_exe() else {
        return;
    };
    let current = std::fs::canonicalize(&current).unwrap_or(current);
    for extension in ["exe.old", "exe.new"] {
        let path = current.with_extension(extension);
        if path != current {
            let _ = std::fs::remove_file(path);
        }
    }

    if let Ok(entries) = std::fs::read_dir(updates_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "part") {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

pub fn updates_dir() -> PathBuf {
    config::data_dir().join("updates")
}

/// `v0.1.2` / `0.1.2` / `0.1.2-rc1` -> `0.1.2`. Returns `None` for anything
/// that does not start with three numeric components.
pub fn normalize_version(text: &str) -> Option<String> {
    let key = version_key(text)?;
    Some(format!("{}.{}.{}", key[0], key[1], key[2]))
}

fn version_key(text: &str) -> Option<[u64; 3]> {
    let trimmed = text.trim();
    let trimmed = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    let core = trimmed.split(['-', '+', ' ']).next().unwrap_or("");
    let mut key = [0u64; 3];
    let mut parts = core.split('.');
    for slot in key.iter_mut() {
        let piece = parts.next()?;
        let digits = piece
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>();
        if digits.is_empty() {
            return None;
        }
        *slot = digits.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(key)
}

pub fn is_newer(remote: &str, current: &str) -> bool {
    match (version_key(remote), version_key(current)) {
        (Some(remote), Some(current)) => remote > current,
        _ => false,
    }
}

fn pe_machine(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 0x40 || &bytes[0..2] != b"MZ" {
        return None;
    }
    let offset = u32::from_le_bytes(bytes.get(0x3C..0x40)?.try_into().ok()?) as usize;
    let header = bytes.get(offset..offset + 6)?;
    if &header[0..4] != b"PE\0\0" {
        return None;
    }
    Some(u16::from_le_bytes([header[4], header[5]]))
}

/// SHA-256 of a file as lowercase hex, computed with Windows CNG (`bcrypt.dll`).
/// The project already links the `Win32_Security_Cryptography` feature for DPAPI,
/// so this needs no extra crate.
fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("无法读取更新文件以便校验：{e}"))?;
    sha256_hex(&bytes)
}

fn sha256_hex(data: &[u8]) -> Result<String, String> {
    use windows::Win32::Security::Cryptography::{
        BCryptCloseAlgorithmProvider, BCryptHash, BCryptOpenAlgorithmProvider, BCRYPT_ALG_HANDLE,
        BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS, BCRYPT_SHA256_ALGORITHM,
    };
    use windows::core::PCWSTR;

    let mut algorithm = BCRYPT_ALG_HANDLE(std::ptr::null_mut());
    let opened = unsafe {
        BCryptOpenAlgorithmProvider(
            &mut algorithm,
            BCRYPT_SHA256_ALGORITHM,
            PCWSTR::null(),
            BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
        )
    };
    if opened.0 != 0 {
        return Err(format!("无法初始化 SHA-256（NTSTATUS 0x{:08X}）", opened.0));
    }

    let mut digest = [0u8; 32];
    let hashed = unsafe { BCryptHash(algorithm, None, data, &mut digest) };
    let _ = unsafe { BCryptCloseAlgorithmProvider(algorithm, 0) };
    if hashed.0 != 0 {
        return Err(format!("计算 SHA-256 失败（NTSTATUS 0x{:08X}）", hashed.0));
    }
    Ok(hex_lower(&digest))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0F), 16).unwrap_or('0'));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn normalizes_release_tags() {
        assert_eq!(normalize_version("v0.1.2").as_deref(), Some("0.1.2"));
        assert_eq!(normalize_version("0.1.2").as_deref(), Some("0.1.2"));
        assert_eq!(normalize_version("v0.1.2-rc1").as_deref(), Some("0.1.2"));
        assert_eq!(normalize_version("0.1").as_deref(), None);
        assert_eq!(normalize_version("release"), None);
    }

    #[test]
    fn hashes_known_vectors() {
        assert_eq!(
            sha256_hex(b"").unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn reads_pe_machine() {
        let mut bytes = vec![0u8; 1024];
        bytes[0..2].copy_from_slice(b"MZ");
        bytes[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        bytes[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
        assert_eq!(pe_machine(&bytes), Some(0x8664));

        let mut bad = bytes.clone();
        bad[0x86..0x88].copy_from_slice(&0x014cu16.to_le_bytes());
        bad[0x84..0x86].copy_from_slice(&0x014cu16.to_le_bytes());
        assert_eq!(pe_machine(&bad), Some(0x014c));

        assert_eq!(pe_machine(b"not an exe"), None);
    }
}
