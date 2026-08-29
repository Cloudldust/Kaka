//! Copy-mode import (PRD 6.1.2, 6.1.4, 6.5) — physically copies photos from a
//! source (e.g. a memory card) into a target directory, and indexes the copies.

use crate::db::{self, Db};
use crate::io::exif;
use crate::io::scanner::{self, ScanItem, ScanOptions};
use crate::io::thumbnails;
use crate::model::{CaptureTimeSource, Photo, Status};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

/// How to organize files under the target directory (PRD 6.1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgMode {
    /// 保持原结构 (default): recreate the source-relative path under target.
    Structure,
    /// 按拍摄日期建子文件夹 (YYYY-MM-DD, or "未知日期").
    Date,
    /// 全部平铺 (flat); conflicts get _dupN suffixes.
    Flat,
}

impl OrgMode {
    pub fn label(self) -> String {
        let (zh, en) = match self {
            OrgMode::Structure => ("保持原结构", "Keep original structure"),
            OrgMode::Date => ("按拍摄日期建子文件夹", "Subfolder by capture date"),
            OrgMode::Flat => ("全部平铺", "Flat into one folder"),
        };
        crate::i18n::t(zh, en).to_string()
    }
    pub fn code(self) -> &'static str {
        match self {
            OrgMode::Structure => "structure",
            OrgMode::Date => "date",
            OrgMode::Flat => "flat",
        }
    }
    pub fn from_code(code: &str) -> Self {
        match code {
            "date" => OrgMode::Date,
            "flat" => OrgMode::Flat,
            _ => OrgMode::Structure,
        }
    }
}

/// Copy-mode options (PRD 6.3).
#[derive(Debug, Clone)]
pub struct CopyOptions {
    pub target_dir: String,
    pub org_mode: OrgMode,
    pub recursive: bool,
    pub dedup: bool,
    /// 清空存储卡 (PRD 6.7): after a fully-successful import, move the copied
    /// source files on the removable card to the recycle bin. Default off.
    pub clear_card: bool,
}

impl Default for CopyOptions {
    fn default() -> Self {
        CopyOptions {
            target_dir: String::new(),
            org_mode: OrgMode::Structure,
            recursive: true,
            dedup: true,
            clear_card: false,
        }
    }
}

/// Result of a copy-mode import, for the completion report (PRD 6.8).
#[derive(Debug, Default, Clone)]
pub struct CopyOutcome {
    pub copied: usize,
    pub skipped_existing: usize,
    pub failed: usize,
    pub scanned: usize,
    pub total_size: u64,
    pub target_dir: String,
    pub failures: Vec<String>,
    /// 清空存储卡: whether the user asked to clear the source card after import.
    pub clear_card: bool,
    /// Source paths that were copied AND recorded in the DB — the only files
    /// eligible to be moved to the recycle bin (PRD 6.7).
    pub copied_sources: Vec<String>,
}

/// Progress callback: (phase, current, total, filename) -> continue? Return
/// false to abort the loop. `phase` is a short stage label (e.g. "检查"/"拷贝").
pub type ProgressFn<'a> = &'a mut dyn FnMut(&str, usize, usize, &str) -> bool;

/// Run a copy-mode import. `resume` (when true) enforces dedup regardless of the
/// user setting, per PRD 6.7.1. `resume_base` is the number of files already
/// finished in a previous, interrupted run, so progress continues from that
/// point instead of restarting at 0 (0 for a fresh import).
pub fn copy_mode_import(
    db: &mut Db,
    source: &Path,
    options: &CopyOptions,
    resume: bool,
    resume_base: usize,
    progress: ProgressFn,
) -> anyhow::Result<CopyOutcome> {
    if !source.exists() || !source.is_dir() {
        anyhow::bail!(
            "{}{}",
            crate::i18n::t("源路径不存在或不是文件夹: ", "Source path does not exist or is not a folder: "),
            source.display()
        );
    }
    if options.target_dir.trim().is_empty() {
        anyhow::bail!("{}", crate::i18n::t("未设置目标目录", "Target directory not set"));
    }
    std::fs::create_dir_all(&options.target_dir)?;

    let items = scanner::scan_folder(source, ScanOptions { recursive: options.recursive })?;
    let total = items.len();
    let mut outcome = CopyOutcome {
        target_dir: options.target_dir.clone(),
        clear_card: options.clear_card,
        ..Default::default()
    };
    outcome.scanned = total;

    // Resolve the dedup policy (resume forces dedup on).
    let dedup = options.dedup || resume;

    // Build the list of copy jobs (dedup-skipped files are counted, not copied).
    // Report progress during this (EXIF + dedup) phase too, so a large import
    // shows activity instead of a stuck "0/0".
    let mut prep_cancelled = false;
    let mut jobs: Vec<CopyJob> = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        if !progress("检查", idx + 1, total, &item.filename) {
            outcome.scanned = idx; // cancelled during prep
            prep_cancelled = true;
            break;
        }
        let ex = exif::parse_exif(&item.path);
        let capture_time = resolve_capture_time(&ex, item);
        let capture_source = match &ex.capture_time {
            Some(_) => ex.capture_time_source,
            None => CaptureTimeSource::MtimeFallback,
        };
        let existing = db::photos::find_by_three_elements(
            db,
            &item.filename,
            item.file_size,
            &capture_time,
        )?;
        if dedup && existing.is_some() {
            outcome.skipped_existing += 1;
            continue;
        }
        jobs.push(CopyJob {
            item: item.clone(),
            ex,
            capture_time,
            capture_source,
            target: PathBuf::new(),
        });
    }
    if prep_cancelled {
        // Cancelled during the analysis/prep phase: nothing copied yet.
        return Ok(outcome);
    }

    // Compute total bytes across all files to be copied (photos + followed
    // sidecars) for the disk-space pre-check (PRD 6.1.4). We also estimate the
    // sidecar bytes by looking them up on disk.
    let mut total_size: u64 = 0;
    for job in &jobs {
        total_size += job.item.file_size as u64;
        if let Some(side) = find_sidecar(&job.item) {
            total_size += std::fs::metadata(&side).map(|m| m.len()).unwrap_or(0);
        }
    }
    outcome.total_size = total_size;

    if !jobs.is_empty() {
        check_disk_space(&options.target_dir, total_size)?;
    }

    // Assign target paths (with flat-mode _dup conflict resolution).
    resolve_target_paths(source, &mut jobs, options, &mut outcome)?;

    // Perform the copies with a small pool of worker threads that only do file
    // I/O; the DB writes are collected single-threaded and batched afterwards.
    // File copying is the bulk of the work, and decoupling the (slow) thumbnail
    // generation from this loop is what makes imports much faster.
    if !jobs.is_empty() {
        let n_workers = copy_worker_count(jobs.len());
        let jobs_arc = Arc::new(jobs);
        let opts_arc = Arc::new(options.clone());
        let next = Arc::new(AtomicUsize::new(0));
        let (res_tx, res_rx) = mpsc::channel::<(usize, std::result::Result<Photo, String>)>();

        let mut handles = Vec::with_capacity(n_workers);
        for _ in 0..n_workers {
            let next = Arc::clone(&next);
            let jobs = Arc::clone(&jobs_arc);
            let opts = Arc::clone(&opts_arc);
            let res_tx = res_tx.clone();
            handles.push(std::thread::spawn(move || {
                loop {
                    let idx = next.fetch_add(1, Ordering::SeqCst);
                    if idx >= jobs.len() {
                        break;
                    }
                    let job = &jobs[idx];
                    let result = copy_files(job)
                        .map(|_| build_photo_from_job(job, &opts))
                        .map_err(|e| format!("{e}"));
                    if res_tx.send((idx, result)).is_err() {
                        break; // receiver dropped (cancelled)
                    }
                }
            }));
        }
        drop(res_tx); // so the loop ends when the last worker drops its clone

        let mut completed = 0usize;
        let mut cancelled = false;
        while let Ok((idx, result)) = res_rx.recv() {
            completed += 1;
            let name = jobs_arc[idx].item.filename.clone();
            if !progress("拷贝", resume_base.saturating_add(completed), total, &name) {
                cancelled = true;
                break;
            }
            match result {
                Ok(photo) => {
                    // Insert each photo as soon as it is copied so a hard kill
                    // still leaves the finished files in the DB (resume skips
                    // them). The DB runs WAL + synchronous=NORMAL, so a single
                    // insert does not pay a per-row fsync.
                    match db::photos::insert_photo(db, &photo) {
                        Ok(Some(_)) => {
                            outcome.copied += 1;
                            // Remember the source path so 清空存储卡 can move the
                            // successfully-copied files on the card to the recycle
                            // bin (PRD 6.7).
                            outcome
                                .copied_sources
                                .push(jobs_arc[idx].item.path.to_string_lossy().into_owned());
                        }
                        Ok(None) => outcome.skipped_existing += 1,
                        Err(e) => {
                            outcome.failed += 1;
                            outcome.failures.push(format!(
                                "{}: {e}",
                                jobs_arc[idx].item.path.display()
                            ));
                        }
                    }
                }
                Err(e) => {
                    outcome.failed += 1;
                    outcome
                        .failures
                        .push(format!("{}: {e}", jobs_arc[idx].item.path.display()));
                }
            }
        }
        // Drop the receiver so any still-running workers see a closed channel and stop.
        drop(res_rx);
        for h in handles {
            let _ = h.join();
        }
        if cancelled {
            // Leave the import session incomplete so it can be resumed later.
            return Ok(outcome);
        }
    }

    // Reconcile RAW+JPG pairing within the target folder (PRD 6.1.3).
    reconcile_pairs(db, &options.target_dir)?;

    Ok(outcome)
}

/// A single copy job with its resolved destination and metadata.
struct CopyJob {
    item: ScanItem,
    ex: exif::ExifData,
    capture_time: String,
    capture_source: CaptureTimeSource,
    target: PathBuf,
}

/// Copy exactly one photo (plus any followed sidecar) to its target path. This
/// is pure file I/O (no DB writes, no thumbnail generation), so it runs safely
/// on parallel worker threads.
fn copy_files(job: &CopyJob) -> anyhow::Result<()> {
    let target = job.target.clone();
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Atomic copy: temp file, then rename.
    atomic_copy(&job.item.path, &target)?;

    if let Some(side) = find_sidecar(&job.item) {
        let side_name = side.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let side_target = target.with_file_name(&side_name);
        if let Some(parent) = side_target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = atomic_copy(&side, &side_target);
    }
    Ok(())
}

/// Build a `Photo` record for a copied job, deriving the cache hash from the
/// destination path and the folder from the target parent.
fn build_photo_from_job(job: &CopyJob, options: &CopyOptions) -> Photo {
    let target = job.target.to_string_lossy().into_owned();
    let folder_path = job
        .target
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| options.target_dir.clone());
    let hash = thumbnails::thumb_hash_for(&target, job.item.file_size, &job.capture_time);
    build_photo(job, target, folder_path, hash)
}

/// Worker count for the parallel copy pool: capped at 4, at least 1.
fn copy_worker_count(files: usize) -> usize {
    if files == 0 {
        1
    } else {
        files.min(4)
    }
}

fn build_photo(
    job: &CopyJob,
    current_path: String,
    folder_path: String,
    hash: String,
) -> Photo {
    Photo {
        id: 0,
        original_filename: job.item.filename.clone(),
        file_size: job.item.file_size,
        capture_time: job.capture_time.clone(),
        current_path,
        folder_path,
        status: Status::Untreated,
        thumb_hash: Some(hash),
        decode_failed: false,
        // 仅预览模式 (PRD 2.3): HEIC only; RAW is fully decodable (see import).
        preview_only: matches!(job.item.kind, crate::io::format::FormatKind::Heif),
        rotation_override: 0,
        exif_orientation: job.ex.orientation.unwrap_or(1),
        pair_group_id: None,
        iso: job.ex.iso,
        aperture: job.ex.aperture.clone(),
        shutter_speed: job.ex.shutter_speed.clone(),
        focal_length: job.ex.focal_length,
        camera_model: job.ex.camera_model.clone(),
        lens_model: job.ex.lens_model.clone(),
        capture_time_source: job.capture_source.as_str().to_string(),
        import_time: String::new(),
        last_access_time: String::new(),
        marked_delete_time: None,
        marked_review_time: None,
    }
}

/// Resolve the final target path for every job, appending _dupN in flat mode
/// when two files collide (PRD 6.1.2 选项C).
fn resolve_target_paths(
    source_root: &Path,
    jobs: &mut [CopyJob],
    options: &CopyOptions,
    _outcome: &mut CopyOutcome,
) -> anyhow::Result<()> {
    let mut used: HashMap<(String, String), usize> = HashMap::new(); // (dir, filename) -> count
    let target_root = Path::new(&options.target_dir);

    for job in jobs.iter_mut() {
        let base_dir = match options.org_mode {
            OrgMode::Structure => {
                let rel = job
                    .item
                    .path
                    .strip_prefix(source_root)
                    .unwrap_or_else(|_| job.item.path.as_path())
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default();
                target_root.join(&rel)
            }
            OrgMode::Date => {
                let sub = capture_date(&job.capture_time);
                target_root.join(sub)
            }
            OrgMode::Flat => target_root.to_path_buf(),
        };

        // Conflict resolution by (dir, filename).
        let filename = job.item.filename.clone();
        let key = (base_dir.to_string_lossy().into_owned(), filename.clone());
        let n = used.entry(key).or_insert(0);
        let final_name = if *n == 0 {
            filename.clone()
        } else {
            let stem = job
                .item
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("img");
            let ext = job.item.path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext.is_empty() {
                format!("{stem}_dup{n}")
            } else {
                format!("{stem}_dup{n}.{ext}")
            }
        };
        *n += 1;
        job.target = base_dir.join(final_name);
    }
    Ok(())
}

pub(crate) fn capture_date(capture_time: &str) -> String {
    if capture_time.len() >= 10 {
        capture_time[..10].to_string()
    } else {
        "未知日期".to_string()
    }
}

/// Find a same-stem sidecar (.xmp/.dop/.pp3) beside a photo.
pub(crate) fn find_sidecar(item: &ScanItem) -> Option<PathBuf> {
    for ext in ["xmp", "dop", "pp3"] {
        let candidate = item.path.with_extension(ext);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Atomic-ish copy: write to a temp file in the destination dir, then rename.
pub(crate) fn atomic_copy(src: &Path, dest: &Path) -> anyhow::Result<()> {
    let dir = dest.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(
        ".{}.kaka.tmp",
        dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "kaka".into())
    ));
    std::fs::copy(src, &tmp)?;
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// Disk-space pre-check (PRD 6.1.4): free_size >= total_size * 1.05 + 512MB.
fn check_disk_space(target_dir: &str, total_size: u64) -> anyhow::Result<()> {
    let free = fs4::available_space(target_dir)?;
    let required = (total_size as f64 * 1.05) as u64 + (512u64 << 20);
    if free < required {
        let msg = match crate::i18n::lang() {
            crate::i18n::Lang::Zh => format!(
                "目标磁盘空间不足。需要约 {}，可用仅 {}。请清理目标磁盘空间或更换目标目录。",
                human_bytes(required as i64),
                human_bytes(free as i64)
            ),
            crate::i18n::Lang::En => format!(
                "Not enough disk space on the target drive. Need ~{}, only {} available. Free up space or pick another target directory.",
                human_bytes(required as i64),
                human_bytes(free as i64)
            ),
        };
        anyhow::bail!("{msg}");
    }
    Ok(())
}

/// Assign `pair_group_id` to same-stem RAW+JPG groups in a folder (PRD 6.1.3).
pub fn reconcile_pairs(db: &mut Db, folder_prefix: &str) -> anyhow::Result<()> {
    let items = db::photos::list_items_in_folder(db, folder_prefix, crate::model::SortOrder::CaptureTimeAsc)?;
    // Group by (folder_path, stem).
    let mut groups: HashMap<(String, String), Vec<i64>> = HashMap::new();
    for p in &items {
        let stem = std::path::Path::new(&p.original_filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_lowercase();
        groups.entry((p.folder_path.clone(), stem)).or_default().push(p.id);
    }
    for (_, ids) in groups {
        if ids.len() < 2 {
            continue;
        }
        // Assign a shared pair_group_id to every member of a same-stem group.
        let group_id = ids[0];
        for id in &ids {
            db.conn.execute(
                "UPDATE photos SET pair_group_id = ?1 WHERE id = ?2",
                rusqlite::params![group_id, id],
            )?;
        }
    }
    Ok(())
}

fn resolve_capture_time(ex: &exif::ExifData, item: &ScanItem) -> String {
    if let Some(t) = &ex.capture_time {
        return t.clone();
    }
    item.modified
        .and_then(|t| {
            chrono::DateTime::<chrono::Local>::from(t)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
                .into()
        })
        .unwrap_or_else(|| "1900-01-01 00:00:00".to_string())
}

pub fn human_bytes(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}
