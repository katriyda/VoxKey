//! First-run model download and local cache.
//!
//! VoxKey uses the multilingual SenseVoice int8 model from sherpa-onnx:
//! Chinese (Mandarin), English, Japanese, Korean and Cantonese.

use std::path::PathBuf;
use std::time::{Duration, Instant};

const REPO: &str = "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17";
const REVISION: &str = "2365baeacb507f821a0c8120fcee3d484dba7a07";
const MODEL_FILE: &str = "model.int8.onnx";
const TOKENS_FILE: &str = "tokens.txt";
const MODEL_SIZE: u64 = 239_233_841;
const TOKENS_SIZE: u64 = 315_894;

const MIRRORS: [&str; 2] = ["https://huggingface.co", "https://hf-mirror.com"];

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

pub fn model_paths() -> (PathBuf, PathBuf) {
    let dir = model_dir();
    (dir.join(MODEL_FILE), dir.join(TOKENS_FILE))
}

/// Downloads missing files into the cache. Returns the directory containing
/// `model.int8.onnx` and `tokens.txt`.
pub fn ensure_model(
    runtime: &tokio::runtime::Runtime,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<PathBuf, String> {
    let dir = model_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建模型缓存目录 {}: {e}", dir.display()))?;
    let model = dir.join(MODEL_FILE);
    let tokens = dir.join(TOKENS_FILE);

    let mut done = 0u64;
    let total = MODEL_SIZE + TOKENS_SIZE;

    // Fully cached startup must not flash a download overlay.
    if valid_file(&model, MODEL_SIZE) {
        done += MODEL_SIZE;
    }
    if valid_file(&tokens, TOKENS_SIZE) {
        done += TOKENS_SIZE;
    }

    runtime.block_on(async {
        if !valid_file(&model, MODEL_SIZE) {
            let _ = std::fs::remove_file(&model);
            download_with_fallbacks(
                MODEL_FILE,
                MODEL_SIZE,
                &model,
                total,
                &mut done,
                progress,
            )
            .await?;
        }
        if !valid_file(&tokens, TOKENS_SIZE) {
            let _ = std::fs::remove_file(&tokens);
            download_with_fallbacks(
                TOKENS_FILE,
                TOKENS_SIZE,
                &tokens,
                total,
                &mut done,
                progress,
            )
            .await?;
        }
        Ok::<(), String>(())
    })?;

    if !valid_file(&model, MODEL_SIZE) || !valid_file(&tokens, TOKENS_SIZE) {
        return Err("模型文件校验失败，请删除缓存后重试".to_string());
    }
    progress(total, total, "模型就绪");
    Ok(dir)
}

async fn download_with_fallbacks(
    filename: &str,
    expected_size: u64,
    destination: &PathBuf,
    total: u64,
    done: &mut u64,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .user_agent(concat!("VoxKey/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("创建下载客户端失败: {e}"))?;

    let mut errors = Vec::new();
    for base in MIRRORS {
        let url = format!("{base}/{REPO}/resolve/{REVISION}/{filename}");
        match download_one(
            &client,
            &url,
            expected_size,
            destination,
            total,
            done,
            progress,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(err) => errors.push(format!("{base}: {err}")),
        }
    }
    Err(format!(
        "无法下载 {filename}。已尝试所有镜像：{}",
        errors.join(" | ")
    ))
}

async fn download_one(
    client: &reqwest::Client,
    url: &str,
    expected_size: u64,
    destination: &PathBuf,
    total: u64,
    done: &mut u64,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;

    let part = destination.with_extension("part");
    let _ = tokio::fs::remove_file(&part).await;

    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    let reported_size = response.content_length().unwrap_or(expected_size);
    if reported_size != expected_size {
        return Err(format!(
            "远端文件大小异常（期望 {expected_size}，实际 {reported_size}）"
        ));
    }

    let mut file = tokio::fs::File::create(&part)
        .await
        .map_err(|e| format!("无法创建临时文件: {e}"))?;
    let mut received = 0u64;
    let mut last_report = Instant::now();
    let base_done = *done;

    progress(base_done, total, "下载语音模型");
    while let Some(chunk) = response.chunk().await.map_err(|e| format!("下载中断: {e}"))? {
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("写入模型失败: {e}"))?;
        received += chunk.len() as u64;
        *done = base_done + received;

        if last_report.elapsed() >= Duration::from_millis(350) {
            progress(*done, total, "下载语音模型");
            last_report = Instant::now();
        }
    }
    file.flush().await.map_err(|e| format!("写入模型失败: {e}"))?;
    drop(file);

    if received != expected_size {
        let _ = tokio::fs::remove_file(&part).await;
        *done = base_done;
        return Err(format!(
            "下载不完整（期望 {expected_size} 字节，实际 {received} 字节）"
        ));
    }

    tokio::fs::rename(&part, destination)
        .await
        .map_err(|e| format!("无法移动模型文件: {e}"))?;
    *done = base_done + expected_size;
    progress(*done, total, "下载语音模型");
    Ok(())
}

fn valid_file(path: &PathBuf, expected_size: u64) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.len() == expected_size)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_path_has_expected_files() {
        let (model, tokens) = model_paths();
        assert_eq!(model.file_name().unwrap(), MODEL_FILE);
        assert_eq!(tokens.file_name().unwrap(), TOKENS_FILE);
    }
}
