//! Pairview: L1 metadata for one file (docs/IMPLEMENTATION_PLAN.md M1-05).
//!
//! Only the start of the file is read: on the NAS a whole ARW costs ~0.5 s, while its EXIF sits
//! in the first few KB. ARW and JPG are parsed directly by kamadak-exif. For HEIF (Sony HIF),
//! kamadak can't read the container, so the EXIF item is located through the HEIF `iloc` index
//! and its TIFF block handed to kamadak. If the start of the file isn't enough, macOS falls back
//! to ImageIO on the whole file.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::Result;
use exif::{In, Tag, Value};

/// How much of a file to read first. Sony puts HIF EXIF at ~60 KB and ARW EXIF in the first KB.
const PREFIX_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExifInfo {
    /// `DateTimeOriginal` as ISO 8601 without zone, e.g. `2026-10-23T14:05:33`.
    pub date_original: Option<String>,
    pub subsec_ms: Option<i64>,
    pub offset_original: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub lens: Option<String>,
    pub orientation: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub iso: Option<i64>,
    pub exposure_s: Option<f64>,
    pub f_number: Option<f64>,
    pub focal_mm: Option<f64>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
    pub gps_alt: Option<f64>,
    /// Bytes read from disk to get this, for the NAS budget.
    pub bytes_read: u64,
}

pub fn read_exif(path: &Path) -> Result<ExifInfo> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut bytes = Vec::with_capacity(PREFIX_BYTES.min(len) as usize);
    (&mut file).take(PREFIX_BYTES).read_to_end(&mut bytes)?;
    let mut info = parse(&bytes).unwrap_or_default();
    if info.date_original.is_none() && len > PREFIX_BYTES {
        file.read_to_end(&mut bytes)?;
        info = parse(&bytes)
            .filter(|i| i.date_original.is_some())
            .or_else(|| imageio::parse(&bytes))
            .unwrap_or_default();
    }
    info.bytes_read = bytes.len() as u64;
    Ok(info)
}

fn parse(bytes: &[u8]) -> Option<ExifInfo> {
    let exif = if bytes.get(4..8) == Some(b"ftyp") {
        exif::Reader::new()
            .read_raw(heif_exif_tiff(bytes)?.to_vec())
            .ok()?
    } else {
        exif::Reader::new()
            .read_from_container(&mut std::io::Cursor::new(bytes))
            .ok()?
    };
    Some(from_kamadak(&exif))
}

fn from_kamadak(exif: &exif::Exif) -> ExifInfo {
    let field = |tag| exif.get_field(tag, In::PRIMARY).map(|f| &f.value);
    let text = |tag| match field(tag)? {
        Value::Ascii(v) => {
            let s = String::from_utf8_lossy(v.first()?);
            let s = s.trim().trim_end_matches('\0').trim();
            (!s.is_empty()).then(|| s.to_string())
        }
        _ => None,
    };
    let nums = |tag| -> Vec<f64> {
        match field(tag) {
            Some(Value::Rational(v)) => v.iter().map(|r| r.to_f64()).collect(),
            Some(Value::SRational(v)) => v.iter().map(|r| r.to_f64()).collect(),
            Some(Value::Short(v)) => v.iter().map(|&n| n as f64).collect(),
            Some(Value::Long(v)) => v.iter().map(|&n| n as f64).collect(),
            Some(Value::Byte(v)) => v.iter().map(|&n| n as f64).collect(),
            _ => Vec::new(),
        }
    };
    let num = |tag| nums(tag).first().copied().filter(|v| v.is_finite());
    // GPS degrees/minutes/seconds → signed decimal degrees.
    let coord = |tag, ref_tag, negative: &str| {
        let dms = nums(tag);
        let value =
            dms.first()? + dms.get(1).unwrap_or(&0.0) / 60.0 + dms.get(2).unwrap_or(&0.0) / 3600.0;
        Some(if text(ref_tag).as_deref() == Some(negative) {
            -value
        } else {
            value
        })
    };
    ExifInfo {
        date_original: text(Tag::DateTimeOriginal).as_deref().and_then(iso_date),
        subsec_ms: text(Tag::SubSecTimeOriginal).as_deref().and_then(subsec_ms),
        offset_original: text(Tag::OffsetTimeOriginal),
        make: text(Tag::Make),
        model: text(Tag::Model),
        serial: text(Tag::BodySerialNumber),
        lens: text(Tag::LensModel),
        orientation: num(Tag::Orientation).map(|v| v as i64),
        width: num(Tag::PixelXDimension).map(|v| v as i64),
        height: num(Tag::PixelYDimension).map(|v| v as i64),
        iso: num(Tag::PhotographicSensitivity).map(|v| v as i64),
        exposure_s: num(Tag::ExposureTime),
        f_number: num(Tag::FNumber),
        focal_mm: num(Tag::FocalLength),
        gps_lat: coord(Tag::GPSLatitude, Tag::GPSLatitudeRef, "S"),
        gps_lon: coord(Tag::GPSLongitude, Tag::GPSLongitudeRef, "W"),
        gps_alt: num(Tag::GPSAltitude).map(|a| {
            if num(Tag::GPSAltitudeRef) == Some(1.0) {
                -a
            } else {
                a
            }
        }),
        bytes_read: 0,
    }
}

/// The TIFF block of a HEIF file's `Exif` item, if it lies within `bytes`.
fn heif_exif_tiff(bytes: &[u8]) -> Option<&[u8]> {
    let meta = boxes(bytes).find(|(t, _)| t == b"meta")?.1;
    let children = meta.get(4..)?; // full box: version + flags
    let iinf = boxes(children).find(|(t, _)| t == b"iinf")?.1;
    let iloc = boxes(children).find(|(t, _)| t == b"iloc")?.1;
    let item = exif_item_id(iinf)?;
    let (offset, length) = item_extent(iloc, item)?;
    let data = bytes.get(offset..offset.checked_add(length)?)?;
    // Payload: a 4-byte offset to the TIFF header (skipping `Exif\0\0`), then the block.
    let skip = u32::from_be_bytes(data.get(..4)?.try_into().ok()?) as usize;
    data.get(4 + skip..)
}

/// ISO BMFF boxes in `data` as (type, body).
fn boxes(data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        let header = data.get(pos..pos + 8)?;
        let size32 = u32::from_be_bytes(header[..4].try_into().ok()?) as usize;
        let kind: [u8; 4] = header[4..8].try_into().ok()?;
        let (size, body_start) = match size32 {
            0 => (data.len() - pos, 8),
            1 => (
                u64::from_be_bytes(data.get(pos + 8..pos + 16)?.try_into().ok()?) as usize,
                16,
            ),
            n => (n, 8),
        };
        if size < body_start {
            return None;
        }
        // The last box may run past a prefix read: hand out what's there.
        let body = data.get(pos + body_start..(pos + size).min(data.len()))?;
        pos = pos.checked_add(size)?;
        Some((kind, body))
    })
}

struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn uint(&mut self, n: usize) -> Option<u64> {
        let (head, rest) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = rest;
        Some(head.iter().fold(0u64, |acc, &b| acc << 8 | b as u64))
    }
}

fn exif_item_id(iinf: &[u8]) -> Option<u64> {
    let mut c = Cursor(iinf);
    let version = c.uint(1)?;
    c.uint(3)?;
    c.uint(if version == 0 { 2 } else { 4 })?; // entry count
    boxes(c.0)
        .filter(|(t, _)| t == b"infe")
        .find_map(|(_, infe)| {
            let mut c = Cursor(infe);
            let version = c.uint(1)?;
            c.uint(3)?;
            if version < 2 {
                return None;
            }
            let id = c.uint(if version == 2 { 2 } else { 4 })?;
            c.uint(2)?; // protection index
            (c.0.get(..4)? == b"Exif").then_some(id)
        })
}

/// (file offset, length) of an item stored as a single extent in the file.
fn item_extent(iloc: &[u8], item: u64) -> Option<(usize, usize)> {
    let mut c = Cursor(iloc);
    let version = c.uint(1)?;
    c.uint(3)?;
    let sizes = c.uint(1)?;
    let (offset_size, length_size) = ((sizes >> 4) as usize, (sizes & 0xf) as usize);
    let sizes = c.uint(1)?;
    let base_offset_size = (sizes >> 4) as usize;
    let index_size = if version > 0 {
        (sizes & 0xf) as usize
    } else {
        0
    };
    let count = c.uint(if version < 2 { 2 } else { 4 })?;
    for _ in 0..count {
        let id = c.uint(if version < 2 { 2 } else { 4 })?;
        let construction = if version > 0 { c.uint(2)? & 0xf } else { 0 };
        c.uint(2)?; // data reference index
        let base = c.uint(base_offset_size)?;
        let extents = c.uint(2)?;
        let mut first = None;
        for _ in 0..extents {
            c.uint(index_size)?;
            let offset = c.uint(offset_size)?;
            let length = c.uint(length_size)?;
            first.get_or_insert((offset, length));
        }
        if id == item {
            // Only plain file offsets in one extent (construction 0), which is what cameras write.
            let (offset, length) = first?;
            return (construction == 0 && extents == 1)
                .then(|| ((base + offset) as usize, length as usize));
        }
    }
    None
}

/// `2026:10:23 14:05:33` → `2026-10-23T14:05:33`.
fn iso_date(exif: &str) -> Option<String> {
    let exif = exif.trim().trim_end_matches('\0');
    chrono::NaiveDateTime::parse_from_str(exif, "%Y:%m:%d %H:%M:%S")
        .ok()
        .map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string())
}

/// `SubsecTimeOriginal` is a fraction's digits: `5` = 500 ms, `123` = 123 ms.
fn subsec_ms(digits: &str) -> Option<i64> {
    let digits: String = digits.trim().chars().take(3).collect();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    format!("{digits:0<3}").parse().ok()
}

/// Whole-file fallback through ImageIO, for files the direct parsers don't understand.
#[cfg(target_os = "macos")]
mod imageio {
    use super::{ExifInfo, iso_date, subsec_ms};
    use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType};

    type Dict = CFRetained<CFDictionary<CFString, CFType>>;

    pub fn parse(bytes: &[u8]) -> Option<ExifInfo> {
        let props: Dict = crate::pv::heif::properties(bytes)?;
        let exif = sub(&props, "{Exif}");
        let tiff = sub(&props, "{TIFF}");
        let e = |key| exif.as_ref().and_then(|d| string(d, key));
        let en = |key| exif.as_ref().and_then(|d| number(d, key));
        Some(ExifInfo {
            date_original: e("DateTimeOriginal").as_deref().and_then(iso_date),
            subsec_ms: e("SubsecTimeOriginal").as_deref().and_then(subsec_ms),
            offset_original: e("OffsetTimeOriginal"),
            make: tiff.as_ref().and_then(|d| string(d, "Make")),
            model: tiff.as_ref().and_then(|d| string(d, "Model")),
            serial: e("BodySerialNumber"),
            lens: e("LensModel"),
            orientation: number(&props, "Orientation").map(|v| v as i64),
            width: number(&props, "PixelWidth").map(|v| v as i64),
            height: number(&props, "PixelHeight").map(|v| v as i64),
            exposure_s: en("ExposureTime"),
            f_number: en("FNumber"),
            focal_mm: en("FocalLength"),
            ..Default::default()
        })
    }

    fn get(dict: &Dict, key: &str) -> Option<CFRetained<CFType>> {
        dict.get(&CFString::from_str(key))
    }

    fn sub(dict: &Dict, key: &str) -> Option<Dict> {
        let d = get(dict, key)?.downcast::<CFDictionary>().ok()?;
        Some(unsafe { CFRetained::cast_unchecked(d) })
    }

    fn string(dict: &Dict, key: &str) -> Option<String> {
        let s = get(dict, key)?.downcast::<CFString>().ok()?.to_string();
        let s = s.trim().trim_end_matches('\0').to_string();
        (!s.is_empty()).then_some(s)
    }

    fn number(dict: &Dict, key: &str) -> Option<f64> {
        get(dict, key)?.downcast::<CFNumber>().ok()?.as_f64()
    }
}

#[cfg(not(target_os = "macos"))]
mod imageio {
    pub fn parse(_bytes: &[u8]) -> Option<super::ExifInfo> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_helpers() {
        assert_eq!(
            iso_date("2026:10:23 14:05:33").as_deref(),
            Some("2026-10-23T14:05:33")
        );
        assert_eq!(iso_date("0000:00:00 00:00:00"), None);
        assert_eq!(subsec_ms("5"), Some(500));
        assert_eq!(subsec_ms("123"), Some(123));
        assert_eq!(subsec_ms("0456"), Some(45));
        assert_eq!(subsec_ms(""), None);
    }

    /// `PV_FIXTURES=~/Desktop/2026-10-23_10261023 cargo test --lib pv::exif_read -- --nocapture`
    #[test]
    fn real_fixture_metadata_from_the_file_start() {
        let Ok(dir) = std::env::var("PV_FIXTURES") else {
            return;
        };
        let mut checked = 0;
        for entry in walkdir::WalkDir::new(dir) {
            let path = entry.unwrap().into_path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !["arw", "hif", "jpg"].contains(&ext.to_ascii_lowercase().as_str()) {
                continue;
            }
            let start = std::time::Instant::now();
            let info = read_exif(&path).unwrap();
            println!(
                "{} {:?} {:?}+{:?} {:?} {:?} iso {:?} {}x{} read {} KB",
                path.file_name().unwrap().to_string_lossy(),
                start.elapsed(),
                info.date_original,
                info.subsec_ms,
                info.model,
                info.lens,
                info.iso,
                info.width.unwrap_or(0),
                info.height.unwrap_or(0),
                info.bytes_read / 1024
            );
            assert!(info.date_original.is_some(), "{}", path.display());
            assert!(info.model.is_some());
            assert_eq!(
                info.bytes_read,
                PREFIX_BYTES,
                "{} needed the whole file",
                path.display()
            );
            checked += 1;
        }
        assert!(checked > 0);
    }
}
