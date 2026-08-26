//! Thumbnail generation and disk cache (PRD 9.1, 9.2 — M1 subset).
//!
//! M1 generates JPEG thumbs for JPEG/PNG/TIFF sources. RAW and HEIF decoding
//! are deferred to M2+ and simply produce no cache entry (placeholder shown).

use crate::paths;
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};

/// Compute the stable thumbnail hash for a photo. SHA-1 of the photo's path +
/// size + capture time, trimmed to the first 16 bytes, hex-encoded (PRD 10.2).
pub fn thumb_hash_for(photo_path: &str, file_size: i64, capture_time: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(photo_path.as_bytes());
    hasher.update(b"|");
    hasher.update(file_size.to_string().as_bytes());
    hasher.update(b"|");
    hasher.update(capture_time.as_bytes());
    let digest = hasher.finalize();
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// Short-edge length for the base thumbnail at DPI scale 1.0 (PRD 9.1).
pub const THUMB_SHORT_EDGE: u32 = 256;
pub const THUMB_QUALITY: u8 = 80;

/// Long-edge length for the preview image at DPI scale 1.0 (PRD 9.1).
pub const PREVIEW_LONG_EDGE: u32 = 1920;
pub const PREVIEW_QUALITY: u8 = 90;

/// Resolve the on-disk path for a thumbnail hash (base @1x version).
pub fn thumb_path(hash: &str, dpi_scale: f32) -> PathBuf {
    let dir = paths::thumbs_dir();
    if dpi_scale >= 1.5 {
        dir.join(format!("{hash}@2x.jpg"))
    } else {
        dir.join(format!("{hash}.jpg"))
    }
}

/// Resolve the on-disk path for a preview cached image.
pub fn preview_path(hash: &str) -> PathBuf {
    paths::previews_dir().join(format!("{hash}_preview.jpg"))
}

/// Ensure a preview image exists for the given photo (long edge <= 1920, Q90).
/// Returns the on-disk path, or None if the source is not decodable (raw).
pub fn ensure_preview(src: &Path, hash: &str) -> anyhow::Result<Option<PathBuf>> {
    let dest = preview_path(hash);
    if dest.exists() {
        return Ok(Some(dest));
    }
    let ok = generate_preview(src, &dest, PREVIEW_LONG_EDGE, PREVIEW_QUALITY)?;
    Ok(if ok { Some(dest) } else { None })
}

/// Generate a long-edge-bounded preview JPEG from `src` into `dest`.
pub fn generate_preview(
    src: &Path,
    dest: &Path,
    max_long_edge: u32,
    quality: u8,
) -> anyhow::Result<bool> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let img = match decode_source(src)? {
        Some(img) => img,
        None => return Ok(false), // not decodable (e.g. RAW w/o preview)
    };
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    if w == 0 || h == 0 {
        return Ok(false);
    }
    let long = w.max(h);
    let small =
        if long > max_long_edge {
            let scale = max_long_edge as f64 / long as f64;
            let nw = ((w as f64) * scale).round().max(1.0) as u32;
            let nh = ((h as f64) * scale).round().max(1.0) as u32;
            image::imageops::resize(&rgb, nw, nh, image::imageops::FilterType::Lanczos3)
        } else {
            rgb
        };
    let file = std::fs::File::create(dest)?;
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(file, quality);
    enc.encode_image(&small)?;
    Ok(true)
}

/// True if a cached thumb file already exists on disk for this hash.
pub fn thumb_exists(hash: &str, dpi_scale: f32) -> bool {
    thumb_path(hash, dpi_scale).exists()
}

/// Open an image source for thumbnail/preview generation. For formats the
/// `image` crate cannot decode (RAW), fall back to the embedded JPEG preview.
fn decode_source(src: &Path) -> anyhow::Result<Option<image::DynamicImage>> {
    let mut img = if let Ok(img) = image::open(src) {
        img
    } else if let Some(bytes) = crate::io::exif::extract_embedded_preview(src) {
        match image::load_from_memory(&bytes) {
            Ok(img) => img,
            Err(_) => return Ok(None),
        }
    } else if let Some(img) = scan_embedded_preview(src) {
        img
    } else if let Ok(img) = rawler::analyze::extract_preview_pixels(
        src,
        &rawler::decoders::RawDecodeParams::default(),
    ) {
        img
    } else {
        return Ok(None);
    };
    // Apply the source file's EXIF orientation so portrait images display
    // upright instead of being rotated 90° sideways.
    let orient = crate::io::exif::parse_exif(src).orientation.unwrap_or(1) as u8;
    if let Some(o) = image::metadata::Orientation::from_exif(orient) {
        img.apply_orientation(o);
    }
    Ok(Some(img))
}

/// Locate an embedded medium-size preview JPEG inside a file by scanning for
/// JPEG SOI markers. Returns the best candidate: a "preview-sized" image
/// (long edge 1200–2800) is returned immediately; otherwise the candidate
/// whose long edge is closest to 1920 among those >= 1200.
fn scan_embedded_preview(src: &Path) -> Option<image::DynamicImage> {
    let data = std::fs::read(src).ok()?;
    let mut i = 0usize;
    let mut best: Option<(u32, image::DynamicImage)> = None;
    while i + 2 < data.len() {
        if data[i] == 0xFF && data[i + 1] == 0xD8 && data[i + 2] == 0xFF {
            if let Ok(img) = image::load_from_memory(&data[i..]) {
                let long = img.width().max(img.height());
                if (1200..=2800).contains(&long) {
                    return Some(img);
                }
                if long >= 1200 {
                    let closer = best
                        .as_ref()
                        .map(|(bl, _)| (long as i64 - 1920).abs() < (*bl as i64 - 1920).abs())
                        .unwrap_or(true);
                    if closer {
                        best = Some((long, img));
                    }
                }
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    best.map(|(_, img)| img)
}

/// Generate a thumbnail from `src` into `dest` (JPEG, quality 80, short edge
/// `short_edge`). Returns Ok(true) on success, Ok(false) if the source is not a
/// decodable image, or an error.
pub fn generate_thumbnail(
    src: &Path,
    dest: &Path,
    short_edge: u32,
    quality: u8,
) -> anyhow::Result<bool> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let img = match decode_source(src)? {
        Some(img) => img,
        None => return Ok(false), // not a decodable image (e.g. RAW w/o preview)
    };
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    if w == 0 || h == 0 {
        return Ok(false);
    }
    let target = if w < h { w.min(short_edge) } else { h.min(short_edge) };
    // Guard against upscaling tiny images.
    let target = target.max(1);
    let thumb = if w > target || h > target {
        let scale = (target as f64) / (w.min(h) as f64);
        let nw = ((w as f64) * scale).round().max(1.0) as u32;
        let nh = ((h as f64) * scale).round().max(1.0) as u32;
        image::imageops::resize(&rgb, nw, nh, image::imageops::FilterType::Lanczos3)
    } else {
        rgb
    };
    // Use the `image` crate's JPEG encoder via its save_with_format.
    let file = std::fs::File::create(dest)?;
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(file, quality);
    enc.encode_image(&thumb)?;
    Ok(true)
}

/// Ensure a thumbnail exists for the given photo metadata. Returns the on-disk
/// path of the cached thumb used (respecting DPI).
///
/// Returns Ok(None) when the source cannot be decoded (raw preview deferred).
pub fn ensure_thumbnail(
    src: &Path,
    hash: &str,
    dpi_scale: f32,
) -> anyhow::Result<Option<PathBuf>> {
    let dest = thumb_path(hash, dpi_scale);
    if dest.exists() {
        return Ok(Some(dest));
    }
    let ok = generate_thumbnail(src, &dest, THUMB_SHORT_EDGE, THUMB_QUALITY)?;
    if ok {
        Ok(Some(dest))
    } else {
        Ok(None)
    }
}

/// Decode the source ONCE and write both the thumbnail and the preview cache
/// files. Avoids decoding a RAW twice when the import/worker needs both.
/// Returns true if at least the thumbnail was written, false when the source
/// is not decodable.
pub fn generate_caches(src: &Path, hash: &str, dpi_scale: f32) -> anyhow::Result<bool> {
    let thumb_dest = thumb_path(hash, dpi_scale);
    let prev_dest = preview_path(hash);
    if thumb_dest.exists() && prev_dest.exists() {
        return Ok(true);
    }
    let img = match decode_source(src)? {
        Some(img) => img,
        None => return Ok(false),
    };
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    if w == 0 || h == 0 {
        return Ok(false);
    }
    let mut wrote = false;

    // Thumbnail (short edge <= 256, Q80).
    if !thumb_dest.exists() {
        let target = w.min(h).min(THUMB_SHORT_EDGE).max(1);
        let thumb = if w > target || h > target {
            let scale = (target as f64) / (w.min(h) as f64);
            let nw = ((w as f64) * scale).round().max(1.0) as u32;
            let nh = ((h as f64) * scale).round().max(1.0) as u32;
            image::imageops::resize(&rgb, nw, nh, image::imageops::FilterType::Lanczos3)
        } else {
            rgb.clone()
        };
        if let Some(dir) = thumb_dest.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::File::create(&thumb_dest)?;
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(file, THUMB_QUALITY);
        enc.encode_image(&thumb)?;
        wrote = true;
    }

    // Preview (long edge <= 1920, Q90; no upscaling).
    if !prev_dest.exists() {
        let long = w.max(h);
        let small = if long > PREVIEW_LONG_EDGE {
            let scale = PREVIEW_LONG_EDGE as f64 / long as f64;
            let nw = ((w as f64) * scale).round().max(1.0) as u32;
            let nh = ((h as f64) * scale).round().max(1.0) as u32;
            image::imageops::resize(&rgb, nw, nh, image::imageops::FilterType::Lanczos3)
        } else {
            rgb
        };
        if let Some(dir) = prev_dest.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::File::create(&prev_dest)?;
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(file, PREVIEW_QUALITY);
        enc.encode_image(&small)?;
        wrote = true;
    }

    Ok(wrote || (thumb_dest.exists() && prev_dest.exists()))
}
