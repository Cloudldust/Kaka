//! Full cache rebuild (PRD 9.6): regenerate thumbnail + preview caches for
//! every photo record in the library, overwriting whatever is on disk.

use crate::db::{self, Db};
use crate::io::thumbnails;
use std::path::Path;

/// Regenerate the thumbnail + preview cache files for every photo row,
/// thumbnails first, previews after (inside `generate_caches`, which decodes
/// the source once). Existing cache files are deleted first so they are truly
/// regenerated, and the fresh files re-register in cache_index.db
/// (`register_cache_file` inside the generators).
///
/// `is_cancelled` is checked before each photo — already-rebuilt photos stay
/// on disk. `progress(done, total)` is called after each photo. Returns
/// (processed, failed); failures (missing source, undecodable, IO error) are
/// logged with the path.
pub fn rebuild_all_caches(
    db: &Db,
    dpi_scale: f32,
    is_cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(usize, usize),
) -> (usize, usize) {
    let rows = match db::photos::list_all_basic(db) {
        Ok(rows) => rows,
        Err(e) => {
            log::error!("缓存重建：读取照片列表失败: {e}");
            return (0, 0);
        }
    };
    let total = rows.len();
    let mut done = 0usize;
    let mut failed = 0usize;
    for (id, path, thumb_hash, file_size, capture_time) in &rows {
        if is_cancelled() {
            break;
        }
        let hash = thumb_hash
            .clone()
            .unwrap_or_else(|| thumbnails::thumb_hash_for(path, *file_size, capture_time));
        let src = Path::new(path);
        let res = if !src.exists() {
            Err(anyhow::anyhow!("源文件不存在"))
        } else {
            // Overwrite semantics: drop the old files, then let
            // generate_caches decode the source once and rewrite both.
            let _ = std::fs::remove_file(thumbnails::thumb_path(&hash, dpi_scale));
            let _ = std::fs::remove_file(thumbnails::preview_path(&hash));
            thumbnails::generate_caches(src, &hash, dpi_scale)
        };
        match res {
            Ok(true) => {}
            Ok(false) => {
                failed += 1;
                log::warn!("缓存重建：无法解码，跳过 {path} (id {id})");
            }
            Err(e) => {
                failed += 1;
                log::warn!("缓存重建失败 {path} (id {id}): {e}");
            }
        }
        done += 1;
        progress(done, total);
    }
    (done, failed)
}
