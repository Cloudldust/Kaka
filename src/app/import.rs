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
    /// Per-file failure details (PRD 6.8).
    pub failures: Vec<ImportFailure>,
    /// Per-file path repairs (PRD 6.4 / 6.8).
    pub repairs: Vec<PathRepair>,
}

/// One failed file with full detail (PRD 6.8).
#[derive(Debug, Clone)]
pub struct ImportFailure {
    pub source_path: String,
    /// Copy mode: the intended destination path; add mode: empty.
    pub target_path: String,
    /// Short stable reason code (e.g. COPY_FAILED / INSERT_FAILED).
    pub reason_code: String,
    /// Human-readable reason description.
    pub reason: String,
    pub file_size: i64,
    /// 三要素捕获时间 (the capture time used for indexing/dedup).
    pub capture_time: String,
}

/// One repaired library path (PRD 6.4 / 6.8).
#[derive(Debug, Clone)]
pub struct PathRepair {
    pub old_path: String,
    pub new_path: String,
    pub original_filename: String,
    pub capture_time: String,
    /// When the repair was applied (local time).
    pub repaired_at: String,
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
    only: Option<&[String]>,
) -> anyhow::Result<ImportOutcome> {
    let mut noop = |_id: i64, _h: &str, _p: &str| {};
    add_mode_import_with_thumbs(db, source, recursive, dedup, progress, &mut noop, only)
}

/// Like [`add_mode_import`], but also reports each newly-inserted photo through
/// `on_thumb` so the caller can request background thumbnail generation while
/// the import is still running (used to prioritize the first few thumbnails).
///
/// `only` restricts the import to the given absolute paths (PRD 6.5 文件网格:
/// the user picked a subset in the pre-scan grid). Path-repair candidates are
/// included by the caller so the automatic repair (PRD 6.4) still runs.
pub fn add_mode_import_with_thumbs(
    db: &mut Db,
    source: &Path,
    recursive: bool,
    dedup: bool,
    progress: ProgressFn,
    on_thumb: &mut dyn FnMut(i64, &str, &str),
    only: Option<&[String]>,
) -> anyhow::Result<ImportOutcome> {
    if !source.exists() || !source.is_dir() {
        anyhow::bail!(
            "{}{}",
            crate::i18n::t("源路径不存在或不是文件夹: ", "Source path does not exist or is not a folder: "),
            source.display()
        );
    }

    let items = scanner::scan_folder(source, ScanOptions { recursive })?;
    // Grid subset (PRD 6.5): only the checked paths are imported.
    let items = match only {
        Some(list) => items
            .into_iter()
            .filter(|i| list.contains(&i.path.to_string_lossy().into_owned()))
            .collect::<Vec<_>>(),
        None => items,
    };
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
            aperture_num: ex.aperture.as_deref().and_then(exif::parse_aperture_num),
            shutter_num: ex.shutter_speed.as_deref().and_then(exif::parse_shutter_num),
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
                    outcome.repairs.push(PathRepair {
                        old_path: found.current_path.clone(),
                        new_path: current_path.clone(),
                        original_filename: photo.original_filename.clone(),
                        capture_time: capture_time.clone(),
                        repaired_at: chrono::Local::now()
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string(),
                    });
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
                outcome.failures.push(ImportFailure {
                    source_path: item.path.to_string_lossy().into_owned(),
                    target_path: String::new(),
                    reason_code: "INSERT_FAILED".into(),
                    reason: e.to_string(),
                    file_size: item.file_size,
                    capture_time,
                });
            }
        }
    }

    Ok(outcome)
}

fn path_valid(p: &str) -> bool {
    Path::new(p).exists()
}

/// Quote a CSV field when it contains a separator, quote or newline.
fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// PRD 6.8: write the import failure list as CSV (UTF-8 BOM + CRLF).
/// Fields: 序号、源路径、目标路径、失败原因代码、失败原因中文描述、文件大小、
/// 三要素捕获时间. Returns the row count.
pub fn export_failure_csv(path: &str, failures: &[ImportFailure]) -> anyhow::Result<usize> {
    let mut text = String::from("\u{feff}");
    text.push_str("序号,源路径,目标路径,失败原因代码,失败原因描述,文件大小,三要素捕获时间\r\n");
    for (i, f) in failures.iter().enumerate() {
        text.push_str(&format!(
            "{},{},{},{},{},{},{}\r\n",
            i + 1,
            csv_field(&f.source_path),
            csv_field(&f.target_path),
            csv_field(&f.reason_code),
            csv_field(&f.reason),
            f.file_size,
            csv_field(&f.capture_time),
        ));
    }
    std::fs::write(path, text)?;
    Ok(failures.len())
}

/// PRD 6.8: write the path-repair list as CSV (UTF-8 BOM + CRLF).
/// Fields: 序号、旧路径、新路径、原始文件名、三要素捕获时间、修复时间戳.
/// Returns the row count.
pub fn export_repair_csv(path: &str, repairs: &[PathRepair]) -> anyhow::Result<usize> {
    let mut text = String::from("\u{feff}");
    text.push_str("序号,旧路径,新路径,原始文件名,三要素捕获时间,修复时间戳\r\n");
    for (i, r) in repairs.iter().enumerate() {
        text.push_str(&format!(
            "{},{},{},{},{},{}\r\n",
            i + 1,
            csv_field(&r.old_path),
            csv_field(&r.new_path),
            csv_field(&r.original_filename),
            csv_field(&r.capture_time),
            csv_field(&r.repaired_at),
        ));
    }
    std::fs::write(path, text)?;
    Ok(repairs.len())
}

/// Pre-scan verdict for one file (PRD 6.5 第三步 去重扫描).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrescanMark {
    /// Not in the library — will be imported.
    New,
    /// Three-element match with a valid library path — already imported.
    Exists,
    /// Three-element match but the library path is gone — importing repairs
    /// the stored path (PRD 6.4) instead of inserting a new photo.
    PathRepair,
}

/// One pre-scanned file row (PRD 6.5 / UI 5.1-4 文件网格).
#[derive(Debug, Clone)]
pub struct PrescanItem {
    pub path: String,
    pub filename: String,
    pub file_size: i64,
    pub capture_time: String,
    /// Thumbnail cache hash for the scan grid (same formula as the library).
    pub thumb_hash: String,
    pub mark: PrescanMark,
    /// RAW+JPG same-stem group index within this scan (None = unpaired).
    pub pair_group: Option<usize>,
}

/// Scan `source` and mark every supported file via the three-element
/// comparison against the library (PRD 6.5 第三步 去重扫描 / PRD 6.4).
/// `progress(done, total)` runs per file — return false to cancel, which
/// yields `Ok(None)`. Same-stem RAW+JPG groups (≥2 members in one folder)
/// are numbered so the grid/stats can report 组.
pub fn prescan_mark(
    db: &mut Db,
    source: &Path,
    recursive: bool,
    pair_threshold_secs: i64,
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> anyhow::Result<Option<Vec<PrescanItem>>> {
    if !source.exists() || !source.is_dir() {
        anyhow::bail!(
            "{}{}",
            crate::i18n::t("源路径不存在或不是文件夹: ", "Source path does not exist or is not a folder: "),
            source.display()
        );
    }
    let items = scanner::scan_folder(source, ScanOptions { recursive })?;
    let total = items.len();
    let mut out: Vec<PrescanItem> = Vec::with_capacity(total);
    for (idx, item) in items.iter().enumerate() {
        if !progress(idx + 1, total) {
            return Ok(None);
        }
        let ex = exif::parse_exif(&item.path);
        let capture_time = match &ex.capture_time {
            Some(t) => t.clone(),
            None => item
                .modified
                .and_then(|t| {
                    chrono::DateTime::<chrono::Local>::from(t)
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                        .into()
                })
                .unwrap_or_else(|| "1900-01-01 00:00:00".to_string()),
        };
        let mark = match db::photos::find_by_three_elements(
            db,
            &item.filename,
            item.file_size,
            &capture_time,
        )? {
            Some(found) if path_valid(&found.current_path) => PrescanMark::Exists,
            Some(_) => PrescanMark::PathRepair,
            None => PrescanMark::New,
        };
        let path = item.path.to_string_lossy().into_owned();
        let thumb_hash = thumbnails::thumb_hash_for(&path, item.file_size, &capture_time);
        out.push(PrescanItem {
            path,
            filename: item.filename.clone(),
            file_size: item.file_size,
            capture_time,
            thumb_hash,
            mark,
            pair_group: None,
        });
    }
    // RAW+JPG pairing: same (folder, stem) with ≥2 members forms a group.
    let mut groups: std::collections::HashMap<(String, String), Vec<usize>> = Default::default();
    for (i, it) in out.iter().enumerate() {
        let parent = Path::new(&it.path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = Path::new(&it.filename)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        groups.entry((parent, stem)).or_default().push(i);
    }
    let mut group_id = 0usize;
    for members in groups.into_values() {
        if members.len() < 2 {
            continue;
        }
        // PRD 6.1.3: 同目录同名 + 拍摄时间差 ≤ 阈值（默认 5s）才判定为一组。
        let times: Vec<Option<i64>> = members
            .iter()
            .map(|&i| crate::model::capture_time_epoch(&out[i].capture_time))
            .collect();
        if times.iter().any(|t| t.is_none()) {
            continue;
        }
        let (mn, mx) = (
            times.iter().flatten().min().copied().unwrap_or(0),
            times.iter().flatten().max().copied().unwrap_or(0),
        );
        if mx - mn > pair_threshold_secs {
            continue;
        }
        for i in members {
            out[i].pair_group = Some(group_id);
        }
        group_id += 1;
    }
    Ok(Some(out))
}
