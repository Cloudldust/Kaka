//! Export of kept photos (PRD 12.1–12.4) and Lightroom 联动 (PRD 13).
//!
//! Only photos with status != `Delete` are exported as "kept". Export never
//! modifies source files or the database.

use crate::app::copy::{OrgMode, atomic_copy};
use crate::db::{self, Db};
use crate::model::{PhotoListItem, SortOrder, Status};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Outcome of a copy-export (PRD 12.1).
#[derive(Debug, Default)]
pub struct ExportOutcome {
    pub copied: usize,
    pub failed: usize,
    pub failures: Vec<String>,
    pub total: usize,
}

/// File-list export format (PRD 12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFileFormat {
    Txt,
    Csv,
}

/// Progress callback for export: (done, total) -> continue?
pub type ExportProgress<'a> = &'a mut dyn FnMut(usize, usize) -> bool;

/// 12.1: copy every kept photo (status != Delete) into `target_dir`, organized
/// by `org`. RAW+JPG pairs are copied together. Optionally copies the original
/// sidecar and writes a rotation XMP sidecar.
pub fn export_kept_copy(
    db: &Db,
    folder: &str,
    target_dir: &str,
    org: OrgMode,
    copy_sidecar: bool,
    write_rotation_xmp: bool,
    progress: ExportProgress,
) -> anyhow::Result<ExportOutcome> {
    if target_dir.trim().is_empty() {
        anyhow::bail!("未设置导出目录");
    }
    std::fs::create_dir_all(target_dir)?;
    let root = PathBuf::from(folder);
    let target_root = PathBuf::from(target_dir);

    let items = db::photos::list_items_in_folder(db, folder, SortOrder::CaptureTimeAsc)?;
    let kept: Vec<PhotoListItem> = items
        .into_iter()
        .filter(|p| p.status != Status::Delete)
        .collect();

    // Expand RAW+JPG pair groups so both members are exported together.
    let mut paths: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for p in &kept {
        if let Some(g) = p.pair_group_id {
            for q in &kept {
                if q.pair_group_id == Some(g) && seen.insert(q.current_path.clone()) {
                    paths.push(q.current_path.clone());
                }
            }
        } else if seen.insert(p.current_path.clone()) {
            paths.push(p.current_path.clone());
        }
    }

    // Resolve destination paths (flat mode dedups with _dupN).
    let mut used: HashMap<(String, String), usize> = HashMap::new();
    let mut files: Vec<(PathBuf, PathBuf)> = Vec::new();
    for p in &paths {
        let src = PathBuf::from(p);
        let filename = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dir = match org {
            OrgMode::Structure => {
                let rel = src
                    .strip_prefix(&root)
                    .unwrap_or(src.as_path())
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default();
                target_root.join(&rel)
            }
            OrgMode::Date => {
                let ct = kept
                    .iter()
                    .find(|k| k.current_path == *p)
                    .map(|k| k.capture_time.clone())
                    .unwrap_or_default();
                target_root.join(crate::app::copy::capture_date(&ct))
            }
            OrgMode::Flat => target_root.clone(),
        };
        let key = (dir.to_string_lossy().into_owned(), filename.clone());
        let n = used.entry(key).or_insert(0);
        let final_name = if *n == 0 {
            filename.clone()
        } else {
            let stem = src
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("img");
            let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext.is_empty() {
                format!("{stem}_dup{n}")
            } else {
                format!("{stem}_dup{n}.{ext}")
            }
        };
        *n += 1;
        files.push((src, dir.join(final_name)));
    }

    let total = files.len();
    let mut outcome = ExportOutcome {
        total,
        ..Default::default()
    };
    // Map current_path -> rotation_override (only when writing rotation XMP).
    let mut rotations: HashMap<String, i64> = HashMap::new();
    if write_rotation_xmp {
        for p in &kept {
            if let Ok(Some(full)) = db::photos::get_photo(db, p.id) {
                rotations.insert(full.current_path.clone(), full.rotation_override);
            }
        }
    }
    let mut done = 0usize;
    for (src, dest) in &files {
        if !progress(done, total) {
            // Cancelled.
            outcome.failed = total.saturating_sub(done);
            return Ok(outcome);
        }
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if atomic_copy(src, dest).is_ok() {
            outcome.copied += 1;
            // Copy the original sidecar (if any).
            if copy_sidecar {
                if let Some(side) = sidecar_for(src) {
                    let side_target = dest.with_extension("xmp");
                    let _ = atomic_copy(&side, &side_target);
                }
            }
            // Write a rotation XMP sidecar when a manual rotation is set.
            if write_rotation_xmp {
                if let Some(&rot) = rotations.get(&src.to_string_lossy().into_owned()) {
                    if rot != 0 {
                        let _ = write_xmp_to(dest, "Kaka:Keep", 3, rot);
                    }
                }
            }
        } else {
            outcome.failed += 1;
            outcome.failures.push(src.display().to_string());
        }
        done += 1;
    }
    Ok(outcome)
}

/// 12.2: write the kept list to a .txt (one absolute path per line) or .csv file.
/// Returns how many photos were written. Both use UTF-8 BOM; CSV has a header.
pub fn export_file_list(
    db: &Db,
    folder: &str,
    out_path: &str,
    format: ExportFileFormat,
) -> anyhow::Result<usize> {
    let items = db::photos::list_items_in_folder(db, folder, SortOrder::CaptureTimeAsc)?;
    let kept: Vec<PhotoListItem> = items
        .into_iter()
        .filter(|p| p.status != Status::Delete)
        .collect();

    let mut text = String::from("\u{feff}"); // UTF-8 BOM
    match format {
        ExportFileFormat::Txt => {
            for p in &kept {
                text.push_str(&p.current_path);
                text.push_str("\r\n");
            }
        }
        ExportFileFormat::Csv => {
            text.push_str("original_filename,current_path,status\r\n");
            for p in &kept {
                let status = match p.status {
                    Status::Reviewed => "2",
                    _ => "0",
                };
                text.push_str(&format!("{},{},{}\r\n", p.original_filename, p.current_path, status));
            }
        }
    }
    std::fs::write(out_path, text)?;
    Ok(kept.len())
}

/// 12.3: write/modify an XMP sidecar for each kept photo marking it "Kaka:Keep"
/// with the given rating and (optionally) rotation. This is a first-cut that
/// replaces the sidecar with a minimal standard XMP; it does not yet merge with
/// existing XMP fields.
pub fn write_xmp_sidecars(db: &Db, folder: &str, rating: u8) -> anyhow::Result<usize> {
    let items = db::photos::list_items_in_folder(db, folder, SortOrder::CaptureTimeAsc)?;
    let kept: Vec<PhotoListItem> = items
        .into_iter()
        .filter(|p| p.status != Status::Delete)
        .collect();
    let mut n = 0usize;
    for p in &kept {
        if let Ok(Some(full)) = db::photos::get_photo(db, p.id) {
            let dest = PathBuf::from(&p.current_path).with_extension("xmp");
            if write_xmp_to(&dest, "Kaka:Keep", rating, full.rotation_override as i64).is_ok() {
                n += 1;
            }
        }
    }
    Ok(n)
}

fn sidecar_for(path: &Path) -> Option<PathBuf> {
    for ext in ["xmp", "dop", "pp3"] {
        let c = path.with_extension(ext);
        if c.exists() {
            return Some(c);
        }
    }
    None
}

/// Write a minimal sidecar XMP to `dest` with the given label, rating and
/// orientation. Creates the parent dir if needed.
fn write_xmp_to(dest: &Path, label: &str, rating: u8, orientation: i64) -> anyhow::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let orient = if orientation != 0 {
        format!("\n   <tiff:Orientation>{orientation}</tiff:Orientation>")
    } else {
        String::new()
    };
    let xml = format!(
        r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
     xmlns:dc="http://purl.org/dc/elements/1.1/"
     xmlns:xmp="http://ns.adobe.com/xap/1.0/"
     xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
     xmlns:tiff="http://ns.adobe.com/tiff/1.0/">
   <dc:subject><rdf:Bag><rdf:li>{label}</rdf:li></rdf:Bag></dc:subject>
   <xmp:Label>{label}</xmp:Label>
   <crs:Rating>{rating}</crs:Rating>{orient}
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#,
        label = label,
        rating = rating,
        orient = orient,
    );
    std::fs::write(dest, xml)?;
    Ok(())
}

// ---- Lightroom 联动 (PRD 13) ----

/// Detect a Lightroom Classic install. If `custom` is a non-empty path the user
/// configured in settings, prefer it (a directory gets `Lightroom.exe` appended);
/// otherwise checks known install paths, then the registry. Returns the path to
/// Lightroom.exe, or None.
pub fn lr_install_path(custom: &str) -> Option<PathBuf> {
    if !custom.trim().is_empty() {
        let p = PathBuf::from(custom.trim());
        if p.is_file() {
            return Some(p);
        }
        if p.is_dir() {
            let exe = p.join("Lightroom.exe");
            if exe.exists() {
                return Some(exe);
            }
        }
        // Fall through to auto-detect if the custom path is invalid.
    }
    for p in [
        r"C:\Program Files\Adobe\Adobe Lightroom Classic\Lightroom.exe",
        r"C:\Program Files\Adobe\Lightroom Classic\Lightroom.exe",
    ] {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    // Registry: HKLM\SOFTWARE\Adobe\Lightroom\CurrentVersion\InstallPath.
    if let Ok(out) = std::process::Command::new("reg")
        .args(["query", r"HKLM\SOFTWARE\Adobe\Lightroom\CurrentVersion", "/v", "InstallPath"])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if let Some(eq) = line.find("REG_SZ") {
                let dir = line[eq..].replace("REG_SZ", "").trim().to_string();
                if !dir.is_empty() {
                    let exe = PathBuf::from(dir).join("Lightroom.exe");
                    if exe.exists() {
                        return Some(exe);
                    }
                }
            }
        }
    }
    None
}

/// 12.4 / 13.1: launch Lightroom Classic with every kept photo path as an import
/// argument, and write a temporary .lrtemplate placeholder file listing the kept
/// paths. Returns the number of photos sent.
pub fn send_to_lightroom(db: &Db, folder: &str, lr_exe: &Path) -> anyhow::Result<usize> {
    let items = db::photos::list_items_in_folder(db, folder, SortOrder::CaptureTimeAsc)?;
    let kept: Vec<PhotoListItem> = items
        .into_iter()
        .filter(|p| p.status != Status::Delete)
        .collect();

    if kept.is_empty() {
        anyhow::bail!("工作区没有保留照片");
    }

    // Write the .lrtemplate placeholder list.
    let tmp = std::env::temp_dir().join("kaka_lr_import.lrtemplate");
    let mut list = String::from("\u{feff}"); // BOM
    for p in &kept {
        list.push_str(&p.current_path);
        list.push_str("\r\n");
    }
    std::fs::write(&tmp, list)?;

    // Launch LR with each kept path as a command-line import argument.
    let mut cmd = std::process::Command::new(lr_exe);
    for p in &kept {
        cmd.arg(&p.current_path);
    }
    cmd.spawn()?;
    Ok(kept.len())
}
