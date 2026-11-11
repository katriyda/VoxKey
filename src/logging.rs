//! Best-effort diagnostics for release builds, which have no console.
//!
//! Every line is timestamped with seconds since the Unix epoch and written to
//! `%APPDATA%\VoxKey\voxkey.log`. Logging never fails the caller: if the file
//! cannot be opened the message is dropped.

use std::fs::OpenOptions;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn error(message: &str) {
    write("ERROR", message);
}

pub fn warn(message: &str) {
    write("WARN", message);
}

pub fn info(message: &str) {
    write("INFO", message);
}

fn write(level: &str, message: &str) {
    let path = crate::config::data_dir().join("voxkey.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("[logging] cannot open {}: {err}", path.display());
            return;
        }
    };
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let _ = writeln!(file, "[{seconds}] {level}: {message}");
}
