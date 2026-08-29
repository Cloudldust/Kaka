//! Add-mode import (PRD 6.1, 6.4) — indexes existing photo folders without
//! copying files. This is the M1 import path.

use crate::db;
use crate::db::Db;
use crate::io::exif;
use crate::io::scanner::{self, ScanOptions};
use crate::io::thumbnails;
use crate::model::{CaptureTimeSource, Photo, Status};
use std::path::Path;

/// Progress callback: (phase, current, total, filename) -> continue? Return
/// false to abort the loop.
pub type ProgressFn<'a> = &'a mut dyn FnMut(&str, usize, usize, &str) -> bool;

/// Result of an add-mode import, used for the completion report (PRD 6.8).
#[derive(Debug, Default, Clone)]
pub struct ImportOutcome {
    pub added: usize,
    pub skipped_existing: usize,
    pub failed: usize,
    pub path_repaired: usize,
    pub scanned: usize,
    pub folder: String,
    pub failures: Vec<String>,
    pub repairs: Vec<String>,
}

impl ImportOutcome {
    /// The "成功" count shown to the user (photos actually added).
    pub fn success(&self) -> usize {
        self.added
    }
}

/// Run an add-mode import over `source`. Returns the outcome report.
///
/// `recursive` controls subfolder traversal; `dedup` controls whether to skip
/// three-element matches against the global database (PRD 6.4). Even when
/// dedup is off, the DB `ON CONFLICT IGNORE` constraint still swallows exact
/// duplicates, which are then reported as skipped.
/// Run an add-mode import over `source`. Returns the outcome report.
///
/// `recursive` controls subfolder traversal; `dedup` controls whether to skip
/// three-element matches against the global database (PRD 6.4). Even when
/// dedup is off, the DB `ON CONFLICT IGNORE` constraint still swallows exact
/// duplicates, which are then reported as skipped.
pub fn add_mode_import(
    db: &mut Db,
    source: &Path,
    recursive: bool,
    dedup: bool,
    progress: ProgressFn,
) -> anyhow::Result<ImportOutcome> {
    let mut noop = |_id: i64, _h: &str, _p: &str| {};
    add_mode_import_with_thumbs(db, source, recursive, dedup, progress, &mut noop)
}

/// Like [`add_mode_import`], but also reports each newly-inserted photo through
/// `on_thumb` so the caller can request background thumbnail generation while
/// the import is still running (used to prioritize the first few thumbnails).
pub fn add_mode_import_with_thumbs(
    db: &mut Db,
    source: &Path,
    recursive: bool,
    dedup: bool,
    progress: ProgressFn,
    on_thumb: &mut dyn FnMut(i64, &str, &str),
) -> anyhow::Result<ImportOutcome> {
    if !source.exists() || !source.is_dir() {
        anyhow::bail!(
            "{}{}",
            crate::i18n::t("源路径不存在或不是文件夹: ", "Source path does not exist or is not a folder: "),
            source.display()
        );
    }

    let items = scanner::scan_folder(source, ScanOptions { recursive })?;
    let total = items.len();
    let mut outcome = ImportOutcome {
        folder: source.to_string_lossy().into_owned(),
        ..Default::default()
    };
    outcome.scanned = total;

    // Ensure the folder (and its parents used by photos) get folder records.
    db::folders::ensure_folder(db, &source.to_string_lossy())?;

    for (idx, item) in items.iter().enumerate() {
        if !progress("索引", idx + 1, total, &item.filename) {
            // Cancelled.
            outcome.scanned = idx;
            break;
        }

        // EXIF + capture-time resolution (with mtime fallback, PRD 6.4).
        let ex = exif::parse_exif(&item.path);
        let capture_time = match &ex.capture_time {
            Some(t) => t.clone(),
            None => {
                let mtime = item
                    .modified
                    .and_then(|t| {
                        chrono::DateTime::<chrono::Local>::from(t)
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string()
                            .into()
                    })
                    .unwrap_or_else(|| "1900-01-01 00:00:00".to_string());
                mtime
            }
        };
        let capture_source = match &ex.capture_time {
            Some(_) => ex.capture_time_source,
            None => CaptureTimeSource::MtimeFallback,
        };

        let current_path = item.path.to_string_lossy().into_owned();
        let folder_path = item
            .path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| outcome.folder.clone());

        let photo = Photo {
            id: 0,
            original_filename: item.filename.clone(),
            file_size: item.file_size,
            capture_time: capture_time.clone(),
            current_path: current_path.clone(),
            folder_path: folder_path.clone(),
            status: Status::Untreated,
            thumb_hash: Some(thumbnails::thumb_hash_for(
                &current_path,
                item.file_size,
                &capture_time,
            )),
            decode_failed: false,
            // 仅预览模式 (PRD 2.3): only formats the app cannot fully decode
            // (HEIC without the system codec). RAW files are decodable via
            // rawler; exotic failures get the decode_failed flag at Z-time.
            preview_only: matches!(item.kind, crate::io::format::FormatKind::Heif),
            rotation_override: 0,
            exif_orientation: ex.orientation.unwrap_or(1),
            pair_group_id: None,
            iso: ex.iso,
            aperture: ex.aperture,
            shutter_speed: ex.shutter_speed,
            focal_length: ex.focal_length,
            camera_model: ex.camera_model.clone(),
            lens_model: ex.lens_model,
            capture_time_source: capture_source.as_str().to_string(),
            import_time: String::new(),
            last_access_time: String::new(),
            marked_delete_time: None,
            marked_review_time: None,
        };

        // Dedup / path-repair (PRD 6.4).
        let existing = db::photos::find_by_three_elements(
            db,
            &photo.original_filename,
            photo.file_size,
            &photo.capture_time,
        )?;
        if let Some(found) = existing {
            if dedup {
                if path_valid(&found.current_path) {
                    outcome.skipped_existing += 1;
                } else {
                    // Path repair.
                    db::photos::update_path(
                        db,
                        found.id,
                        &current_path,
                        &folder_path,
                    )?;
                    outcome.path_repaired += 1;
                    outcome.repairs.push(format!(
                        "{} -> {}",
                        found.current_path, current_path
                    ));
                }
                continue;
            }
            // dedup off: still can't double-insert; treat as duplicate skip.
            outcome.skipped_existing += 1;
            continue;
        }

        // Insert (silently skips on unique-conflict collision).
        match db::photos::insert_photo(db, &photo) {
            Ok(Some(photo_id)) => {
                outcome.added += 1;
                // Tell the caller about this new photo so it can request
                // background thumbnail generation (the first few, prioritized).
                if let Some(hash) = &photo.thumb_hash {
                    on_thumb(photo_id, hash, &item.path.to_string_lossy());
                }
            }
            Ok(None) => outcome.skipped_existing += 1,
            Err(e) => {
                outcome.failed += 1;
                outcome
                    .failures
                    .push(format!("{}: {e}", item.path.display()));
            }
        }
    }

    Ok(outcome)
}

fn path_valid(p: &str) -> bool {
    Path::new(p).exists()
}
