//! File-system traversal and supported-file collection (PRD 6.1.1).

use super::format::{self, Classification, FormatKind};
use std::path::{Path, PathBuf};

/// A single file discovered during scanning, before any database work.
#[derive(Debug, Clone)]
pub struct ScanItem {
    pub path: PathBuf,
    pub file_size: i64,
    pub modified: Option<std::time::SystemTime>,
    pub kind: FormatKind,
    pub filename: String,
    /// File name without extension (used for RAW+JPG pairing).
    pub stem: String,
    /// True when a same-stem sidecar (.xmp) exists beside this file.
    pub has_sidecar: bool,
}

/// Options controlling a scan.
#[derive(Debug, Clone, Copy)]
pub struct ScanOptions {
    pub recursive: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions { recursive: true }
    }
}

/// Recursively (or not) collect supported photo files under `root`.
///
/// Returns a sorted list (by path) plus a count of recognized photo files.
/// Unsupported / hidden files and sidecars are filtered out here; sidecars are
/// only used to annotate `has_sidecar`.
pub fn scan_folder(root: &Path, opts: ScanOptions) -> std::io::Result<Vec<ScanItem>> {
    let mut items = Vec::new();
    scan_into(root, root, opts.recursive, &mut items)?;
    items.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(items)
}

fn scan_into(
    root: &Path,
    dir: &Path,
    recursive: bool,
    out: &mut Vec<ScanItem>,
) -> std::io::Result<()> {
    let entries = std::fs::read_dir(dir)?;
    // First pass: collect accepted photo files and note sidecar stems.
    let mut sidecar_stems: Vec<String> = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            // Skip hidden/system dirs (e.g. AppData, node_modules, .git).
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.starts_with('.') || name == "system volume information" || name == "$recycle.bin"
            {
                continue;
            }
            if recursive {
                scan_into(root, &path, recursive, out)?;
            }
            continue;
        }
        match format::classify(&path) {
            Classification::Photo(kind) => {
                paths.push(path.clone());
                if kind == FormatKind::Raw {
                    // Raw handled below; nothing special here.
                }
            }
            Classification::Sidecar => {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    sidecar_stems.push(stem.to_string());
                }
            }
            _ => {}
        }
    }

    for path in paths {
        if let Ok(meta) = std::fs::metadata(&path) {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            let kind = match format::classify(&path) {
                Classification::Photo(k) => k,
                _ => continue,
            };
            out.push(ScanItem {
                filename: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                stem: stem.clone(),
                path,
                file_size: meta.len() as i64,
                modified: meta.modified().ok(),
                kind,
                has_sidecar: sidecar_stems.contains(&stem),
            });
        }
    }
    Ok(())
}
