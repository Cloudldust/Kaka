//! Database connection management and low-level maintenance (PRD 10).

pub mod folders;
pub mod photos;
pub mod schema;
pub mod workspace;

use crate::paths;
use rusqlite::Connection;
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 1;

/// A wrapping handle over the SQLite connection plus the raw path it was
/// opened from, so callers can compute backup file names.
pub struct Db {
    pub conn: Connection,
    pub path: std::path::PathBuf,
}

impl Db {
    /// Open (creating if necessary) the default database at %APPDATA%/Kaka.
    pub fn open_default() -> anyhow::Result<Self> {
        paths::ensure_dirs()?;
        let path = paths::db_path();
        Self::open(&path)
    }

    /// Open a database at an explicit path. Does NOT run schema init.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        // WAL mode + foreign keys and busy timeout (PRD 10.6).
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Db {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Open an in-memory database (used for tests).
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Ok(Db {
            conn,
            path: std::path::PathBuf::from(":memory:"),
        })
    }

    /// Run PRAGMA integrity_check. Returns true when the database reports "ok".
    /// Any other value means corruption.
    pub fn integrity_check(&self) -> anyhow::Result<bool> {
        let mut stmt = self
            .conn
            .prepare("PRAGMA integrity_check")?;
        let mut rows = stmt.query([])?;
        let mut result = String::new();
        while let Some(row) = rows.next()? {
            let r: String = row.get(0)?;
            result.push_str(&r);
            result.push('\n');
        }
        Ok(result.trim() == "ok")
    }

    /// Run a WAL checkpoint to flush the WAL back into the main db file.
    pub fn checkpoint(&self) -> anyhow::Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    /// Read the current schema version from `meta`. Defaults to 0 if the
    /// meta table does not exist yet.
    pub fn schema_version(&self) -> anyhow::Result<i64> {
        let v: Option<i64> = self
            .conn
            .query_row(
                "SELECT schema_version FROM meta WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .ok();
        Ok(v.unwrap_or(0))
    }
}
