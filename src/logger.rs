//! File-only logger for the Tellers CLI.
//!
//! This logger writes all log messages to a timestamped file in the system temp directory
//! (`{temp_dir}/tellers-cli/tellers-cli_{timestamp}.log`) without outputting to the terminal.
//!
//! Terminal output is handled separately by the `output` module to avoid interfering with
//! Ratatui's TUI components (e.g., progress bars). This separation ensures:
//! - Clean terminal display with colored, formatted output
//! - Complete raw logs preserved in files for debugging
//! - No conflicts between logger output and TUI rendering

use log::{Log, Metadata, Record};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static LOGGER: Logger = Logger {
    inner: Mutex::new(LoggerInner {
        log_file: None,
        log_path: None,
    }),
};

struct Logger {
    inner: Mutex<LoggerInner>,
}

struct LoggerInner {
    log_file: Option<File>,
    log_path: Option<PathBuf>,
}

impl Logger {
    fn init() -> Result<(), String> {
        let log_dir = std::env::temp_dir().join("tellers-cli");

        std::fs::create_dir_all(&log_dir)
            .map_err(|e| format!("Failed to create log directory: {}", e))?;

        let now = time::OffsetDateTime::now_utc();
        let timestamp = format!(
            "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
            now.year(),
            now.month() as u8,
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        );

        let log_path = log_dir.join(format!("tellers-cli_{}.log", timestamp));

        let log_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&log_path)
            .map_err(|e| format!("Failed to open log file: {}", e))?;

        let mut inner = LOGGER.inner.lock().unwrap();
        inner.log_file = Some(log_file);
        inner.log_path = Some(log_path.clone());

        log::set_logger(&LOGGER)
            .map(|()| log::set_max_level(log::LevelFilter::Trace))
            .map_err(|e| format!("Failed to set logger: {}", e))?;

        let initial_msg = format!("[INIT] Log file: {}\n", log_path.display());
        if let Some(ref mut file) = inner.log_file {
            let _ = file.write_all(initial_msg.as_bytes());
            let _ = file.flush();
        }

        drop(inner);

        log::info!("Logger initialized");

        Ok(())
    }

    fn get_log_path() -> Option<PathBuf> {
        LOGGER.inner.lock().unwrap().log_path.clone()
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::Level::Trace
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let now = time::OffsetDateTime::now_utc();
        let timestamp = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            now.year(),
            now.month() as u8,
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        );

        let log_line = format!("[{}] [{}] {}\n", timestamp, record.level(), record.args());

        let mut inner = self.inner.lock().unwrap();
        if let Some(ref mut file) = inner.log_file {
            let _ = file.write_all(log_line.as_bytes());
            let _ = file.flush();
        }
    }

    fn flush(&self) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(ref mut file) = inner.log_file {
            let _ = file.flush();
        }
    }
}

/// Initialize the file-only logger.
///
/// Creates a new log file in `{temp_dir}/tellers-cli/` with a timestamp in the filename.
/// All subsequent log messages (via `log::info!`, `log::warn!`, etc.) will be written
/// to this file without appearing in the terminal.
///
/// # Returns
///
/// Returns `Ok(())` on success or an error message if initialization fails.
pub fn init() -> Result<(), String> {
    Logger::init()
}

/// Get the path to the current log file.
///
/// # Returns
///
/// Returns `Some(PathBuf)` with the path to the log file if the logger is initialized,
/// or `None` if not yet initialized.
pub fn get_log_path() -> Option<PathBuf> {
    Logger::get_log_path()
}
