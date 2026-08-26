//! Schema creation, migration (PRD 10.5) and corruption repair (PRD 10.6).

use super::Db;
use anyhow::Context;
use rusqlite::Connection;

/// Returns the current schema version this build targets.
pub fn target_schema_version() -> i64 {
    super::SCHEMA_VERSION
}

/// Create all tables, indexes and seed the `meta` row if it does not exist.
pub fn init(db: &mut Db) -> anyhow::Result<()> {
    db.conn.execute_batch(SCHEMA_DDL)?;
    // Seed the single-row meta table on first init.
    db.conn.execute(
        "INSERT OR IGNORE INTO meta (id, schema_version, app_version, created_at)
         VALUES (1, ?1, ?2, datetime('now'))",
        rusqlite::params![target_schema_version(), env!("CARGO_PKG_VERSION")],
    )?;
    Ok(())
}

/// Run pending schema migrations. Called after `init` on every startup.
///
/// - meta == target: no-op.
/// - meta < target: apply migrations 1->2, 2->3 ... one at a time, in a
///   transaction, backing up before each. Any failure rolls back and we bail.
/// - meta > target: a newer version created the db; reject startup.
pub fn migrate(db: &mut Db) -> anyhow::Result<()> {
    let current = db.schema_version()?;
    let target = target_schema_version();

    if current == target {
        return Ok(());
    }
    if current > target {
        anyhow::bail!(
            "此数据库由更高版本的咔咔创建（版本 {}），请升级咔咔后再打开。",
            current
        );
    }

    for ver in (current + 1)..=target {
        // Back up before migrating (keep last 3 backups).
        backup_version(db, ver - 1)?;

        db.conn
            .execute("BEGIN IMMEDIATE", [])
            .context("begin migration transaction")?;
        let res = run_migration(&db.conn, ver);
        match res {
            Ok(()) => {
                db.conn
                    .execute(
                        "UPDATE meta SET schema_version = ?1, last_migrated_at = datetime('now') WHERE id = 1",
                        rusqlite::params![ver],
                    )?;
                db.conn
                    .execute("COMMIT", [])
                    .context("commit migration")?;
            }
            Err(e) => {
                // Rollback and bail out entirely.
                let _ = db.conn.execute("ROLLBACK", []);
                anyhow::bail!(
                    "数据库迁移到版本 {} 失败，已回滚：{e}。请联系开发者或手动恢复备份。",
                    ver
                );
            }
        }
    }
    Ok(())
}

/// Apply the migration that bumps the schema from `ver-1` to `ver`.
/// Version 1 is the initial schema; there are no earlier migrations yet.
fn run_migration(_conn: &Connection, ver: i64) -> anyhow::Result<()> {
    match ver {
        // Example future migration:
        // 2 => { conn.execute_batch("ALTER TABLE photos ADD COLUMN foo TEXT;")?; }
        _ => Ok(()),
    }
}

/// Copy the current db file to `<db>.v<ver>.bak`, pruning to keep 3 backups.
fn backup_version(db: &Db, ver: i64) -> anyhow::Result<()> {
    if db.path == std::path::PathBuf::from(":memory:") {
        return Ok(());
    }
    // Flush WAL first so the backup file is complete.
    let _ = db.checkpoint();

    let backup = db
        .path
        .with_file_name(format!("kaka.db.v{ver}.bak"));
    std::fs::copy(&db.path, &backup)
        .with_context(|| format!("备份数据库到 {} 失败", backup.display()))?;

    prune_backups(&db.path, 3)?;
    Ok(())
}

/// Keep only the newest `keep` backups matching `kaka.db.v*.bak`.
fn prune_backups(db_path: &std::path::Path, keep: usize) -> anyhow::Result<()> {
    let dir = db_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let prefix = db_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("kaka.db");
    let mut backups: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&format!("{prefix}.v")) && name.ends_with(".bak") {
            let meta = entry.metadata()?;
            if let Ok(t) = meta.modified() {
                backups.push((t, entry.path()));
            }
        }
    }
    backups.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    for (_, p) in backups.into_iter().skip(keep) {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

/// Corruption repair flow (PRD 10.6).
///
/// Given a database that failed `PRAGMA integrity_check`, try to restore an
/// intact backup. Returns Ok(db) on success, or the new empty database when
/// the user chose to create a fresh one / no backup is usable.
pub fn repair_or_reset(db: &mut Db) -> anyhow::Result<()> {
    // Find the most recent backup that passes integrity check.
    if let Some(bak) = find_good_backup(&db.path)? {
        // Close current connection by replacing it.
        let new_path = db.path.clone();
        db.conn = Connection::open(&new_path)?;
        std::fs::copy(&bak, &new_path)
            .with_context(|| format!("用备份 {} 恢复数据库失败", bak.display()))?;
        // Re-open after copy to avoid stale handles.
        db.conn = Connection::open(&new_path)?;
        db.conn.pragma_update(None, "journal_mode", "WAL")?;
        log::info!("数据库从备份恢复: {}", bak.display());
        return Ok(());
    }

    // No usable backup: rename the corrupt file and build a fresh one.
    reset_to_fresh(db)
}

fn find_good_backup(db_path: &std::path::Path) -> anyhow::Result<Option<std::path::PathBuf>> {
    let dir = db_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let prefix = db_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("kaka.db");
    let mut backups: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&format!("{prefix}.v")) && name.ends_with(".bak") {
            let meta = entry.metadata()?;
            if let Ok(t) = meta.modified() {
                backups.push((t, entry.path()));
            }
        }
    }
    backups.sort_by(|a, b| b.0.cmp(&a.0)); // try newest first
    for (_, bak) in backups {
        if let Ok(c) = Connection::open(&bak) {
            let ok = (|| -> anyhow::Result<bool> {
                let mut stmt = c.prepare("PRAGMA integrity_check")?;
                let mut rows = stmt.query([])?;
                let mut s = String::new();
                while let Some(row) = rows.next()? {
                    let r: String = row.get(0)?;
                    s.push_str(&r);
                }
                Ok(s.trim() == "ok")
            })();
            if let Ok(true) = ok {
                return Ok(Some(bak));
            }
        }
    }
    Ok(None)
}

/// Rename the corrupt db and create a fresh, empty one.
fn reset_to_fresh(db: &mut Db) -> anyhow::Result<()> {
    let path = db.path.clone();
    if path != std::path::PathBuf::from(":memory:") {
        let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let corrupted = path.with_file_name(format!("kaka.db.corrupted_{stamp}"));
        let _ = std::fs::rename(&path, &corrupted);
        log::warn!("数据库损坏，已重命名为 {}", corrupted.display());
    }
    db.conn = Connection::open(&path)?;
    db.conn.pragma_update(None, "journal_mode", "WAL")?;
    super::schema::init(db)?;
    log::info!("已新建空数据库: {}", path.display());
    Ok(())
}

const SCHEMA_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    schema_version  INTEGER NOT NULL DEFAULT 1,
    app_version     TEXT,
    created_at      TEXT DEFAULT (datetime('now')),
    last_migrated_at TEXT
);

CREATE TABLE IF NOT EXISTS photos (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    original_filename     TEXT NOT NULL,
    file_size             INTEGER NOT NULL,
    capture_time          TEXT NOT NULL,
    current_path          TEXT NOT NULL,
    folder_path           TEXT NOT NULL,
    status                INTEGER DEFAULT 0,
    thumb_hash            TEXT,
    decode_failed         INTEGER DEFAULT 0,
    preview_only          INTEGER DEFAULT 0,
    rotation_override     INTEGER DEFAULT 0,
    exif_orientation      INTEGER DEFAULT 1,
    pair_group_id         INTEGER,
    iso                   INTEGER,
    aperture              TEXT,
    shutter_speed         TEXT,
    focal_length          INTEGER,
    camera_model          TEXT,
    lens_model            TEXT,
    capture_time_source   TEXT DEFAULT 'exif_original',
    import_time           TEXT DEFAULT (datetime('now')),
    last_access_time      TEXT DEFAULT (datetime('now')),
    marked_delete_time    TEXT,
    marked_review_time    TEXT,
    CONSTRAINT unique_photo UNIQUE(original_filename, file_size, capture_time) ON CONFLICT IGNORE
);

CREATE TABLE IF NOT EXISTS folders (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_path         TEXT UNIQUE NOT NULL,
    display_name        TEXT,
    notes               TEXT,
    first_import_time   TEXT DEFAULT (datetime('now')),
    last_open_time      TEXT,
    recursive_show      INTEGER DEFAULT 1
);

CREATE TABLE IF NOT EXISTS workspace_state (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    current_folder_path TEXT,
    current_index       INTEGER DEFAULT 0,
    current_sort        TEXT DEFAULT 'capture_time_asc',
    filter_json         TEXT,
    last_selected_id    INTEGER,
    last_save_time      TEXT DEFAULT (datetime('now')),
    last_crash_marker   INTEGER DEFAULT 0,
    recent_folders_json TEXT
);

CREATE INDEX IF NOT EXISTS idx_photos_folder          ON photos(folder_path);
CREATE INDEX IF NOT EXISTS idx_photos_status          ON photos(status);
CREATE INDEX IF NOT EXISTS idx_photos_capture_time    ON photos(capture_time);
CREATE INDEX IF NOT EXISTS idx_photos_filename        ON photos(original_filename);
CREATE INDEX IF NOT EXISTS idx_photos_import_time     ON photos(import_time);
CREATE INDEX IF NOT EXISTS idx_photos_pair_group      ON photos(pair_group_id);
CREATE INDEX IF NOT EXISTS idx_photos_camera          ON photos(camera_model);
CREATE INDEX IF NOT EXISTS idx_photos_lens            ON photos(lens_model);
CREATE INDEX IF NOT EXISTS idx_photos_iso             ON photos(iso);
CREATE INDEX IF NOT EXISTS idx_photos_focal           ON photos(focal_length);
CREATE INDEX IF NOT EXISTS idx_photos_status_folder   ON photos(status, folder_path);
"#;
