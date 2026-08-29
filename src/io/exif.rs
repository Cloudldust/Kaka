//! EXIF extraction (PRD 6.4) with multi-encoding fallback decoding.
//!
//! M1 focuses on JPEG metadata; RAW parsing is deferred to M2.

use crate::model::CaptureTimeSource;
use exif::{In, Reader, Tag, Value};
use std::io::{Read, SeekFrom};
use std::path::Path;

/// Extracted photograph metadata, all optional except when unavailable.
#[derive(Debug, Clone, Default)]
pub struct ExifData {
    pub capture_time: Option<String>,
    pub capture_time_source: CaptureTimeSource,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    pub iso: Option<i64>,
    pub aperture: Option<String>,
    pub shutter_speed: Option<String>,
    pub focal_length: Option<i64>,
    pub orientation: Option<i64>,
    pub has_exif: bool,
}

/// Parse EXIF from an image file. Never returns an error for a missing/partial
/// EXIF block; on total failure it returns a default `ExifData`.
pub fn parse_exif(path: &Path) -> ExifData {
    let mut data = ExifData::default();
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return data,
    };
    let mut reader = std::io::BufReader::new(file);
    let exif = match Reader::new().read_from_container(&mut reader) {
        Ok(e) => e,
        Err(_) => {
            // No EXIF: fall back to a later mtime check by caller.
            return data;
        }
    };
    data.has_exif = true;

    // Capture time: DateTimeOriginal > DateTimeDigitized > (caller mtime fallback).
    if let Some(v) = exif.get_field(Tag::DateTimeOriginal, In::PRIMARY) {
        if let Some(t) = parse_datetime_field(v) {
            data.capture_time = Some(t);
            data.capture_time_source = CaptureTimeSource::ExifOriginal;
        }
    }
    if data.capture_time.is_none() {
        if let Some(v) = exif.get_field(Tag::DateTimeDigitized, In::PRIMARY) {
            if let Some(t) = parse_datetime_field(v) {
                data.capture_time = Some(t);
                data.capture_time_source = CaptureTimeSource::ExifDigitized;
            }
        }
    }

    data.camera_model = get_string_field(&exif, Tag::Model);
    data.lens_model = get_string_field(&exif, Tag::LensModel);

    data.iso = exif
        .get_field(Tag::PhotographicSensitivity, In::PRIMARY)
        .and_then(short_value);

    data.aperture = exif
        .get_field(Tag::FNumber, In::PRIMARY)
        .and_then(rational_value)
        .map(|v| format!("f/{}", trim_float(v)));

    data.shutter_speed = exif
        .get_field(Tag::ExposureTime, In::PRIMARY)
        .and_then(exposure_fraction);

    data.focal_length = exif
        .get_field(Tag::FocalLength, In::PRIMARY)
        .and_then(rational_value)
        .map(|v| v.round() as i64);

    data.orientation = exif
        .get_field(Tag::Orientation, In::PRIMARY)
        .and_then(short_value);

    data
}

fn get_string_field(exif: &exif::Exif, tag: Tag) -> Option<String> {    let field = exif.get_field(tag, In::PRIMARY)?;
    let bytes = match &field.value {
        Value::Ascii(v) => v.iter().flatten().copied().collect(),
        Value::Undefined(v, _) => v.clone(),
        _ => return Some(field.display_value().to_string()),
    };
    Some(decode_string(&bytes))
}

fn parse_datetime_field(field: &exif::Field) -> Option<String> {
    // kamadak-exif returns "YYYY:MM:DD HH:MM:SS" (possibly with fractional/tz).
    let s = field.display_value().to_string();
    normalize_datetime(&s)
}

fn normalize_datetime(s: &str) -> Option<String> {
    // Accept YYYY:MM:DD HH:MM:SS and YYYY-MM-DD HH:MM:SS.
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let b = s.as_bytes();
    let sep1 = b[4];
    let sep2 = b[7];
    if sep1 != b':' && sep1 != b'-' {
        return None;
    }
    if sep2 != b':' && sep2 != b'-' {
        return None;
    }
    let year = &s[0..4];
    let month = &s[5..7];
    let day = &s[8..10];
    let time = &s[11..19]; // "HH:MM:SS"
    Some(format!("{year}-{month}-{day} {time}"))
}

fn short_value(f: &exif::Field) -> Option<i64> {
    match &f.value {
        Value::Short(v) => v.first().map(|&x| x as i64),
        Value::Long(v) => v.first().map(|&x| x as i64),
        _ => None,
    }
}

fn rational_value(f: &exif::Field) -> Option<f64> {
    match &f.value {
        Value::Rational(v) => v.first().map(|r| r.to_f64()),
        _ => None,
    }
}

fn exposure_fraction(f: &exif::Field) -> Option<String> {
    match &f.value {
        Value::Rational(v) => {
            let r = v.first()?;
            if r.denom < 2 {
                return Some(format!("{:.0}s", r.to_f64()));
            }
            let denom = f64::round((1.0 / r.to_f64()) as f64);
            Some(format!("1/{}s", denom as i64))
        }
        _ => None,
    }
}

fn trim_float(v: f64) -> String {
    if (v - v.round()).abs() < 1e-6 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

/// Fraction of characters considered "printable" (not control characters).
fn printable_ratio(s: &str) -> f64 {
    if s.is_empty() {
        return 1.0;
    }
    let printable = s
        .chars()
        .filter(|c| !c.is_control())
        .count();
    printable as f64 / s.chars().count() as f64
}

/// Decode EXIF string bytes using the PRD 6.4 fallback chain:
/// ASCII -> UTF-8 -> GBK -> Shift-JIS -> EUC-KR -> Hex.
pub fn decode_string(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    // 1. ASCII (all bytes < 0x80).
    if bytes.iter().all(|&b| b.is_ascii()) {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    // 2. UTF-8.
    if let Ok(s) = std::str::from_utf8(bytes) {
        if printable_ratio(s) >= 0.7 {
            return s.to_string();
        }
    }
    // 3. GBK (common for Simplified Chinese cameras).
    let (cow, _, had_errors) = encoding_rs::GBK.decode(bytes);
    if !had_errors && printable_ratio(&cow) >= 0.7 {
        return cow.into_owned();
    }
    // 4. Shift-JIS (common for Japanese cameras).
    let (cow, _, had_errors) = encoding_rs::SHIFT_JIS.decode(bytes);
    if !had_errors && printable_ratio(&cow) >= 0.7 {
        return cow.into_owned();
    }
    // 5. EUC-KR (common for Korean cameras).
    let (cow, _, had_errors) = encoding_rs::EUC_KR.decode(bytes);
    if !had_errors && printable_ratio(&cow) >= 0.7 {
        return cow.into_owned();
    }
    // 6. Hex fallback.
    let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
    format!("0x{hex}")
}

/// Extract the embedded JPEG preview/thumbnail bytes from an image file.
///
/// For RAW (TIFF-based) formats the thumbnail/preview is stored inside the file
/// and referenced by the EXIF `JPEGInterchangeFormat`/`...Length` tags. We seek
/// to that byte range and return the JPEG bytes so we can build a thumbnail
/// without fully decoding the RAW. Returns None when there is no usable
/// embedded preview (e.g. some formats).
pub fn extract_embedded_preview(path: &Path) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut magic = [0u8; 2];
    file.read_exact(&mut magic).ok()?;

    // JPEG files carry the EXIF TIFF header at offset 6; TIFF/RAW at offset 0.
    // We try both bases so a RAW whose stored offset is absolute (base 0) or an
    // odd container still resolves correctly.
    let base_candidates: [u64; 2] = if magic == [0xFF, 0xD8] { [6, 0] } else { [0, 6] };

    let file2 = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file2);
    let exif = Reader::new().read_from_container(&mut reader).ok()?;

    for ifd in [In::PRIMARY, In::THUMBNAIL] {
        let Some(offset) = exif
            .get_field(Tag::JPEGInterchangeFormat, ifd)
            .and_then(long_value)
        else {
            continue;
        };
        let Some(length) = exif
            .get_field(Tag::JPEGInterchangeFormatLength, ifd)
            .and_then(long_value)
        else {
            continue;
        };
        if length == 0 || length > 128 * 1024 * 1024 {
            continue;
        }
        for base in &base_candidates {
            let abs = base.saturating_add(offset);
            if let Some(buf) = read_abs(&mut reader, abs, length as usize) {
                // Validate it is JPEG (SOI) — the embedded preview is a JPEG.
                if buf.len() >= 2 && buf[0] == 0xFF && buf[1] == 0xD8 {
                    return Some(buf);
                }
            }
        }
    }
    None
}

/// Cheap full-resolution dimension hint from the EXIF PixelXDimension /
/// PixelYDimension (ExifImageWidth/Height) tags. Used by the Z-key 100% view
/// to frame the display before the slow RAW decode lands; the authoritative
/// dimensions arrive with the decoded pixels themselves. Returns None when
/// the tags are absent or degenerate.
pub fn pixel_dims(path: &Path) -> Option<(u32, u32)> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let exif = Reader::new().read_from_container(&mut reader).ok()?;
    let w = exif
        .get_field(Tag::ImageWidth, In::PRIMARY)
        .and_then(short_value)?;
    let h = exif
        .get_field(Tag::ImageLength, In::PRIMARY)
        .and_then(short_value)?;
    let (w, h) = (w as u32, h as u32);
    if w == 0 || h == 0 {
        return None;
    }
    Some((w, h))
}

fn read_abs<R: std::io::Seek + std::io::Read>(
    reader: &mut R,
    pos: u64,
    len: usize,
) -> Option<Vec<u8>> {
    if reader.seek(SeekFrom::Start(pos)).is_err() {
        return None;
    }
    let mut buf = vec![0u8; len];
    if reader.read_exact(&mut buf).is_err() {
        return None;
    }
    Some(buf)
}

fn long_value(f: &exif::Field) -> Option<u64> {
    match &f.value {
        Value::Short(v) => v.first().map(|&x| x as u64),
        Value::Long(v) => v.first().map(|&x| x as u64),
        Value::SShort(v) => v.first().map(|&x| x as u64),
        Value::SLong(v) => v.first().map(|&x| x as u64),
        Value::Rational(v) => v.first().map(|r| r.num.max(0) as u64),
        _ => None,
    }
}
