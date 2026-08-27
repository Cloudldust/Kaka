//! Shared data model types used across the db, io and app layers.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Default DPI scale = 1.0 (100%). All UI dimensions use this as the base
/// reference value and are multiplied by the actual display DPI scale.
pub const BASE_DPI: f32 = 96.0;

/// Photo processing status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i64)]
pub enum Status {
    /// 未处理 / untreated
    Untreated = 0,
    /// 待删 / marked for deletion
    Delete = 1,
    /// 已阅跳过 / reviewed & skip
    Reviewed = 2,
}

impl Status {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => Status::Delete,
            2 => Status::Reviewed,
            _ => Status::Untreated,
        }
    }
    pub fn as_i64(self) -> i64 {
        self as i64
    }
}

/// Sort order for the current workspace view (PRD 7.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortOrder {
    CaptureTimeAsc,
    CaptureTimeDesc,
    FilenameAsc,
    FilenameDesc,
    FileSizeAsc,
    FileSizeDesc,
    ImportTimeAsc,
    ImportTimeDesc,
    StatusGrouped,
}

impl SortOrder {
    pub fn from_code(code: &str) -> Self {
        match code {
            "capture_time_desc" => SortOrder::CaptureTimeDesc,
            "filename_asc" => SortOrder::FilenameAsc,
            "filename_desc" => SortOrder::FilenameDesc,
            "file_size_asc" => SortOrder::FileSizeAsc,
            "file_size_desc" => SortOrder::FileSizeDesc,
            "import_time_asc" => SortOrder::ImportTimeAsc,
            "import_time_desc" => SortOrder::ImportTimeDesc,
            "status_grouped" => SortOrder::StatusGrouped,
            _ => SortOrder::CaptureTimeAsc,
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            SortOrder::CaptureTimeAsc => "capture_time_asc",
            SortOrder::CaptureTimeDesc => "capture_time_desc",
            SortOrder::FilenameAsc => "filename_asc",
            SortOrder::FilenameDesc => "filename_desc",
            SortOrder::FileSizeAsc => "file_size_asc",
            SortOrder::FileSizeDesc => "file_size_desc",
            SortOrder::ImportTimeAsc => "import_time_asc",
            SortOrder::ImportTimeDesc => "import_time_desc",
            SortOrder::StatusGrouped => "status_grouped",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SortOrder::CaptureTimeAsc => "拍摄时间 ↑",
            SortOrder::CaptureTimeDesc => "拍摄时间 ↓",
            SortOrder::FilenameAsc => "文件名 A-Z",
            SortOrder::FilenameDesc => "文件名 Z-A",
            SortOrder::FileSizeAsc => "文件大小 小→大",
            SortOrder::FileSizeDesc => "文件大小 大→小",
            SortOrder::ImportTimeAsc => "导入时间 ↑",
            SortOrder::ImportTimeDesc => "导入时间 ↓",
            SortOrder::StatusGrouped => "状态分组",
        }
    }
}

/// The capture-time source, for logging the mtime fallback (PRD 6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CaptureTimeSource {
    #[default]
    ExifOriginal,
    ExifDigitized,
    MtimeFallback,
}

impl CaptureTimeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureTimeSource::ExifOriginal => "exif_original",
            CaptureTimeSource::ExifDigitized => "exif_digitized",
            CaptureTimeSource::MtimeFallback => "mtime_fallback",
        }
    }
}

/// A photo record (maps to the `photos` table).
#[derive(Debug, Clone)]
pub struct Photo {
    pub id: i64,
    pub original_filename: String,
    pub file_size: i64,
    pub capture_time: String,
    pub current_path: String,
    pub folder_path: String,
    pub status: Status,
    pub thumb_hash: Option<String>,
    pub decode_failed: bool,
    pub preview_only: bool,
    pub rotation_override: i64,
    pub exif_orientation: i64,
    pub pair_group_id: Option<i64>,
    pub iso: Option<i64>,
    pub aperture: Option<String>,
    pub shutter_speed: Option<String>,
    pub focal_length: Option<i64>,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    pub capture_time_source: String,
    pub import_time: String,
    pub last_access_time: String,
    pub marked_delete_time: Option<String>,
    pub marked_review_time: Option<String>,
}

/// A folder record (maps to the `folders` table).
#[derive(Debug, Clone)]
pub struct Folder {
    pub id: i64,
    pub folder_path: String,
    pub display_name: Option<String>,
    pub notes: Option<String>,
    pub first_import_time: String,
    pub last_open_time: Option<String>,
    pub recursive_show: bool,
}

/// Workspace state record (maps to `workspace_state`, id=1 single row).
#[derive(Debug, Clone)]
pub struct WorkspaceState {
    pub current_folder_path: Option<String>,
    pub current_index: i64,
    pub current_sort: String,
    pub filter_json: Option<String>,
    pub last_selected_id: Option<i64>,
    pub last_save_time: String,
    pub last_crash_marker: bool,
    pub recent_folders_json: Option<String>,
}

/// A compact photo view used for the thumbnail strip / preview list.
#[derive(Debug, Clone)]
pub struct PhotoListItem {
    pub id: i64,
    pub original_filename: String,
    pub current_path: String,
    pub folder_path: String,
    pub status: Status,
    pub capture_time: String,
    pub file_size: i64,
    pub thumb_hash: Option<String>,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    pub iso: Option<i64>,
    pub aperture: Option<String>,
    pub shutter_speed: Option<String>,
    pub focal_length: Option<i64>,
    pub decode_failed: bool,
    pub preview_only: bool,
    pub pair_group_id: Option<i64>,
}

impl PhotoListItem {
    /// True if the underlying file no longer exists on disk.
    pub fn is_missing(&self) -> bool {
        let p = PathBuf::from(&self.current_path);
        !p.exists()
    }
}

/// Advanced filter conditions (PRD 7.8). All conditions are AND-combined.
/// Empty enum collections mean "no restriction" for that field.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    /// Status ids (0=untreated,1=delete,2=reviewed); empty = all.
    pub statuses: Vec<i64>,
    /// Camera model values; empty = all.
    pub cameras: Vec<String>,
    /// Lens model values; empty = all.
    pub lenses: Vec<String>,
    pub iso_min: Option<i64>,
    pub iso_max: Option<i64>,
    pub focal_min: Option<i64>,
    pub focal_max: Option<i64>,
    /// Date range (inclusive) as YYYY-MM-DD; None = no bound.
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    /// File formats (extensions, uppercased e.g. "NEF"); empty = all.
    pub formats: Vec<String>,
    /// None = all; Some(true) = missing files only; Some(false) = existing only.
    pub missing: Option<bool>,
    /// None = all; Some(true) = paired RAW+JPG only; Some(false) = single only.
    pub pair: Option<bool>,
}

impl Filter {
    /// True when at least one condition is active.
    pub fn is_active(&self) -> bool {
        !self.statuses.is_empty()
            || !self.cameras.is_empty()
            || !self.lenses.is_empty()
            || self.iso_min.is_some()
            || self.iso_max.is_some()
            || self.focal_min.is_some()
            || self.focal_max.is_some()
            || self.date_from.is_some()
            || self.date_to.is_some()
            || !self.formats.is_empty()
            || self.missing.is_some()
            || self.pair.is_some()
    }
}

/// Settings persisted to %APPDATA%/Kaka/config.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub github_repo: String,
    pub auto_detect_card: bool,
    pub auto_open_last_workspace: bool,
    pub show_clipping_warning: bool,
    pub dim_reviewed_thumbnails: bool,
    pub batch_confirm: bool,
    pub wrap_at_end: bool,
    pub default_target_dir: String,
    pub cache_dir: String,
    pub cache_capacity_gb: u64,
    pub cache_expire_days: u64,
    pub high_dpi_2x: bool,
    pub star_rating: u8,
    pub include_sidecar_export: bool,
    pub export_space_guard: bool,
    /// User-specified Lightroom Classic install path (empty = auto-detect).
    pub lr_install_path: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        let cache_default = dirs::cache_dir()
            .unwrap_or_else(|| std::env::temp_dir())
            .join("Kaka")
            .join("cache")
            .to_string_lossy()
            .into_owned();
        AppConfig {
            github_repo: "https://github.com/kaka-rs/kaka".to_string(),
            auto_detect_card: true,
            auto_open_last_workspace: true,
            show_clipping_warning: true,
            dim_reviewed_thumbnails: true,
            batch_confirm: true,
            wrap_at_end: false,
            default_target_dir: String::new(),
            cache_dir: cache_default,
            cache_capacity_gb: 20,
            cache_expire_days: 30,
            high_dpi_2x: true,
            star_rating: 3,
            include_sidecar_export: true,
            export_space_guard: true,
            lr_install_path: String::new(),
        }
    }
}
