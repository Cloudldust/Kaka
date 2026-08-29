//! Disk-cache index (PRD 9.2 cache_index.db) + LRU/expiration bookkeeping
//! (PRD 9.4).
//!
//! Every thumbnail/preview written into the cache directory is registered here
//! with its size and timestamps; every access bumps `last_access_time`. The
//! cleanup worker (see [`super::cache_clean`]) evicts by (a) age ≥ expire_days
//! since creation and (b) total size over capacity, LRU-oldest first.
//!
//! The index is best-effort: any failure to open or write it is swallowed by
//! the global helpers so caching itself never breaks.

use crate::paths;
use rusqlite::{params, Connection};
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

const CACHE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS cache_entries (
    rel_path         TEXT PRIMARY KEY,
    kind             TEXT NOT NULL,
    size             INTEGER NOT NULL,
    created_at       TEXT NOT NULL,
    last_access_time TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cache_access  ON cache_entries(last_access_time);
CREATE INDEX IF NOT EXISTS idx_cache_created ON cache_entries(created_at);
"#;

pub struct CacheIndex {
    conn: Connection,
}

impl CacheIndex {
    /// Open (creating if needed) the index at %LOCALAPPDATA%/Kaka/cache.
    pub fn open_default() -> anyhow::Result<Self> {
        Self::open(&paths::cache_index_path())
    }

    /// Open an index at an explicit path (tests use a temp dir).
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(CACHE_SCHEMA)?;
        Ok(CacheIndex { conn })
    }

    /// Register a freshly written cache file (upsert).
    pub fn record_write(&self, rel_path: &str, kind: &str, size: u64) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO cache_entries (rel_path, kind, size, created_at, last_access_time)
             VALUES (?1, ?2, ?3, datetime('now'), datetime('now'))
             ON CONFLICT(rel_path) DO UPDATE SET
                kind = excluded.kind,
                size = excluded.size,
                last_access_time = datetime('now')",
            params![rel_path, kind, size as i64],
        )?;
        Ok(())
    }

    /// Test/reconcile helper: register with explicit timestamps
    /// ("YYYY-MM-DD HH:MM:SS").
    pub fn record_write_at(
        &self,
        rel_path: &str,
        kind: &str,
        size: u64,
        created: &str,
        accessed: &str,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO cache_entries (rel_path, kind, size, created_at, last_access_time)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(rel_path) DO UPDATE SET
                kind = excluded.kind,
                size = excluded.size,
                created_at = excluded.created_at,
                last_access_time = excluded.last_access_time",
            params![rel_path, kind, size as i64, created, accessed],
        )?;
        Ok(())
    }

    /// Bump last_access_time (LRU recency).
    pub fn touch(&self, rel_path: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE cache_entries SET last_access_time = datetime('now') WHERE rel_path = ?1",
            params![rel_path],
        )?;
        Ok(())
    }

    pub fn has(&self, rel_path: &str) -> anyhow::Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM cache_entries WHERE rel_path = ?1",
            params![rel_path],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Total bytes across all registered entries.
    pub fn total_size(&self) -> anyhow::Result<i64> {
        let v: i64 =
            self.conn
                .query_row("SELECT COALESCE(SUM(size), 0) FROM cache_entries", [], |r| {
                    r.get(0)
                })?;
        Ok(v)
    }

    pub fn count(&self) -> anyhow::Result<i64> {
        let v: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM cache_entries", [], |r| r.get(0))?;
        Ok(v)
    }

    /// All registered rel paths (for reconcile diffing).
    pub fn all_rel_paths(&self) -> anyhow::Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT rel_path FROM cache_entries")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.flatten().collect())
    }

    /// rel paths created more than `days` ago (PRD 9.4 时间条件: 生成时间距今).
    pub fn expired(&self, days: u64) -> anyhow::Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT rel_path FROM cache_entries
             WHERE created_at <= datetime('now', ?1)
             ORDER BY created_at ASC",
        )?;
        let modifier = format!("-{days} days");
        let rows = stmt.query_map(params![modifier], |r| r.get::<_, String>(0))?;
        Ok(rows.flatten().collect())
    }

    /// Up to `limit` entries ordered by last_access_time ascending (LRU-oldest
    /// first) with their sizes, for capacity eviction.
    pub fn lru_oldest(&self, limit: usize) -> anyhow::Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT rel_path, size FROM cache_entries
             ORDER BY last_access_time ASC, rel_path ASC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        Ok(rows.flatten().collect())
    }

    pub fn delete_row(&self, rel_path: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "DELETE FROM cache_entries WHERE rel_path = ?1",
            params![rel_path],
        )?;
        Ok(())
    }
}

// ---- Global best-effort handle (used from the UI / worker threads) ----

static GLOBAL_INDEX: OnceLock<Mutex<Option<CacheIndex>>> = OnceLock::new();
static TOUCHED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn with_global(f: impl FnOnce(&mut CacheIndex)) {
    let lock = GLOBAL_INDEX.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = lock.lock() else {
        return;
    };
    if guard.is_none() {
        *guard = CacheIndex::open_default().ok();
    }
    if let Some(idx) = guard.as_mut() {
        f(idx);
    }
}

/// Normalize a path inside the cache root to a forward-slash rel path
/// (e.g. "thumbs/abc123.jpg"). Returns None for paths outside the cache root,
/// which keeps test/temp files out of the real index.
fn rel_of(path: &Path) -> Option<String> {
    path.strip_prefix(paths::cache_dir())
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

/// Index kind for a rel path.
pub fn kind_for(rel: &str) -> &'static str {
    if rel.starts_with("previews/") {
        "preview"
    } else if rel.contains("@2x") {
        "thumb2x"
    } else {
        "thumb"
    }
}

/// Register a freshly written cache file with the global index. Files outside
/// the cache root (tests, temp output) are ignored. Best-effort.
pub fn register_cache_file(dest: &Path) {
    let Some(rel) = rel_of(dest) else {
        return;
    };
    let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    let kind = kind_for(&rel);
    with_global(|idx| {
        let _ = idx.record_write(&rel, kind, size);
    });
}

/// Bump last_access for a cache file once per path per process (plenty for LRU
/// recency; avoids a DB write on every texture load).
pub fn touch_path(path: &Path) {
    let Some(rel) = rel_of(path) else {
        return;
    };
    let seen = TOUCHED.get_or_init(|| Mutex::new(HashSet::new()));
    let unseen = match seen.lock() {
        Ok(mut s) => s.insert(rel.clone()),
        Err(_) => false,
    };
    if !unseen {
        return;
    }
    with_global(|idx| {
        let _ = idx.touch(&rel);
    });
}
