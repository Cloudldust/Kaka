//! Central path helpers for the app data, cache, logs and session files.

use std::path::PathBuf;

/// %APPDATA%/Kaka — app data directory.
pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::env::temp_dir())
        .join("Kaka")
}

/// The main SQLite database file: %APPDATA%/Kaka/kaka.db
pub fn db_path() -> PathBuf {
    app_data_dir().join("kaka.db")
}

/// %LOCALAPPDATA%/Kaka/cache — disk cache root.
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| std::env::temp_dir())
        .join("Kaka")
        .join("cache")
}

/// %LOCALAPPDATA%/Kaka/cache/thumbs — thumbnail cache.
pub fn thumbs_dir() -> PathBuf {
    cache_dir().join("thumbs")
}

/// %LOCALAPPDATA%/Kaka/cache/previews — preview image cache.
pub fn previews_dir() -> PathBuf {
    cache_dir().join("previews")
}

/// %LOCALAPPDATA%/Kaka/cache/cache_index.db — cache index db.
pub fn cache_index_path() -> PathBuf {
    cache_dir().join("cache_index.db")
}

/// %APPDATA%/Kaka/logs — log directory.
pub fn logs_dir() -> PathBuf {
    app_data_dir().join("logs")
}

/// %APPDATA%/Kaka/abandoned — abandoned import sessions (kept 7 days).
pub fn abandoned_dir() -> PathBuf {
    app_data_dir().join("abandoned")
}

/// %APPDATA%/Kaka/config.toml — persisted settings.
pub fn config_path() -> PathBuf {
    app_data_dir().join("config.toml")
}

/// Ensure the app data and cache directories exist.
pub fn ensure_dirs() -> std::io::Result<()> {
    for d in [
        app_data_dir(),
        cache_dir(),
        thumbs_dir(),
        previews_dir(),
        logs_dir(),
        abandoned_dir(),
    ] {
        std::fs::create_dir_all(&d)?;
    }
    Ok(())
}
