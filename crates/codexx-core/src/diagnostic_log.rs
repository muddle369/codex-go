use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Value, json};

static TEST_LOG_PATH: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
static LOG_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
struct DiagnosticRecord {
    timestamp_ms: u64,
    pid: u32,
    event: String,
    detail: Value,
}

pub fn append_diagnostic_log(event: &str, detail: impl Serialize) -> std::io::Result<()> {
    if matches!(
        event,
        "bridge.request" | "bridge.response" | "bridge.resolve_start" | "bridge.resolve_ok"
    ) {
        return Ok(());
    }
    let _write_guard = LOG_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = diagnostic_log_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    rotate_log_if_oversized(&path, MAX_LOG_BYTES)?;

    let detail = serde_json::to_value(detail).unwrap_or_else(|error| {
        json!({
            "serialization_error": error.to_string()
        })
    });
    let record = DiagnosticRecord {
        timestamp_ms: now_ms(),
        pid: std::process::id(),
        event: event.to_string(),
        detail,
    };
    let line = serde_json::to_string(&record).unwrap_or_else(|error| {
        json!({
            "timestamp_ms": now_ms(),
            "pid": std::process::id(),
            "event": "diagnostic_log.serialization_failed",
            "detail": {
                "message": error.to_string()
            }
        })
        .to_string()
    });

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

fn rotate_log_if_oversized(path: &std::path::Path, max_bytes: u64) -> std::io::Result<()> {
    if std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
        < max_bytes
    {
        return Ok(());
    }
    let rotated = path.with_extension("log.old");
    if rotated.exists() {
        std::fs::remove_file(&rotated)?;
    }
    std::fs::rename(path, rotated)
}

pub fn diagnostic_log_path() -> PathBuf {
    if let Some(lock) = TEST_LOG_PATH.get() {
        if let Ok(guard) = lock.lock() {
            if let Some(path) = &*guard {
                return path.clone();
            }
        }
    }
    crate::paths::default_diagnostic_log_path()
}

#[doc(hidden)]
pub fn set_diagnostic_log_path_for_tests(path: Option<PathBuf>) {
    let lock = TEST_LOG_PATH.get_or_init(|| Mutex::new(None));
    *lock.lock().expect("test log path lock poisoned") = path;
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_log_rotates_before_next_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("codexx.log");
        std::fs::write(&path, vec![b'x'; 1024]).unwrap();

        rotate_log_if_oversized(&path, 512).unwrap();

        assert!(!path.exists());
        assert_eq!(
            std::fs::metadata(path.with_extension("log.old"))
                .unwrap()
                .len(),
            1024
        );
    }
}
