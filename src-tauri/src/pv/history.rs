//! Edit history for style learning (docs/IMPLEMENTATION_PLAN.md M3-05, PRD §6.8): every finished
//! edit stores image features → final sliders + look, for the regressor of M6.
//!
//! An append-only JSON-lines file next to the catalog, not in it: the catalog is a cache that can
//! be rebuilt from the folders (A3), and this can't. Local only; the file can be copied or deleted.
//! Features describe the image *before* the edit (the RAW's camera preview, or a Photo's hidden
//! base). A re-edit appends again and a Revert appends a marker: the trainer takes the latest line
//! per Photo.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::Value;

use crate::pv::catalog::{Catalog, now_ms};
use crate::pv::signals::{self, ImageStats};
use crate::pv::{assets, lifecycle, locations};

pub const FILE_NAME: &str = "pairview-edit-history.jsonl";
const VERSION: u32 = 1;

/// Heavy or machine-specific recipe values a regressor has no use for.
const NOT_TRAINING_DATA: &[&str] = &["lutData", "aiPatches", "masks", "exif", "lutPath"];

pub fn file_next_to(catalog: &Path) -> PathBuf {
    catalog.with_file_name(FILE_NAME)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhotoKey {
    /// Survives a remount or a new mount path, as location identity does.
    volume_id: Option<String>,
    /// The Photo's grid file, from the volume root when known, else from the location root.
    path: String,
    stem: String,
    captured_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Exif {
    camera: Option<String>,
    lens: Option<String>,
    iso: Option<i64>,
    exposure_s: Option<f64>,
    f_number: Option<f64>,
    focal_mm: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Look {
    name: Option<String>,
    intensity: Option<f64>,
}

// Each line is built, written and dropped; there's no collection of them to keep small.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
enum Line {
    /// A finished edit.
    Edit {
        version: u32,
        at: i64,
        photo: PhotoKey,
        exif: Exif,
        features: Option<ImageStats>,
        sliders: Value,
        look: Look,
    },
    /// The edit was reverted: not something to learn from.
    Revert {
        version: u32,
        at: i64,
        photo: PhotoKey,
    },
}

fn photo_key(cat: &Catalog, photo_id: i64) -> Result<PhotoKey> {
    let (location_id, stem, captured_at): (i64, String, Option<String>) = cat.conn().query_row(
        "SELECT s.location_id, p.stem_key, p.captured_at
         FROM photos p JOIN folders s ON s.id = p.shoot_folder_id WHERE p.id = ?1",
        [photo_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let location = locations::get(cat, location_id)?;
    let file = assets::representative_path(cat, photo_id)?;
    let (volume_id, path) = match (&location, &file) {
        (Some(l), Some(f)) => {
            let rel = f.strip_prefix(&l.path).unwrap_or(f).to_string_lossy();
            let path = match l.volume_rel_path.as_deref().filter(|v| !v.is_empty()) {
                Some(prefix) => format!("{prefix}/{rel}"),
                None => rel.into_owned(),
            };
            (l.volume_id.clone(), path)
        }
        _ => (None, stem.clone()),
    };
    Ok(PhotoKey {
        volume_id,
        path: path.replace('\\', "/"),
        stem,
        captured_at,
    })
}

fn exif_of(cat: &Catalog, photo_id: i64) -> Result<Exif> {
    Ok(cat
        .conn()
        .query_row(
            "SELECT e.model, e.lens, e.iso, e.exposure_s, e.f_number, e.focal_mm
             FROM photo_members m JOIN exif e ON e.file_id = m.file_id
             WHERE m.photo_id = ?1 AND m.role IN ('raw', 'display')
             ORDER BY CASE m.role WHEN 'raw' THEN 0 ELSE 1 END LIMIT 1",
            [photo_id],
            |r| {
                Ok(Exif {
                    camera: r.get(0)?,
                    lens: r.get(1)?,
                    iso: r.get(2)?,
                    exposure_s: r.get(3)?,
                    f_number: r.get(4)?,
                    focal_mm: r.get(5)?,
                })
            },
        )
        .optional()?
        .unwrap_or_default())
}

/// The image the edit started from: the RAW (its camera preview), else the hidden base.
fn features(cat: &Catalog, photo_id: i64) -> Result<Option<ImageStats>> {
    let raw: Option<i64> = cat
        .conn()
        .query_row(
            "SELECT file_id FROM photo_members WHERE photo_id = ?1 AND role = 'raw' LIMIT 1",
            [photo_id],
            |r| r.get(0),
        )
        .optional()?;
    let source = match raw {
        Some(file) => Some(assets::file_path(cat, file)?),
        None => {
            let display: Option<i64> = cat.conn().query_row(
                "SELECT display_file_id FROM photos WHERE id = ?1",
                [photo_id],
                |r| r.get(0),
            )?;
            match display {
                Some(file) => {
                    let path = assets::file_path(cat, file)?;
                    Some(lifecycle::existing_base(&path).unwrap_or(path))
                }
                None => None,
            }
        }
    };
    Ok(source.and_then(|p| signals::stats_of(&p)))
}

fn trained_sliders(adjustments: &Value) -> Value {
    let mut sliders = adjustments.clone();
    if let Some(map) = sliders.as_object_mut() {
        for key in NOT_TRAINING_DATA {
            map.remove(*key);
        }
    }
    sliders
}

fn append(file: &Path, line: &Line) -> Result<()> {
    let mut text = serde_json::to_string(line)?;
    text.push('\n');
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?;
    // One write, so a crash can at worst lose or cut the last line, which the reader skips.
    out.write_all(text.as_bytes())?;
    Ok(())
}

/// Records a finished edit.
pub fn record_edit(file: &Path, cat: &Catalog, photo_id: i64, adjustments: &Value) -> Result<()> {
    let line = Line::Edit {
        version: VERSION,
        at: now_ms(),
        photo: photo_key(cat, photo_id)?,
        exif: exif_of(cat, photo_id)?,
        features: features(cat, photo_id)?,
        sliders: trained_sliders(adjustments),
        look: Look {
            name: adjustments
                .get("lutName")
                .and_then(Value::as_str)
                .map(str::to_string),
            intensity: adjustments.get("lutIntensity").and_then(Value::as_f64),
        },
    };
    append(file, &line)
}

/// Records that the Photo's edit was reverted.
pub fn record_revert(file: &Path, cat: &Catalog, photo_id: i64) -> Result<()> {
    let line = Line::Revert {
        version: VERSION,
        at: now_ms(),
        photo: photo_key(cat, photo_id)?,
    };
    append(file, &line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pv::scan::l0::{ScanOptions, scan_location};
    use image::{DynamicImage, Rgb, RgbImage};
    use serde_json::json;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, Catalog, i64, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap().join("DCIM");
        fs::create_dir_all(root.join("s")).unwrap();
        DynamicImage::ImageRgb8(RgbImage::from_pixel(64, 48, Rgb([250, 250, 250])))
            .save(root.join("s/DSC0001.JPG"))
            .unwrap();
        let mut cat = Catalog::open_in_memory().unwrap();
        let loc = locations::add(&cat, &root, None).unwrap();
        let loc = locations::get(&cat, loc).unwrap().unwrap();
        let scan = scan_location(&mut cat, &loc, &ScanOptions::default()).unwrap();
        assets::refresh(&mut cat, &scan.changed_folders).unwrap();
        let photo = cat
            .conn()
            .query_row("SELECT id FROM photos", [], |r| r.get(0))
            .unwrap();
        let history = tmp.path().join(FILE_NAME);
        (tmp, cat, photo, history)
    }

    fn lines(file: &Path) -> Vec<Value> {
        fs::read_to_string(file)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn an_edit_stores_features_sliders_and_look_without_the_heavy_parts() {
        let (_tmp, cat, photo, file) = fixture();
        let adjustments = json!({
            "exposure": 1.5,
            "lutName": "Classic Neg",
            "lutIntensity": 80,
            "lutData": "AAAA".repeat(1000),
            "masks": [{"bitmap": "..."}],
            "aiPatches": ["..."],
        });
        record_edit(&file, &cat, photo, &adjustments).unwrap();
        let l = &lines(&file)[0];
        assert_eq!(l["kind"], "edit");
        assert_eq!(l["sliders"]["exposure"], 1.5);
        assert!(l["sliders"].get("lutData").is_none());
        assert!(l["sliders"].get("masks").is_none());
        assert_eq!(l["look"]["name"], "Classic Neg");
        assert_eq!(l["look"]["intensity"], 80.0);
        assert_eq!(l["photo"]["stem"], "dsc0001");
        assert!(
            l["photo"]["path"]
                .as_str()
                .unwrap()
                .ends_with("s/DSC0001.JPG")
        );
        assert!(l["features"]["clipped"].as_f64().unwrap() > 0.5, "{l}");
    }

    #[test]
    fn lines_only_accumulate_and_a_revert_is_a_marker() {
        let (_tmp, cat, photo, file) = fixture();
        record_edit(&file, &cat, photo, &json!({"exposure": 1})).unwrap();
        record_edit(&file, &cat, photo, &json!({"exposure": 2})).unwrap();
        record_revert(&file, &cat, photo).unwrap();
        let all = lines(&file);
        assert_eq!(all.len(), 3);
        assert_eq!(all[1]["sliders"]["exposure"], 2);
        assert_eq!(all[2]["kind"], "revert");
        assert_eq!(all[2]["photo"]["stem"], "dsc0001");
    }

    #[test]
    fn it_sits_next_to_the_catalog_not_inside_it() {
        assert_eq!(
            file_next_to(Path::new("/app/pairview-catalog.sqlite3")),
            Path::new("/app").join(FILE_NAME)
        );
    }
}
