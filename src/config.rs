//! User settings persistence (%APPDATA%/Kaka/config.toml).

use crate::model::{AppConfig, DEFAULT_REPO_URL};
use crate::paths;

/// 历史默认仓库地址：曾作为 `AppConfig::default().github_repo` 的占位值写入
/// 老版本生成的 config.toml。加载配置时检测到会迁移到 `DEFAULT_REPO_URL`，
/// 否则「打开 GitHub 仓库」按钮会一直指向旧地址。
const LEGACY_REPO_URL: &str = "https://github.com/kaka-rs/kaka";

/// Load settings from disk, falling back to defaults on any error.
pub fn load() -> AppConfig {
    let path = paths::config_path();
    let mut cfg = match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    };
    // 已存在配置中的历史占位仓库地址 → 当前官方地址（迁移后顺手持久化，
    // 避免下次启动重复迁移；失败仅告警，不影响本次运行）。
    if migrate_legacy_repo(&mut cfg) {
        if let Err(e) = save(&cfg) {
            log::warn!("迁移 github_repo 后保存配置失败（忽略）: {e}");
        }
    }
    cfg
}

/// 把配置里的历史占位仓库地址替换为当前官方地址。仅当值刚好是旧默认值时
/// 才替换（用户自定义的 fork 地址不受影响）。返回是否发生了替换。
pub fn migrate_legacy_repo(cfg: &mut AppConfig) -> bool {
    if cfg.github_repo.trim().eq_ignore_ascii_case(LEGACY_REPO_URL) {
        cfg.github_repo = DEFAULT_REPO_URL.to_string();
        true
    } else {
        false
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_repo_url_is_migrated_to_default() {
        let mut cfg = AppConfig {
            github_repo: "https://github.com/kaka-rs/kaka".into(),
            ..AppConfig::default()
        };
        assert!(migrate_legacy_repo(&mut cfg));
        assert_eq!(cfg.github_repo, DEFAULT_REPO_URL);
    }

    #[test]
    fn legacy_repo_url_migration_is_case_insensitive() {
        let mut cfg = AppConfig {
            github_repo: "HTTPS://GitHub.com/kaka-rs/kaka".into(),
            ..AppConfig::default()
        };
        assert!(migrate_legacy_repo(&mut cfg));
        assert_eq!(cfg.github_repo, DEFAULT_REPO_URL);
    }

    #[test]
    fn custom_fork_repo_is_untouched() {
        let mut cfg = AppConfig {
            github_repo: "https://github.com/someone/Kaka".into(),
            ..AppConfig::default()
        };
        assert!(!migrate_legacy_repo(&mut cfg));
        assert_eq!(cfg.github_repo, "https://github.com/someone/Kaka");
    }

    #[test]
    fn default_repo_is_untouched() {
        let mut cfg = AppConfig::default();
        assert!(!migrate_legacy_repo(&mut cfg));
        assert_eq!(cfg.github_repo, DEFAULT_REPO_URL);
    }
}