//! Folder metadata CRUD (PRD 10.3).

use super::Db;
use crate::model::Folder;
use rusqlite::{params, Row};

fn map_folder(r: &Row) -> rusqlite::Result<Folder> {
    Ok(Folder {
        id: r.get("id")?,
        folder_path: r.get("folder_path")?,
        display_name: r.get("display_name")?,
        notes: r.get("notes")?,
        first_import_time: r.get("first_import_time")?,
        last_open_time: r.get("last_open_time")?,
        recursive_show: r.get::<_, i64>("recursive_show")? != 0,
    })
}

/// Ensure a folder record exists, returning its id.
pub fn ensure_folder(db: &Db, path: &str) -> anyhow::Result<i64> {
    db.conn.execute(
        "INSERT INTO folders (folder_path, first_import_time)
         VALUES (?1, datetime('now'))
         ON CONFLICT(folder_path) DO UPDATE SET folder_path = excluded.folder_path",
        params![path],
    )?;
    let r = db
        .conn
        .query_row(
            "SELECT id FROM folders WHERE folder_path = ?1",
            params![path],
            |r| r.get(0),
        )?;
    Ok(r)
}

/// Mark a folder's last_open_time as now (used when switching workspaces).
pub fn touch_open(db: &Db, path: &str) -> anyhow::Result<()> {
    db.conn.execute(
        "UPDATE folders SET last_open_time = datetime('now') WHERE folder_path = ?1",
        params![path],
    )?;
    Ok(())
}

/// Fetch a folder record by path.
pub fn get_folder(db: &Db, path: &str) -> anyhow::Result<Option<Folder>> {
    let mut stmt = db.conn.prepare("SELECT * FROM folders WHERE folder_path = ?1")?;
    let mut rows = stmt.query(params![path])?;
    if let Some(row) = rows.next()? {
        Ok(Some(map_folder(row)?))
    } else {
        Ok(None)
    }
}

/// Recent folders, ordered by last_open_time descending (most recent first).
pub fn recent_folders(db: &Db) -> anyhow::Result<Vec<Folder>> {
    let mut stmt = db.conn.prepare(
        "SELECT * FROM folders WHERE last_open_time IS NOT NULL
         ORDER BY last_open_time DESC",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(map_folder(row)?);
    }
    Ok(out)
}

/// All folders (by most recently opened first).
pub fn all_folders(db: &Db) -> anyhow::Result<Vec<Folder>> {
    let mut stmt =
        db.conn
            .prepare("SELECT * FROM folders ORDER BY last_open_time DESC, id DESC")?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(map_folder(row)?);
    }
    Ok(out)
}
