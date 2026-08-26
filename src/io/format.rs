//! Supported / unsupported format classification (PRD 2.1, 2.2).

use std::path::Path;

/// Raw (manufacturer-proprietary) extension set, lowercased.
const RAW_EXTS: &[&str] = &[
    "nef", "nrw", "cr2", "cr3", "arw", "srf", "sr2", "raf", "pef", "ptx", "orf", "rw2", "dng",
    "raw", "raws", "iiq", "3fr", "x3f",
];

/// Non-raw image extension set we can handle in M1 (JPEG / PNG / TIFF).
/// HEIC/HEIF are listed so the scanner reports them gracefully as preview-only
/// candidates, but actual decode may be unavailable.
const JPG_EXTS: &[&str] = &["jpg", "jpeg", "jfif"];
const PNG_EXTS: &[&str] = &["png"];
const TIF_EXTS: &[&str] = &["tif", "tiff"];
const HEIF_EXTS: &[&str] = &["heic", "heif"];

/// Sidecar / non-photo file extensions that must never be imported.
const SIDECAR_EXTS: &[&str] = &["xmp", "dop", "pp3"];
const VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "avi", "mkv", "mts", "m2ts", "wmv", "flv", "3gp", "m4v", "webm",
];
const AUDIO_EXTS: &[&str] = &["mp3", "wav", "flac", "aac", "ogg", "m4a", "wma"];
const DOC_EXTS: &[&str] = &[
    "doc", "docx", "pdf", "xls", "xlsx", "ppt", "pptx", "txt", "md", "csv", "zip", "rar",
];

/// System hidden files that should be filtered out entirely.
const HIDDEN_NAMES: &[&str] = &["thumbs.db", ".ds_store", "desktop.ini", ".thumbnails"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatKind {
    /// Manufacturer RAW (may or may not be decodable; M1 treats as preview-only).
    Raw,
    /// JPEG.
    Jpeg,
    /// PNG.
    Png,
    /// TIFF.
    Tiff,
    /// HEIC/HEIF (depends on system codec).
    Heif,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    /// A supported photo to import.
    Photo(FormatKind),
    /// A sidecar (xmp/dop/pp3) that should follow its photo but not be imported.
    Sidecar,
    /// Explicitly unsupported (video/audio/doc) — filtered out, not reported.
    Unsupported,
    /// A hidden system file — filtered out.
    Hidden,
}

/// Get the lowercased extension of a path without the dot.
pub fn ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Classify a file path for scanning.
pub fn classify(path: &Path) -> Classification {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if HIDDEN_NAMES.contains(&name.as_str()) {
        return Classification::Hidden;
    }
    let e = ext(path);
    if SIDECAR_EXTS.contains(&e.as_str()) {
        return Classification::Sidecar;
    }
    if VIDEO_EXTS.contains(&e.as_str())
        || AUDIO_EXTS.contains(&e.as_str())
        || DOC_EXTS.contains(&e.as_str())
    {
        return Classification::Unsupported;
    }
    if RAW_EXTS.contains(&e.as_str()) {
        return Classification::Photo(FormatKind::Raw);
    }
    if JPG_EXTS.contains(&e.as_str()) {
        return Classification::Photo(FormatKind::Jpeg);
    }
    if PNG_EXTS.contains(&e.as_str()) {
        return Classification::Photo(FormatKind::Png);
    }
    if TIF_EXTS.contains(&e.as_str()) {
        return Classification::Photo(FormatKind::Tiff);
    }
    if HEIF_EXTS.contains(&e.as_str()) {
        return Classification::Photo(FormatKind::Heif);
    }
    Classification::Unsupported
}

/// True if the path represents an importable photo.
pub fn is_photo(path: &Path) -> bool {
    matches!(classify(path), Classification::Photo(_))
}

/// True if the path is a RAW file.
pub fn is_raw(path: &Path) -> bool {
    matches!(classify(path), Classification::Photo(FormatKind::Raw))
}

/// True if the path is a JPEG.
pub fn is_jpeg(path: &Path) -> bool {
    matches!(classify(path), Classification::Photo(FormatKind::Jpeg))
}

/// True if the image is fully decodable by the `image` crate on this build
/// (i.e. JPEG / PNG / TIFF). RAW / HEIF are handled by preview-only fallback
/// in M1.
pub fn is_decodable(kind: FormatKind) -> bool {
    matches!(kind, FormatKind::Jpeg | FormatKind::Png | FormatKind::Tiff)
}
