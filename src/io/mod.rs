//! System IO layer: file scanning, format filtering, EXIF, thumbnails,
//! disk-cache index/cleanup, recycle-bin shell calls.

pub mod cache_clean;
pub mod cache_index;
pub mod exif;
pub mod format;
pub mod histogram;
pub mod recycle;
pub mod scanner;
pub mod thumbnails;
