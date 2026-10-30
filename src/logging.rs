//! File logging. A windowless background process has nowhere to print, so
//! support needs a log file to diagnose "it doesn't print".
//!
//! Locations:
//!   Windows: `%LOCALAPPDATA%\Neoprint\logs\neoprint.<date>.log`
//!   macOS:   `~/Library/Logs/Neoprint/neoprint.<date>.log`
//!   other:   `<temp>/neoprint-logs/…`
//! Daily rotation, last 7 files kept.

use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};

const DEFAULT_FILTER: &str = "neoprint=info,tower_http=warn";
const KEEP_FILES: usize = 7;

pub fn log_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(local).join("Neoprint").join("logs");
    }
    #[cfg(target_os = "macos")]
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join("Library")
            .join("Logs")
            .join("Neoprint");
    }
    std::env::temp_dir().join("neoprint-logs")
}

/// Install the global tracing subscriber writing to the rolling log file.
/// Falls back to stderr if the log directory can't be created. Keep the
/// returned guard alive for the whole process: dropping it flushes + stops
/// the background writer.
pub fn init() -> Option<WorkerGuard> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| DEFAULT_FILTER.into());

    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("neoprint")
        .filename_suffix("log")
        .max_log_files(KEEP_FILES)
        .build(log_dir());

    match appender {
        Ok(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(writer)
                .with_ansi(false)
                .init();
            Some(guard)
        }
        Err(_) => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
            None
        }
    }
}
