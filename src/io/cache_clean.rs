//! Disk-cache eviction (PRD 9.4): expire-by-age + capacity LRU cleanup.
//!
//! Two conditions mark an entry as reclaimable, whichever hits first:
//! - 时间条件: creation older than `expire_days` (default 30);
//! - 容量条件: total cache size over the configured cap (default 20 GB) —
//!   evict LRU-oldest by last_access until usage drops to 85% of the cap.
//!
//! Incremental cleanups are capped at `max_files` (100 per PRD) so they never
//! hog IO; the settings-page 「立即清理」 passes an unlimited budget.

use super::cache_index::CacheIndex;
use std::path::Path;

/// Percentage of the capacity cap to shrink to when over budget (PRD 9.4: 85%).
const EVICT_FLOOR_PCT: u64 = 85;

#[derive(Debug, Default, Clone)]
pub struct CleanStats {
    pub deleted: usize,
    pub expired: usize,
    pub freed_bytes: u64,
}

/// Reconcile the index with reality: register files on disk that the index
/// doesn't know about (caches written before the index existed, crashes
/// mid-write), using the file mtime as both created_at and last_access_time.
/// Returns how many files were newly registered.
pub fn reconcile(idx: &CacheIndex, cache_dir: &Path) -> usize {
    let known: std::collections::HashSet<String> = idx
        .all_rel_paths()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let mut registered = 0usize;
    for sub in ["thumbs", "previews"] {
        let dir = cache_dir.join(sub);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let rel = format!("{sub}/{}", entry.file_name().to_string_lossy());
            if known.contains(&rel) {
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .map(|t| {
                    chrono::DateTime::<chrono::Local>::from(t)
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                })
                .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());
            let kind = super::cache_index::kind_for(&rel);
            if idx.record_write_at(&rel, kind, meta.len(), &mtime, &mtime).is_ok() {
                registered += 1;
            }
        }
    }
    registered
}

/// One cleanup pass. `max_files` caps how many files may be removed; `progress`
/// receives the running deleted count and returns false to cancel. Expired
/// entries go first, then capacity-driven LRU eviction.
pub fn run_cleanup(
    idx: &CacheIndex,
    cache_dir: &Path,
    cap_bytes: u64,
    expire_days: u64,
    max_files: usize,
    progress: &mut dyn FnMut(usize) -> bool,
) -> anyhow::Result<CleanStats> {
    let mut stats = CleanStats::default();
    let mut budget = max_files;

    // 1) Expired by creation age.
    for rel in idx.expired(expire_days)? {
        if budget == 0 || !progress(stats.deleted) {
            return Ok(stats);
        }
        let freed = remove_entry(idx, cache_dir, &rel);
        stats.freed_bytes += freed;
        stats.expired += 1;
        stats.deleted += 1;
        budget -= 1;
    }

    // 2) Capacity: only when usage exceeds the cap, evict LRU-oldest until
    //    usage drops to 85% of the cap (PRD 9.4).
    if cap_bytes == 0 {
        return Ok(stats);
    }
    // Multiply before dividing: integer-division first would floor to 0 for
    // small caps (and test fixtures). u64*85 cannot overflow at ≤100 GB caps.
    let floor = (cap_bytes * EVICT_FLOOR_PCT / 100) as i64;
    let mut total = idx.total_size()?;
    if total > cap_bytes as i64 {
        let mut batch = idx.lru_oldest(500)?;
        while total > floor && budget > 0 {
            if !progress(stats.deleted) {
                return Ok(stats);
            }
            let Some((rel, size)) = batch.first().cloned() else {
                break;
            };
            batch.remove(0);
            let freed = remove_entry(idx, cache_dir, &rel);
            stats.freed_bytes += freed;
            stats.deleted += 1;
            budget -= 1;
            total = total.saturating_sub(size.max(0));
            if batch.is_empty() {
                batch = idx.lru_oldest(500)?;
            }
        }
    }
    Ok(stats)
}

/// Delete one cache entry: drop the index row, then the file (missing files
/// are fine — the row is the source of truth here).
fn remove_entry(idx: &CacheIndex, cache_dir: &Path, rel_path: &str) -> u64 {
    let _ = idx.delete_row(rel_path);
    let p = cache_dir.join(rel_path);
    let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    let _ = std::fs::remove_file(&p);
    size
}
