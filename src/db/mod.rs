//! Database connection management and low-level maintenance (PRD 10).

pub mod folders;
pub mod photos;
pub mod schema;
pub mod workspace;

use crate::paths;
use rusqlite::Connection;
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 2;

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

    /// A handle for a database that could not even be opened (bad/corrupt
    /// header): keep the real path on a throwaway in-memory connection so the
    /// repair dialog can still restore/reset the file at that path. Never fails.
    pub fn placeholder_at_default() -> Self {
        let conn = Connection::open_in_memory()
            .expect("in-memory sqlite connection cannot fail");
        Db {
            conn,
            path: paths::db_path(),
        }
    }

    /// Open a database at an explicit path. Does NOT run schema init.
    ///
    /// The pragmas are best-effort: a corrupt file must still open (so the
    /// startup can detect the corruption and show the repair dialog) instead of
    /// failing the whole launch. A healthy DB always sets them successfully.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        // WAL mode + foreign keys and busy timeout (PRD 10.6).
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
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
    /// Any other value (including hard SQLITE_CORRUPT errors on a badly damaged
    /// file) is reported as `Ok(false)` so startup can show the repair dialog
    /// instead of failing to launch.
    pub fn integrity_check(&self) -> anyhow::Result<bool> {
        let result: rusqlite::Result<String> = (|| {
            let mut stmt = self.conn.prepare("PRAGMA integrity_check")?;
            let mut rows = stmt.query([])?;
            let mut result = String::new();
            while let Some(row) = rows.next()? {
                let r: String = row.get(0)?;
                result.push_str(&r);
                result.push('\n');
            }
            Ok(result)
        })();
        Ok(result.map(|s| s.trim() == "ok").unwrap_or(false))
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
