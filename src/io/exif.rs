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

/// Parse the numeric f-value from the stored aperture string ("f/5.6", "f/8").
pub fn parse_aperture_num(s: &str) -> Option<f64> {
    let s = s.trim();
    let s = s.strip_prefix("f/").unwrap_or(s);
    let v: f64 = s.parse().ok()?;
    (v > 0.0).then_some(v)
}

/// Parse the exposure seconds from the stored shutter string
/// ("1/200s" -> 0.005, "2s" -> 2.0).
pub fn parse_shutter_num(s: &str) -> Option<f64> {
    let s = s.trim();
    let s = s.strip_suffix('s').unwrap_or(s);
    if let Some((num, den)) = s.split_once('/') {
        let n: f64 = num.trim().parse().ok()?;
        let d: f64 = den.trim().parse().ok()?;
        (n > 0.0 && d > 0.0).then_some(n / d)
    } else {
        let v: f64 = s.parse().ok()?;
        (v > 0.0).then_some(v)
    }
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
    let orient = exif
        .get_field(Tag::Orientation, In::PRIMARY)
        .and_then(short_value);
    // Prefer the Exif sub-IFD's PixelX/YDimension (cameras that record it put
    // the true size there), then IFD0's ImageWidth/Length.
    let dim = |tag: Tag| exif.get_field(tag, In::PRIMARY).and_then(short_value);
    let mut w = dim(Tag::PixelXDimension)
        .or_else(|| dim(Tag::ImageWidth))
        .map(|v| v as u32)?;
    let mut h = dim(Tag::PixelYDimension)
        .or_else(|| dim(Tag::ImageLength))
        .map(|v| v as u32)?;
    if w == 0 || h == 0 {
        return None;
    }
    // NEF quirk (D7100 verified): IFD0's ImageWidth/Length is only the
    // embedded ~120x160 thumbnail, and the Exif sub-IFD has no PixelX/Y —
    // the sensor dims live in the raw SubIFD behind TIFF tag 0x014A. A
    // sub-1000px "full resolution" is never real, so walk the TIFF headers
    // for the actual dims.
    if w.max(h) < 1000 {
        if let Some((tw, th)) = tiff_full_dims(path) {
            w = tw;
            h = th;
        }
    }
    if w.max(h) < 1000 {
        return None;
    }
    // EXIF Orientation 5–8 are 90° rotations: PixelX/YDimension describe the
    // sensor (landscape) while every decoded texture applies the orientation
    // and is presented portrait. Swap so the dimension hint (Z-key framing,
    // PRD 7.4) matches what the decode will produce — otherwise the preview
    // is stretched into the wrong aspect until the decode lands.
    if let Some(o) = orient {
        if (5..=8).contains(&o) {
            std::mem::swap(&mut w, &mut h);
        }
    }
    Some((w, h))
}

/// Walk a TIFF-based RAW's IFD chain (IFD0 → SubIFDs behind tag 0x014A) for
/// the largest plausible ImageWidth/Length pair — the full sensor dims. Reads
/// only IFD headers (a few hundred bytes), fast enough for the UI thread.
/// Returns None for non-TIFF containers (CR3/RAF/ORF magic variants).
fn tiff_full_dims(path: &Path) -> Option<(u32, u32)> {
    use std::io::{Read, Seek};
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 8];
    f.read_exact(&mut head).ok()?;
    let le = match &head[0..4] {
        b"II\x2a\x00" => true,
        b"MM\x00\x2a" => false,
        _ => return None,
    };
    let u16_at =
        |b: &[u8]| if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) };
    let u32_at = |b: &[u8]| {
        if le {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        }
    };

    let mut best: Option<(u32, u32)> = None;
    let mut queue = vec![u32_at(&head[4..8])]; // IFD0 offset
    let mut visited = 0usize;
    while let Some(off) = queue.pop() {
        visited += 1;
        if off == 0 || visited > 8 {
            continue;
        }
        f.seek(std::io::SeekFrom::Start(off as u64)).ok()?;
        let mut nb = [0u8; 2];
        f.read_exact(&mut nb).ok()?;
        let n = u16_at(&nb) as usize;
        if n == 0 || n > 512 {
            continue;
        }
        let mut buf = vec![0u8; n * 12];
        f.read_exact(&mut buf).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        for i in 0..n {
            let e = &buf[i * 12..i * 12 + 12];
            let (tag, typ, cnt) = (u16_at(&e[0..2]), u16_at(&e[2..4]), u32_at(&e[4..8]));
            // A count-1 value fits inline in the 4-byte value field
            // (SHORT in the low half, LONG across all of it).
            let inline = u32_at(&e[8..12]);
            match (tag, typ, cnt) {
                (0x0100, 3, 1) => w = u16_at(&e[8..10]) as u32,
                (0x0100, 4, 1) => w = inline,
                (0x0101, 3, 1) => h = u16_at(&e[8..10]) as u32,
                (0x0101, 4, 1) => h = inline,
                (0x014A, 4, 1) => queue.push(inline),
                (0x014A, 4, _) if cnt <= 8 && inline > 0 => {
                    // SubIFD offset array lives at the value offset.
                    f.seek(std::io::SeekFrom::Start(inline as u64)).ok()?;
                    let mut a = vec![0u8; cnt as usize * 4];
                    if f.read_exact(&mut a).is_ok() {
                        for k in 0..cnt as usize {
                            queue.push(u32_at(&a[k * 4..k * 4 + 4]));
                        }
                    }
                }
                _ => {}
            }
        }
        if w > 0 && h > 0 {
            best = match best {
                Some((bw, bh)) if bw as u64 * bh as u64 >= w as u64 * h as u64 => best,
                _ => Some((w, h)),
            };
        }
    }
    best
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal little-endian TIFF with IFD0 entries:
    /// (tag, type, count, inline value). SHORT count-1 values sit in the low
    /// half of the 4-byte value field, so writing the full u32 covers both.
    fn tiff_le(entries: &[(u16, u16, u32, i64)]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"II");
        buf.extend_from_slice(&0x2Au16.to_le_bytes());
        buf.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
        buf.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, ty, count, val) in entries {
            buf.extend_from_slice(&tag.to_le_bytes());
            buf.extend_from_slice(&ty.to_le_bytes());
            buf.extend_from_slice(&count.to_le_bytes());
            buf.extend_from_slice(&(*val as u32).to_le_bytes());
        }
        buf.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
        buf
    }

    /// NEF-style TIFF: IFD0 carries only a small embedded thumbnail's
    /// dimensions plus a SubIFD pointer (0x014A); the real sensor dims live
    /// in the SubIFD. Written as a raw TIFF container (like a real NEF).
    fn write_nef_like(
        dir: &Path,
        name: &str,
        orientation: i64,
        thumb: (u32, u32),
        sensor: (u32, u32),
    ) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let ifd0 = tiff_le(&[
            (0x0112, 3, 1, orientation),    // Orientation
            (0x0100, 3, 1, thumb.0 as i64), // ImageWidth  = thumbnail!
            (0x0101, 3, 1, thumb.1 as i64), // ImageLength = thumbnail!
            (0x014A, 4, 1, 0),              // SubIFDs offset, patched below
        ]);
        let sub_off = ifd0.len() as i64; // SubIFD starts right after IFD0
        let mut sub = tiff_le(&[
            (0x0100, 4, 1, sensor.0 as i64),
            (0x0101, 4, 1, sensor.1 as i64),
        ]);
        sub.drain(0..8); // strip the second TIFF header
        let mut buf = ifd0;
        let tail = buf.split_off(buf.len() - 4); // drop IFD0's next-IFD zero
        // Patch the SubIFD pointer (last entry's inline value field).
        let ptr_at = buf.len() - 4;
        buf[ptr_at..].copy_from_slice(&(sub_off as u32).to_le_bytes());
        buf.extend_from_slice(&tail);
        buf.extend_from_slice(&sub);
        let path = dir.join(name);
        std::fs::write(&path, buf).unwrap();
        path
    }

    /// Wrap a TIFF blob into a minimal JPEG container (SOI + APP1 Exif + EOI),
    /// enough for kamadak-exif's container reader.
    fn jpeg_with_exif(tiff: &[u8]) -> Vec<u8> {
        let payload_len = 6 + tiff.len(); // "Exif\0\0" + TIFF
        let mut jpg = vec![0xFF, 0xD8];
        jpg.extend_from_slice(&[0xFF, 0xE1]);
        jpg.extend_from_slice(&(payload_len as u16 + 2).to_be_bytes());
        jpg.extend_from_slice(b"Exif\0\0");
        jpg.extend_from_slice(tiff);
        jpg.extend_from_slice(&[0xFF, 0xD9]);
        jpg
    }

    fn write_exif_jpg(dir: &Path, name: &str, orientation: i64) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let tiff = tiff_le(&[
            (0x0112, 3, 1, orientation), // Orientation
            (0x0100, 3, 1, 8256),        // ImageWidth (PixelXDimension)
            (0x0101, 3, 1, 5504),        // ImageLength (PixelYDimension)
        ]);
        let path = dir.join(name);
        std::fs::write(&path, jpeg_with_exif(&tiff)).unwrap();
        path
    }

    #[test]
    fn pixel_dims_swaps_for_rotated_orientation() {
        let dir = std::env::temp_dir().join(format!("kaka_exif_{}", std::process::id()));

        // Orientation 6 (rotate 90° CW): landscape sensor → portrait display.
        let p = write_exif_jpg(&dir, "o6.jpg", 6);
        assert_eq!(pixel_dims(&p), Some((5504, 8256)));

        // Orientation 8 (rotate 270° CW): swapped as well.
        let p = write_exif_jpg(&dir, "o8.jpg", 8);
        assert_eq!(pixel_dims(&p), Some((5504, 8256)));

        // Orientation 1 (normal) and 4 (flip): sensor order kept.
        let p = write_exif_jpg(&dir, "o1.jpg", 1);
        assert_eq!(pixel_dims(&p), Some((8256, 5504)));
        let p = write_exif_jpg(&dir, "o4.jpg", 4);
        assert_eq!(pixel_dims(&p), Some((8256, 5504)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_aperture_and_shutter_strings() {
        assert_eq!(parse_aperture_num("f/5.6"), Some(5.6));
        assert_eq!(parse_aperture_num("f/8"), Some(8.0));
        assert_eq!(parse_aperture_num("junk"), None);
        assert!((parse_shutter_num("1/200s").unwrap() - 0.005).abs() < 1e-9);
        assert_eq!(parse_shutter_num("2s"), Some(2.0));
        assert_eq!(parse_shutter_num("1/60s").unwrap(), 1.0 / 60.0);
        assert_eq!(parse_shutter_num("garbage"), None);
    }

    #[test]
    fn pixel_dims_reads_nef_subifd_dims() {
        let dir = std::env::temp_dir().join(format!("kaka_nef_{}", std::process::id()));

        // NEF layout: IFD0's ImageWidth/Length is a 160x120 thumbnail; the
        // sensor (6000x4000) sits in the SubIFD behind tag 0x014A.
        let p = write_nef_like(&dir, "dsc.nef", 8, (160, 120), (6000, 4000));
        assert_eq!(tiff_full_dims(&p), Some((6000, 4000)));
        // Orientation 8 swaps to portrait, matching the decoded texture.
        assert_eq!(pixel_dims(&p), Some((4000, 6000)));

        let p = write_nef_like(&dir, "lscape.nef", 1, (160, 120), (6000, 4000));
        assert_eq!(pixel_dims(&p), Some((6000, 4000)));

        std::fs::remove_dir_all(&dir).ok();
    }
}
