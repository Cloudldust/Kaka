//! XMP rating metadata: sidecar XML packet + in-file embedding (PRD 12.3).
//!
//! Lightroom Classic only reads `.xmp` sidecars for proprietary RAW files;
//! for JPEG/PNG (and TIFF/DNG) it ignores sidecars entirely and reads metadata
//! embedded inside the file itself. So "write XMP marks" writes the sidecar
//! for every kept photo and additionally embeds the same packet into JPEG
//! (APP1 segment) and PNG (iTXt `XML:com.adobe.xmp` chunk) files.

use anyhow::{anyhow, Context, Result};
use std::path::Path;

/// Namespace prefix that starts every XMP APP1 payload in JPEG.
const JPEG_XMP_NS: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
/// Extended-XMP continuation segments (drop these too when replacing).
const JPEG_XMP_EXT_NS: &[u8] = b"http://ns.adobe.com/xmp/extension/";
/// PNG iTXt keyword mandated by the XMP spec for embedded packets.
const PNG_XMP_KEYWORD: &[u8] = b"XML:com.adobe.xmp";

/// Build the minimal XMP packet with keywords/label/rating/orientation that
/// Lightroom reads (`xmp:Rating`, `xmp:Label`, `dc:subject`, `tiff:Orientation`).
pub fn rating_xml(label: &str, rating: u8, orientation: i64) -> String {
    let orient = if orientation != 0 {
        format!("\n   <tiff:Orientation>{orientation}</tiff:Orientation>")
    } else {
        String::new()
    };
    format!(
        r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
     xmlns:dc="http://purl.org/dc/elements/1.1/"
     xmlns:xmp="http://ns.adobe.com/xap/1.0/"
     xmlns:tiff="http://ns.adobe.com/tiff/1.0/">
   <dc:subject><rdf:Bag><rdf:li>{label}</rdf:li></rdf:Bag></dc:subject>
   <xmp:Label>{label}</xmp:Label>
   <xmp:Rating>{rating}</xmp:Rating>{orient}
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#,
        label = label,
        rating = rating,
        orient = orient,
    )
}

/// Embed `xml` into JPEG `data` as an APP1 segment. Any existing XMP APP1
/// (standard or extended) is replaced, everything else is byte-identical.
pub fn jpeg_embed(data: &[u8], xml: &str) -> Result<Vec<u8>> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(anyhow!("not a JPEG (missing SOI marker)"));
    }
    let payload_len = JPEG_XMP_NS.len() + xml.len();
    anyhow::ensure!(
        payload_len + 2 <= 0xFFFF,
        "XMP packet too large for a JPEG APP1 segment"
    );

    let mut out = Vec::with_capacity(data.len() + payload_len + 8);
    out.extend_from_slice(&data[..2]); // SOI

    let mut i = 2usize;
    // XMP APP1 goes after any JFIF/JFXX APP0 segments and before Exif (XMP
    // spec part 3 places it as early as possible, following only APP0s).
    while i + 4 <= data.len() && data[i] == 0xFF && data[i + 1] == 0xE0 {
        let end = seg_end(data, i)?;
        out.extend_from_slice(&data[i..end]);
        i = end;
    }

    // The new XMP APP1 segment.
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((payload_len + 2) as u16).to_be_bytes());
    out.extend_from_slice(JPEG_XMP_NS);
    out.extend_from_slice(xml.as_bytes());

    // Copy the remaining segments, dropping old XMP APP1s, up to SOS; the
    // entropy-coded tail after SOS is copied verbatim.
    while i + 4 <= data.len() && data[i] == 0xFF {
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1; // fill bytes before a marker
            continue;
        }
        if marker == 0xDA {
            out.extend_from_slice(&data[i..]);
            return Ok(out);
        }
        if is_standalone_marker(marker) {
            out.extend_from_slice(&data[i..i + 2]);
            i += 2;
            continue;
        }
        let end = seg_end(data, i)?;
        if end < i + 4 {
            return Err(anyhow!("JPEG segment with invalid length"));
        }
        let payload = &data[i + 4..end];
        let is_old_xmp =
            marker == 0xE1 && (payload.starts_with(JPEG_XMP_NS) || payload.starts_with(JPEG_XMP_EXT_NS));
        if !is_old_xmp {
            out.extend_from_slice(&data[i..end]);
        }
        i = end;
    }
    Err(anyhow!("JPEG has no SOS segment"))
}

fn is_standalone_marker(marker: u8) -> bool {
    matches!(marker, 0x01 | 0xD0..=0xD9)
}

/// End offset (exclusive) of the segment whose marker starts at `i`
/// (`data[i] == 0xFF`, `data[i+1]` = marker, then a 2-byte big-endian length).
fn seg_end(data: &[u8], i: usize) -> Result<usize> {
    let len = data
        .get(i + 2..i + 4)
        .map(|s| u16::from_be_bytes([s[0], s[1]]) as usize)
        .ok_or_else(|| anyhow!("JPEG truncated (segment length)"))?;
    let end = i + 2 + len;
    if end > data.len() {
        return Err(anyhow!("JPEG segment overruns file"));
    }
    Ok(end)
}

/// Embed `xml` into PNG `data` as an iTXt chunk with the XMP keyword. Any
/// existing XMP iTXt is replaced; all other chunks (and their order) are kept.
pub fn png_embed(data: &[u8], xml: &str) -> Result<Vec<u8>> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if data.len() < 8 || data[..8] != SIG {
        return Err(anyhow!("not a PNG (bad signature)"));
    }

    let mut out = Vec::with_capacity(data.len() + xml.len() + 64);
    out.extend_from_slice(&SIG);
    let mut i = 8usize;
    loop {
        if i + 8 > data.len() {
            return Err(anyhow!("PNG truncated (missing IEND)"));
        }
        let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        let ctype = &data[i + 4..i + 8];
        let chunk_end = i + 8 + len;
        if chunk_end + 4 > data.len() {
            return Err(anyhow!("PNG chunk overruns file"));
        }
        if ctype == b"IEND" {
            push_png_itxt(&mut out, xml);
            out.extend_from_slice(&data[i..chunk_end + 4]);
            return Ok(out);
        }
        let is_old_xmp = ctype == b"iTXt" && data[i + 8..chunk_end].starts_with(PNG_XMP_KEYWORD);
        if !is_old_xmp {
            out.extend_from_slice(&data[i..chunk_end + 4]);
        }
        i = chunk_end + 4;
    }
}

/// Append an uncompressed iTXt chunk carrying the XMP packet.
fn push_png_itxt(out: &mut Vec<u8>, xml: &str) {
    let mut chunk: Vec<u8> = Vec::with_capacity(PNG_XMP_KEYWORD.len() + 5 + xml.len());
    chunk.extend_from_slice(PNG_XMP_KEYWORD);
    chunk.push(0); // keyword terminator
    chunk.push(0); // compression flag: uncompressed
    chunk.push(0); // compression method
    chunk.push(0); // language tag (empty)
    chunk.push(0); // translated keyword (empty)
    chunk.extend_from_slice(xml.as_bytes());

    let mut crc_input = Vec::with_capacity(4 + chunk.len());
    crc_input.extend_from_slice(b"iTXt");
    crc_input.extend_from_slice(&chunk);

    out.extend_from_slice(&(chunk.len() as u32).to_be_bytes());
    out.extend_from_slice(b"iTXt");
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// Embed `xml` into the photo file at `path` (JPEG or PNG only), replacing the
/// file atomically via a temp file in the same directory.
pub fn embed_into_file(path: &Path, xml: &str) -> Result<()> {
    let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let ext = crate::io::format::ext(path);
    let embedded = match ext.as_str() {
        "jpg" | "jpeg" | "jfif" => jpeg_embed(&data, xml)?,
        "png" => png_embed(&data, xml)?,
        other => return Err(anyhow!("embedded XMP unsupported for .{other}")),
    };

    let tmp = path.with_extension(format!("kakatmp{}", std::process::id()));
    std::fs::write(&tmp, &embedded)
        .with_context(|| format!("write {}", tmp.display()))?;
    if std::fs::rename(&tmp, path).is_err() {
        // Windows rename normally replaces (MOVEFILE_REPLACE_EXISTING); fall
        // back to remove+rename for exotic filesystems.
        let _ = std::fs::remove_file(path);
        std::fs::rename(&tmp, path)
            .with_context(|| format!("replace {}", path.display()))?;
    }
    Ok(())
}

/// CRC-32 (IEEE/PNG: reflected, poly 0xEDB88320, init/final 0xFFFFFFFF).
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Merge the keep-mark fields (xmp:Label / xmp:Rating / tiff:Orientation)
/// into an existing XMP packet, preserving every other field. Only those
/// three are added or replaced (both element-form and attribute-form
/// occurrences of them are removed first); missing namespace declarations
/// are added to the rdf:Description tag. Returns None when `existing` is not
/// recognisable XMP — the caller then falls back to a fresh standard packet
/// and logs a warning.
pub fn merge_rating_fields(
    existing: &str,
    label: &str,
    rating: u8,
    orientation: i64,
) -> Option<String> {
    // Minimal structure check: an XMP packet wraps an rdf:Description.
    if !existing.contains("<x:xmpmeta") || !existing.contains("<rdf:RDF") {
        return None;
    }
    let open_start = existing.find("<rdf:Description")?;
    let open_end = existing[open_start..].find('>')? + open_start;
    let close_start = existing[open_start..].find("</rdf:Description>")? + open_start;
    if close_start <= open_end {
        return None;
    }
    let head = &existing[..open_start];
    let mut open_tag = existing[open_start..=open_end].to_string();
    let body = existing[open_end + 1..close_start].to_string();
    let tail = &existing[close_start..];

    // The inserted fields need their namespaces declared on the Description.
    for (decl, ns) in [
        ("xmlns:xmp=", "http://ns.adobe.com/xap/1.0/"),
        ("xmlns:tiff=", "http://ns.adobe.com/tiff/1.0/"),
    ] {
        if !open_tag.contains(decl) {
            open_tag.insert_str(open_tag.len() - 1, &format!(" {decl}\"{ns}\""));
        }
    }
    // Drop attribute-shorthand occurrences of our fields (some writers use
    // <rdf:Description ... xmp:Rating="3" ...>).
    strip_attr(&mut open_tag, "xmp:Label");
    strip_attr(&mut open_tag, "xmp:Rating");
    strip_attr(&mut open_tag, "tiff:Orientation");

    // Drop element-form occurrences of our fields from the body.
    let body = strip_element(body, "xmp:Label");
    let body = strip_element(body, "xmp:Rating");
    let body = strip_element(body, "tiff:Orientation");

    let orient_xml = if orientation != 0 {
        format!("\n   <tiff:Orientation>{orientation}</tiff:Orientation>")
    } else {
        String::new()
    };
    let inject = format!(
        "\n   <xmp:Label>{label}</xmp:Label>\n   <xmp:Rating>{rating}</xmp:Rating>{orient_xml}"
    );
    Some(format!("{head}{open_tag}{body}{inject}{tail}"))
}

/// Remove `name="value"` attribute occurrences from a tag string.
fn strip_attr(tag: &mut String, name: &str) {
    let needle = format!("{name}=\"");
    while let Some(pos) = tag.find(&needle) {
        let after = pos + needle.len();
        let Some(end_rel) = tag[after..].find('"') else { break };
        tag.replace_range(pos..after + end_rel + 1, "");
    }
}

/// Remove `<name>…</name>` (or self-closing `<name/>`) element occurrences
/// from a body string, keeping everything else byte-identical.
fn strip_element(body: String, name: &str) -> String {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut result = String::with_capacity(body.len());
    let mut pos = 0usize;
    while let Some(rel) = body[pos..].find(&open) {
        let start = pos + rel;
        result.push_str(&body[pos..start]);
        let after_open = start + open.len();
        let next = body[after_open..].chars().next();
        let is_element = matches!(
            next,
            Some('>') | Some('/') | Some(' ') | Some('\n') | Some('\r') | Some('\t')
        );
        if !is_element {
            // False positive like <xmp:RatingX — keep and continue scanning.
            result.push_str(&body[start..after_open]);
            pos = after_open;
            continue;
        }
        if next == Some('/') {
            let Some(end_rel) = body[after_open..].find('>') else { break };
            pos = after_open + end_rel + 1;
        } else {
            match body[after_open..].find(&close) {
                Some(c) => pos = after_open + c + close.len(),
                None => break, // malformed; keep the remainder untouched
            }
        }
    }
    result.push_str(&body[pos..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vectors() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn rating_xml_has_lr_properties() {
        let xml = rating_xml("Kaka:Keep", 4, 90);
        assert!(xml.contains("<xmp:Rating>4</xmp:Rating>"));
        assert!(!xml.contains("crs:Rating"));
        assert!(xml.contains("<tiff:Orientation>90</tiff:Orientation>"));
    }

    fn encode_tmp(name: &str, png: bool) -> Vec<u8> {
        let path = std::env::temp_dir().join(format!("kaka_xmp_{}_{}", std::process::id(), name));
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(16, 12, |x, y| {
            image::Rgb([(x * 8) as u8, (y * 8) as u8, 128])
        }));
        let fmt = if png { image::ImageFormat::Png } else { image::ImageFormat::Jpeg };
        img.save_with_format(&path, fmt).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        bytes
    }

    fn count_jpeg_xmp(data: &[u8]) -> usize {
        let mut n = 0;
        let mut i = 2usize;
        while i + 4 <= data.len() && data[i] == 0xFF {
            let marker = data[i + 1];
            if marker == 0xDA {
                break;
            }
            let end = seg_end(data, i).unwrap();
            if marker == 0xE1 && data[i + 4..end].starts_with(JPEG_XMP_NS) {
                n += 1;
            }
            i = end;
        }
        n
    }

    fn count_png_xmp(data: &[u8]) -> usize {
        let mut n = 0;
        let mut i = 8usize;
        while i + 8 <= data.len() {
            let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
            let ctype = &data[i + 4..i + 8];
            let chunk_end = i + 8 + len;
            if ctype == b"iTXt" && data[i + 8..chunk_end].starts_with(PNG_XMP_KEYWORD) {
                n += 1;
            }
            if ctype == b"IEND" {
                break;
            }
            i = chunk_end + 4;
        }
        n
    }

    #[test]
    fn jpeg_embed_replaces_and_stays_decodable() {
        let xml = rating_xml("Kaka:Keep", 4, 0);
        let jpg = encode_tmp("src.jpg", false);
        let once = jpeg_embed(&jpg, &xml).unwrap();
        assert_eq!(count_jpeg_xmp(&once), 1);
        image::load_from_memory(&once).unwrap();

        // Embedding again must replace, not duplicate.
        let twice = jpeg_embed(&once, &xml).unwrap();
        assert_eq!(count_jpeg_xmp(&twice), 1);
        image::load_from_memory(&twice).unwrap();

        assert!(jpeg_embed(&[0x00, 0x01], &xml).is_err());
    }

    #[test]
    fn png_embed_replaces_and_stays_decodable() {
        let xml = rating_xml("Kaka:Keep", 3, 0);
        let png = encode_tmp("src.png", true);
        let once = png_embed(&png, &xml).unwrap();
        assert_eq!(count_png_xmp(&once), 1);
        image::load_from_memory(&once).unwrap();

        let twice = png_embed(&once, &xml).unwrap();
        assert_eq!(count_png_xmp(&twice), 1);
        image::load_from_memory(&twice).unwrap();

        assert!(png_embed(&[0x00; 16], &xml).is_err());
    }

    const FOREIGN_XMP: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/">
   <dc:description><rdf:Alt><rdf:li xml:lang="x-default">Custom caption</rdf:li></rdf:Alt></dc:description>
   <photoshop:City>Qingdao</photoshop:City>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    #[test]
    fn merge_preserves_foreign_fields() {
        let merged = merge_rating_fields(FOREIGN_XMP, "Kaka:Keep", 4, 90).unwrap();
        // Original fields survive untouched.
        assert!(merged.contains("Custom caption"));
        assert!(merged.contains("<photoshop:City>Qingdao</photoshop:City>"));
        // Our three fields are present, namespaces auto-declared.
        assert!(merged.contains("<xmp:Rating>4</xmp:Rating>"));
        assert!(merged.contains("<xmp:Label>Kaka:Keep</xmp:Label>"));
        assert!(merged.contains("<tiff:Orientation>90</tiff:Orientation>"));
        assert!(merged.contains("xmlns:xmp="));
        assert!(merged.contains("xmlns:tiff="));

        // Re-merge is idempotent: single occurrence, replaced values.
        let merged2 = merge_rating_fields(&merged, "Kaka:Keep", 2, 0).unwrap();
        assert_eq!(merged2.matches("<xmp:Rating>").count(), 1);
        assert!(merged2.contains("<xmp:Rating>2</xmp:Rating>"));
        assert!(merged2.contains("<xmp:Label>Kaka:Keep</xmp:Label>"));
        assert!(!merged2.contains("<tiff:Orientation>90"));
        assert!(merged2.contains("Custom caption"));
    }

    #[test]
    fn merge_replaces_element_and_attribute_forms() {
        let existing = "<x:xmpmeta><rdf:RDF><rdf:Description rdf:about=\"\" \
            xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmp:Rating=\"3\" xmp:Label=\"OldAttr\">\
            <xmp:Label>OldEl</xmp:Label><xmp:Rating>1</xmp:Rating>\
            <dc:title>T</dc:title></rdf:Description></rdf:RDF></x:xmpmeta>";
        let merged = merge_rating_fields(existing, "Kaka:Keep", 5, 0).unwrap();
        assert!(!merged.contains("xmp:Rating=\"3\""));
        assert!(!merged.contains("xmp:Label=\"OldAttr\""));
        assert!(!merged.contains("OldEl"));
        assert!(merged.matches("<xmp:Rating>").count() == 1);
        assert!(merged.contains("<xmp:Rating>5</xmp:Rating>"));
        assert!(merged.contains("<dc:title>T</dc:title>"));
    }

    #[test]
    fn merge_rejects_non_xmp() {
        assert!(merge_rating_fields("<html><body>hi</body></html>", "K", 3, 0).is_none());
        // Metadata but no rdf:Description to merge into.
        assert!(merge_rating_fields(
            "<x:xmpmeta><rdf:RDF></rdf:RDF></x:xmpmeta>",
            "K",
            3,
            0
        )
        .is_none());
    }

    #[test]
    fn embed_into_file_roundtrip_and_reject() {
        let xml = rating_xml("Kaka:Keep", 5, 0);
        let dir = std::env::temp_dir().join(format!("kaka_xmp_dir_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let jpg_path = dir.join("photo.jpg");
        std::fs::write(&jpg_path, encode_tmp("rt.jpg", false)).unwrap();
        embed_into_file(&jpg_path, &xml).unwrap();
        let data = std::fs::read(&jpg_path).unwrap();
        assert_eq!(count_jpeg_xmp(&data), 1);
        image::load_from_memory(&data).unwrap();

        let txt_path = dir.join("note.txt");
        std::fs::write(&txt_path, b"hello").unwrap();
        assert!(embed_into_file(&txt_path, &xml).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }
}
