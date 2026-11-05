//! Grid thumbnails per Photo (docs/IMPLEMENTATION_PLAN.md M1-07, M0 finding 3).
//!
//! Which file a Photo's thumbnail comes from:
//! - unedited Photo with a RAW → the RAW's embedded JPEG preview (upstream's fast path reads the
//!   first 64 KB plus the preview's byte range, ~0.5–1 MB, instead of the whole ~40 MB ARW);
//! - edited Photo → its rendered display copy, which shows the edit;
//! - display-only Photo → the HIF/JPG (ImageIO thumbnail for HIF; it reads the whole ~10 MB file,
//!   so it's slower on the NAS).
//!
//! Generated thumbnails are cached by upstream's thumbnail cache, keyed by path + mtime +
//! adjustments. A Photo that becomes edited switches to a different file and so to a new key.

// The grid (M1-12) and viewer filmstrip (M1-14) are the first users.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::Result;
use image::DynamicImage;
use rusqlite::OptionalExtension;

use crate::pv::assets::{self, state::State};
use crate::pv::catalog::Catalog;

/// The file the grid shows for a Photo.
pub fn thumbnail_file(cat: &Catalog, photo_id: i64) -> Result<Option<PathBuf>> {
    thumbnail_file_id(cat, photo_id)?
        .map(|f| assets::file_path(cat, f))
        .transpose()
}

/// Catalog id of the file the grid shows: the RAW (its camera preview), or the rendered display
/// copy of an edited Photo, or the display copy when there's no RAW.
pub fn thumbnail_file_id(cat: &Catalog, photo_id: i64) -> Result<Option<i64>> {
    let row: Option<(String, Option<i64>, Option<i64>)> = cat
        .conn()
        .query_row(
            "SELECT state, representative_file_id, display_file_id FROM photos WHERE id = ?1",
            [photo_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((state, representative, display)) = row else {
        return Ok(None);
    };
    Ok(match State::parse(&state) {
        Some(State::Edited) => display.or(representative),
        _ => representative.or(display),
    })
}

/// A thumbnail from the cheap sources only (RAW preview, HEIF thumbnail, small JPEG), never a
/// full RAW develop. `None` means the caller needs the full pipeline.
pub fn quick_thumbnail(path: &Path, target_res: u32) -> Option<DynamicImage> {
    if crate::formats::is_raw_file(path) {
        crate::file_management::try_load_embedded_raw_preview(path, target_res)
    } else if crate::pv::heif::is_heif_file(path) {
        let mmap = crate::file_management::read_file_mapped(path).ok()?;
        crate::pv::heif::heif_thumbnail(&mmap, target_res).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pv::locations;
    use crate::pv::scan::l0::{ScanOptions, scan_location};
    use std::fs;
    use std::time::Instant;

    fn names(cat: &Catalog) -> Vec<(String, String)> {
        let ids: Vec<(i64, String)> = cat
            .conn()
            .prepare("SELECT id, stem_key FROM photos ORDER BY stem_key")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        ids.into_iter()
            .map(|(id, stem)| {
                let path = thumbnail_file(cat, id).unwrap().unwrap();
                (
                    stem,
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn picks_the_preview_source_per_photo_state() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("DCIM");
        for f in [
            "a/arw/DSC00001.ARW",
            "a/hif/DSC00001.HIF",
            "a/arw/DSC00002.ARW",
            "a/hif/DSC00002.HIF",
            "a/hif/DSC00003.HIF",
            "a/arw/DSC00004.ARW",
        ] {
            let p = root.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, b"x").unwrap();
        }
        let mut cat = Catalog::open_in_memory().unwrap();
        let id = locations::add(&cat, &root, None).unwrap();
        let loc = locations::get(&cat, id).unwrap().unwrap();
        let scan = scan_location(&mut cat, &loc, &ScanOptions::default()).unwrap();
        assets::refresh(&mut cat, &scan.changed_folders).unwrap();
        cat.conn()
            .execute(
                "UPDATE photos SET state = 'edited' WHERE stem_key = 'dsc00002'",
                [],
            )
            .unwrap();

        assert_eq!(
            names(&cat),
            [
                ("dsc00001".into(), "DSC00001.ARW".into()),
                ("dsc00002".into(), "DSC00002.HIF".into()),
                ("dsc00003".into(), "DSC00003.HIF".into()),
                ("dsc00004".into(), "DSC00004.ARW".into()),
            ]
        );
        assert_eq!(thumbnail_file(&cat, 999).unwrap(), None);
    }

    /// `PV_FIXTURES=~/Desktop/2026-10-23_10261023 cargo test --lib pv::thumbs -- --nocapture`
    #[test]
    fn real_fixture_thumbnails_from_previews() {
        let Ok(dir) = std::env::var("PV_FIXTURES") else {
            return;
        };
        let mut cat = Catalog::open_in_memory().unwrap();
        let id = locations::add(&cat, Path::new(&dir), None).unwrap();
        let loc = locations::get(&cat, id).unwrap().unwrap();
        let scan = scan_location(&mut cat, &loc, &ScanOptions::default()).unwrap();
        assets::refresh(&mut cat, &scan.changed_folders).unwrap();
        let photos: Vec<i64> = cat
            .conn()
            .prepare("SELECT id FROM photos")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();

        let start = Instant::now();
        for photo in &photos {
            let path = thumbnail_file(&cat, *photo).unwrap().unwrap();
            let t = Instant::now();
            let thumb = quick_thumbnail(&path, 1280).expect("fast thumbnail");
            println!(
                "{} {}x{} in {:?}",
                path.file_name().unwrap().to_string_lossy(),
                thumb.width(),
                thumb.height(),
                t.elapsed()
            );
            assert!(thumb.width().max(thumb.height()) >= 1216);
            // The HIF of the same Photo has the same orientation.
            let hif = path
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("hif")
                .join(path.with_extension("HIF").file_name().unwrap());
            if hif.exists() {
                let reference = quick_thumbnail(&hif, 256).unwrap();
                assert_eq!(
                    thumb.width() > thumb.height(),
                    reference.width() > reference.height(),
                    "orientation of {}",
                    path.display()
                );
            }
        }
        println!("{} thumbnails in {:?}", photos.len(), start.elapsed());
    }
}
