//! User settings persistence (%APPDATA%/Kaka/config.toml).

use crate::model::AppConfig;
use crate::paths;

/// Load settings from disk, falling back to defaults on any error.
pub fn load() -> AppConfig {
    let path = paths::config_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
}

/// Persist settings to disk. Creates the parent dir if needed.
pub fn save(cfg: &AppConfig) -> anyhow::Result<()> {
    let path = paths::config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(cfg)?;
    std::fs::write(&path, text)?;
    Ok(())
}
