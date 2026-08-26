//! Workspace state persistence (PRD 10.4) and crash recovery (PRD 11).

use super::Db;
use crate::model::WorkspaceState;
use rusqlite::{params, Row};

fn map_state(r: &Row) -> rusqlite::Result<WorkspaceState> {
    Ok(WorkspaceState {
        current_folder_path: r.get("current_folder_path")?,
        current_index: r.get("current_index")?,
        current_sort: r.get("current_sort")?,
        filter_json: r.get("filter_json")?,
        last_selected_id: r.get("last_selected_id")?,
        last_save_time: r.get("last_save_time")?,
        last_crash_marker: r.get::<_, i64>("last_crash_marker")? != 0,
        recent_folders_json: r.get("recent_folders_json")?,
    })
}

/// Load the persisted workspace state, or None if the row is absent.
pub fn load(db: &Db) -> anyhow::Result<Option<WorkspaceState>> {
    let mut stmt = db
        .conn
        .prepare("SELECT * FROM workspace_state WHERE id = 1")?;
    let mut rows = stmt.query([])?;
    if let Some(row) = rows.next()? {
        Ok(Some(map_state(row)?))
    } else {
        Ok(None)
    }
}

/// Persist the workspace state. Creates the single row if needed.
pub fn save(db: &Db, state: &WorkspaceState) -> anyhow::Result<()> {
    db.conn.execute(
        "INSERT INTO workspace_state
         (id, current_folder_path, current_index, current_sort, filter_json,
          last_selected_id, last_save_time, last_crash_marker, recent_folders_json)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, datetime('now'), ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            current_folder_path = excluded.current_folder_path,
            current_index = excluded.current_index,
            current_sort = excluded.current_sort,
            filter_json = excluded.filter_json,
            last_selected_id = excluded.last_selected_id,
            last_save_time = datetime('now'),
            last_crash_marker = excluded.last_crash_marker,
            recent_folders_json = excluded.recent_folders_json",
        params![
            state.current_folder_path,
            state.current_index,
            state.current_sort,
            state.filter_json,
            state.last_selected_id,
            state.last_crash_marker as i64,
            state.recent_folders_json,
        ],
    )?;
    Ok(())
}

/// Mark the crash marker as 1 (called at startup before any guaranteed save).
pub fn mark_crash(db: &Db) -> anyhow::Result<()> {
    db.conn.execute(
        "INSERT INTO workspace_state (id, last_crash_marker, last_save_time)
         VALUES (1, 1, datetime('now'))
         ON CONFLICT(id) DO UPDATE SET last_crash_marker = 1, last_save_time = datetime('now')",
        [],
    )?;
    Ok(())
}

/// Clear the crash marker (called after a successful save / clean exit).
pub fn clear_crash(db: &Db) -> anyhow::Result<()> {
    db.conn.execute(
        "INSERT INTO workspace_state (id, last_crash_marker, last_save_time)
         VALUES (1, 0, datetime('now'))
         ON CONFLICT(id) DO UPDATE SET last_crash_marker = 0, last_save_time = datetime('now')",
        [],
    )?;
    Ok(())
}

/// Notification state about a previous crash, presented to the UI.
#[derive(Debug, Clone)]
pub struct CrashInfo {
    pub state: WorkspaceState,
}

/// Read the crash marker. True means the app was shut down uncleanly.
pub fn crash_marker(db: &Db) -> anyhow::Result<bool> {
    let v: Option<i64> = db
        .conn
        .query_row(
            "SELECT last_crash_marker FROM workspace_state WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .ok();
    Ok(v.unwrap_or(0) != 0)
}
