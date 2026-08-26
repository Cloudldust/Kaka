//! Import session journal for interrupted-import resume (PRD 6.7.1).
//!
//! Sessions live in %APPDATA%/Kaka/import_session_{id}.json. A session that is
//! neither `completed` nor `abandoned` when the app starts is an unfinished
//! import that may be resumed.

use crate::paths;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSession {
    pub session_id: String,
    /// "copy" (add mode never creates sessions).
    pub mode: String,
    pub source: String,
    pub target: String,
    /// "structure" / "date" / "flat"
    pub org_mode: String,
    pub recursive: bool,
    pub dedup: bool,
    pub created_at: String,
    pub completed: bool,
    pub abandoned: bool,
    pub total: usize,
    pub done: usize,
}

impl ImportSession {
    /// Reconstruct a copy-mode [`CopyOptions`] from the session for resume.
    pub fn copy_options(&self) -> crate::app::copy::CopyOptions {
        crate::app::copy::CopyOptions {
            target_dir: self.target.clone(),
            org_mode: crate::app::copy::OrgMode::from_code(&self.org_mode),
            recursive: self.recursive,
            dedup: self.dedup,
        }
    }
}

fn journal_dir() -> PathBuf {
    paths::app_data_dir()
}

fn journal_path(session_id: &str) -> PathBuf {
    journal_dir().join(format!("import_session_{session_id}.json"))
}

fn new_session_id() -> String {
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{ts}_{nanos:x}")
}

/// Create and persist a new session.
pub fn start(
    source: &str,
    target: &str,
    org_mode: &str,
    recursive: bool,
    dedup: bool,
    total: usize,
) -> anyhow::Result<ImportSession> {
    let session = ImportSession {
        session_id: new_session_id(),
        mode: "copy".to_string(),
        source: source.to_string(),
        target: target.to_string(),
        org_mode: org_mode.to_string(),
        recursive,
        dedup,
        created_at: chrono::Local::now().to_rfc3339(),
        completed: false,
        abandoned: false,
        total,
        done: 0,
    };
    write(&session)?;
    Ok(session)
}

/// Persist a session to disk.
pub fn write(session: &ImportSession) -> anyhow::Result<()> {
    let path = journal_path(&session.session_id);
    std::fs::create_dir_all(journal_dir())?;
    let text = serde_json::to_string_pretty(session)?;
    std::fs::write(&path, text)?;
    Ok(())
}

/// Mark a session completed (normal finish: success/failed/cancelled).
pub fn complete(session: &mut ImportSession) -> anyhow::Result<()> {
    session.completed = true;
    write(session)
}

/// Mark a session abandoned and move it to the abandoned dir (kept 7 days).
pub fn abandon(session: &mut ImportSession) -> anyhow::Result<()> {
    session.abandoned = true;
    write(session)?;
    let from = journal_path(&session.session_id);
    std::fs::create_dir_all(paths::abandoned_dir())?;
    let to = paths::abandoned_dir().join(format!(
        "import_session_{}.json",
        session.session_id
    ));
    let _ = std::fs::rename(&from, &to);
    Ok(())
}

/// Load a single session by id.
pub fn load(session_id: &str) -> anyhow::Result<Option<ImportSession>> {
    let path = journal_path(session_id);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    Ok(Some(serde_json::from_str(&text)?))
}

/// List sessions that are still in progress (not completed, not abandoned).
pub fn list_incomplete() -> Vec<ImportSession> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(journal_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("import_session_") && name.ends_with(".json") {
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                if let Ok(s) = serde_json::from_str::<ImportSession>(&text) {
                    if !s.completed && !s.abandoned {
                        out.push(s);
                    }
                }
            }
        }
    }
    out
}
