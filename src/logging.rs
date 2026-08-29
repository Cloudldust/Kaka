//! Minimal file logging (PRD 14, deliberately trimmed).
//!
//! The whole codebase already reports through the `log` facade; this module
//! only adds the missing sink: one plain-text file per day
//! (`kaka_YYYYMMDD.log` under %APPDATA%/Kaka/logs), old files pruned after
//! 14 days, plus a panic hook that records the crash location/message before
//! the `panic = "abort"` release profile kills the process. Without this,
//! release builds lose every log line (stderr of a double-clicked GUI goes
//! nowhere), making user-reported import/db/recycle failures undiagnosable.
//!
//! Deliberately NOT implemented from PRD 14 (over-engineering for a local
//! tool): JSON Lines, a level-switch setting, 50MB rotation, a "clear all
//! logs" button, and minidump files.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::paths;

/// Log files older than this many days are deleted at startup (PRD 14).
const RETENTION_DAYS: i64 = 14;

/// Serializes log-file writes (UI thread + background workers share the file).
static WRITE_LOCK: Mutex<()> = Mutex::new(());

struct FileLogger {
    max_level: log::LevelFilter,
    dir: PathBuf,
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.filter(metadata.level(), metadata.target())
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} [{:<5}] [{}] {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            record.level(),
            record.target(),
            record.args()
        );
        // File first (best-effort), then stderr so `cargo run` keeps a console.
        let _guard = WRITE_LOCK.lock();
        append_line(&log_file_path(&self.dir), &line);
        drop(_guard);
        let _ = std::io::stderr().write_all(line.as_bytes());
    }

    fn flush(&self) {}
}

impl FileLogger {
    /// Own code logs at INFO+; third-party crates (wgpu adapter enumeration
    /// etc. are chatty at INFO) only at WARN+.
    fn filter(&self, level: log::Level, target: &str) -> bool {
        if level > self.max_level {
            return false;
        }
        if target.starts_with("kaka") {
            true
        } else {
            level <= log::Level::Warn
        }
    }
}

/// Today's log file — one per day, rotation is just the filename.
fn log_file_path(dir: &Path) -> PathBuf {
    dir.join(format!(
        "kaka_{}.log",
        chrono::Local::now().format("%Y%m%d")
    ))
}

/// Open-append-write-close: no long-held handle, and the line is handed to the
/// OS (survives process abort) before this returns. Missing parent dirs or
/// locked files are silently ignored — logging must never break the app.
fn append_line(path: &Path, line: &str) {
    if let Ok(mut f) = OpenOptions::new().append(true).create(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Install the panic hook (records panic message + location before
/// `panic = "abort"` tears the process down) and the file logger.
/// Called once at GUI startup, before anything else; never fails startup.
pub fn init() {
    let dir = paths::logs_dir();
    let _ = fs::create_dir_all(&dir);

    // Hook first so even a panic during the rest of startup is captured.
    install_panic_hook(&dir);

    // Startup housekeeping: drop log files past the retention window.
    prune(&dir, chrono::Local::now(), RETENTION_DAYS);

    // DEBUG only in dev builds (PRD 14's intent, decided at compile time).
    let max_level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    if log::set_boxed_logger(Box::new(FileLogger { max_level, dir })).is_ok() {
        log::set_max_level(max_level);
    }
}

fn install_panic_hook(dir: &Path) {
    let dir = dir.to_path_buf();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        let line = format!(
            "{} [PANIC] thread '{}' panicked at {}: {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            name,
            location,
            msg
        );
        // Write directly — NOT via log!() — so the hook cannot deadlock on the
        // logger's own mutex if the panic happened while it was held. No lock:
        // append-mode writes from separate handles are atomic enough for logs.
        append_line(&log_file_path(&dir), &line);
        let _ = std::io::stderr().write_all(line.as_bytes());
        // Keep the default behavior (message + backtrace) after ours.
        default_hook(info);
    }));
}

/// Delete `kaka_YYYYMMDD.log` files older than `days` relative to `now`.
fn prune(dir: &Path, now: chrono::DateTime<chrono::Local>, days: i64) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stamp) = name.strip_prefix("kaka_").and_then(|r| r.strip_suffix(".log")) else {
            continue;
        };
        if stamp.len() != 8 || !stamp.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(date) = chrono::NaiveDate::parse_from_str(stamp, "%Y%m%d") else {
            continue;
        };
        if (now.date_naive() - date).num_days() > days {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log as _;
    use std::fs::File;
    use std::io::Read;

    fn temp_dir(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let d = std::env::temp_dir().join(format!("kaka_log_test_{}_{}_{}", tag, std::process::id(), n));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn read(path: &Path) -> String {
        let mut s = String::new();
        File::open(path).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn logger_writes_level_and_message_to_daily_file() {
        let dir = temp_dir("write");
        let logger = FileLogger {
            max_level: log::LevelFilter::Info,
            dir: dir.clone(),
        };
        assert!(logger.enabled(&log::Metadata::builder().level(log::Level::Warn).build()));
        assert!(!logger.enabled(&log::Metadata::builder().level(log::Level::Debug).build()));
        // Third-party targets (wgpu adapter spam etc.) are limited to WARN+.
        assert!(!logger.enabled(&log::Metadata::builder().level(log::Level::Info).target("wgpu_hal").build()));
        assert!(logger.enabled(&log::Metadata::builder().level(log::Level::Warn).target("wgpu_hal").build()));

        for level in [log::Level::Error, log::Level::Info] {
            log::Log::log(
                &logger,
                &log::Record::builder()
                    .level(level)
                    .target("kaka::test")
                    .args(format_args!("boom-{}", level))
                    .build(),
            );
        }
        let text = read(&log_file_path(&dir));
        assert!(text.contains("[ERROR] [kaka::test] boom-ERROR"));
        assert!(text.contains("[INFO ] [kaka::test] boom-INFO"));
        // Filtered levels never reach the file.
        assert!(!text.contains("DEBUG"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prune_keeps_recent_and_removes_old_files() {
        let dir = temp_dir("prune");
        let now = chrono::Local::now();
        let make = |age_days: i64| {
            let date = (now - chrono::Duration::days(age_days))
                .format("%Y%m%d")
                .to_string();
            let p = dir.join(format!("kaka_{date}.log"));
            fs::write(&p, b"x").unwrap();
            p
        };
        let today = make(0);
        let old = make(10);
        let ancient = make(30);
        // Unparseable names must be ignored by prune.
        fs::write(dir.join("not_a_kaka_file.log"), b"x").unwrap();
        fs::write(dir.join("kaka_garbage.log"), b"x").unwrap();

        prune(&dir, now, RETENTION_DAYS);

        assert!(today.exists());
        assert!(old.exists(), "10 days <= 14-day retention must survive");
        assert!(!ancient.exists(), "30 days > 14-day retention must be removed");
        assert!(dir.join("not_a_kaka_file.log").exists());
        assert!(dir.join("kaka_garbage.log").exists());
        fs::remove_dir_all(&dir).ok();
    }
}
