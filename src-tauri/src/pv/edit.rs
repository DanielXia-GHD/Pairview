//! Done: the edit loop's last step (docs/IMPLEMENTATION_PLAN.md M3-02, PRD §6.4). 🛡
//!
//! RapidRAW renders the RAW + recipe at full resolution; this encodes the result in the display
//! copy's own format (HIF → 10-bit HEIF, JPG → JPG q92), copying its metadata, checks the encoded
//! file by decoding it again, and only then asks `pv/lifecycle` to swap it in. The camera original
//! goes to Recently Deleted. RAW + recipe stay the master, so every re-edit starts from the RAW.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail, ensure};
use image::{DynamicImage, GenericImageView};

use crate::pv::catalog::{Catalog, now_ms};
use crate::pv::lifecycle::{self, Replaced};
use crate::pv::{assets, metadata, scan::l1};

/// HEIF quality: what M0 measured as matching the camera's file size.
const HEIF_QUALITY: f32 = 0.9;
/// PRD §6.4.
const JPEG_QUALITY: u8 = 92;

/// What Done needs before the expensive render: the Photo's display copy, and that it has a RAW
/// (a display-only Photo needs the hidden base of M3-03 first).
pub fn display_to_replace(cat: &Catalog, photo_id: i64) -> Result<PathBuf> {
    let (display, has_raw): (Option<i64>, bool) = cat.conn().query_row(
        "SELECT p.display_file_id,
                EXISTS (SELECT 1 FROM photo_members m WHERE m.photo_id = p.id AND m.role = 'raw')
         FROM photos p WHERE p.id = ?1",
        [photo_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let display = display.ok_or_else(|| anyhow!("this Photo has no display copy to replace"))?;
    ensure!(
        has_raw,
        "editing a Photo without a RAW isn't available yet: its only copy would be replaced"
    );
    let path = assets::file_path(cat, display)?;
    ensure!(
        output_format(&path).is_some(),
        "{} can't be written back in its own format",
        path.display()
    );
    Ok(path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Heif,
    Jpeg,
}

fn output_format(path: &Path) -> Option<Format> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "hif" | "heif" | "heic" => Some(Format::Heif),
        "jpg" | "jpeg" => Some(Format::Jpeg),
        _ => None,
    }
}

/// `image` in the format of `display`, with `display`'s metadata.
pub fn encode_like(display: &Path, image: &DynamicImage) -> Result<Vec<u8>> {
    match output_format(display) {
        Some(Format::Heif) => {
            let original = std::fs::read(display)?;
            crate::pv::heif::encode_heif(image, Some(&original), HEIF_QUALITY)
        }
        Some(Format::Jpeg) => {
            let mut bytes = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
                .encode_image(&image.to_rgb8())?;
            let xmp = crate::exif_processing::ExportXmp::from_metadata(&Default::default());
            crate::exif_processing::write_image_with_metadata(
                &mut bytes,
                &display.to_string_lossy(),
                "jpg",
                true,
                false,
                &xmp,
                None,
            )
            .map_err(|e| anyhow!("cannot copy metadata from {}: {e}", display.display()))?;
            Ok(bytes)
        }
        None => bail!("{} isn't a HIF or JPG", display.display()),
    }
}

/// Decodes the encoded file and checks it is the size that was rendered.
pub fn verify(display: &Path, bytes: &[u8], rendered: (u32, u32)) -> Result<()> {
    let decoded = match output_format(display) {
        Some(Format::Heif) => crate::pv::heif::decode_heif(bytes)?,
        _ => image::load_from_memory(bytes)?,
    };
    ensure!(
        decoded.dimensions() == rendered,
        "the encoded file is {:?}, the render was {rendered:?}; nothing was changed",
        decoded.dimensions()
    );
    Ok(())
}

/// The file half of Done, after the render: encode, verify, swap, mark ✎, read its EXIF again.
pub fn finish(cat: &mut Catalog, photo_id: i64, rendered: &DynamicImage) -> Result<Replaced> {
    let display = display_to_replace(cat, photo_id)?;
    let bytes = encode_like(&display, rendered)?;
    verify(&display, &bytes, rendered.dimensions())?;
    let replaced = lifecycle::replace_display(cat, photo_id, &bytes)?;

    // The swap is done; what follows only keeps the record straight.
    if let Err(e) = metadata::mark_edited(cat, photo_id, Some(now_ms())) {
        log::warn!("Pairview: Photo {photo_id}: edited marker not written: {e:#}");
    }
    let shoot: Result<i64, _> = cat.conn().query_row(
        "SELECT shoot_folder_id FROM photos WHERE id = ?1",
        [photo_id],
        |r| r.get(0),
    );
    if let Ok(shoot) = shoot
        && let Err(e) = l1::run(cat, &[shoot])
    {
        log::warn!("Pairview: Photo {photo_id}: EXIF not re-read: {e:#}");
    }
    Ok(replaced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pv::locations;
    use crate::pv::scan::l0::{ScanOptions, scan_location};
    use image::{Rgb, RgbImage};
    use std::fs;

    fn picture(w: u32, h: u32, shade: u8) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, Rgb([shade, shade / 2, 40])))
    }

    fn fixture(display_name: &str, display: &[u8]) -> (tempfile::TempDir, Catalog, i64, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap().join("DCIM");
        fs::create_dir_all(root.join("s/arw")).unwrap();
        fs::create_dir_all(root.join("s/hif")).unwrap();
        fs::write(root.join("s/arw/DSC0001.ARW"), b"raw").unwrap();
        let path = root.join("s/hif").join(display_name);
        fs::write(&path, display).unwrap();
        let mut cat = Catalog::open_in_memory().unwrap();
        let loc = locations::add(&cat, &root, None).unwrap();
        let loc = locations::get(&cat, loc).unwrap().unwrap();
        let scan = scan_location(&mut cat, &loc, &ScanOptions::default()).unwrap();
        assets::refresh(&mut cat, &scan.changed_folders).unwrap();
        let photo = cat
            .conn()
            .query_row("SELECT id FROM photos", [], |r| r.get(0))
            .unwrap();
        (tmp, cat, photo, path)
    }

    fn state(cat: &Catalog, photo: i64) -> String {
        cat.conn()
            .query_row("SELECT state FROM photos WHERE id = ?1", [photo], |r| {
                r.get(0)
            })
            .unwrap()
    }

    fn jpeg(image: &DynamicImage) -> Vec<u8> {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
            .encode_image(&image.to_rgb8())
            .unwrap();
        out
    }

    #[test]
    fn done_on_a_jpg_replaces_it_with_a_jpg_of_the_render() {
        let original = jpeg(&picture(64, 48, 200));
        let (_tmp, mut cat, photo, path) = fixture("DSC0001.JPG", &original);
        let render = picture(128, 96, 60);
        let r = finish(&mut cat, photo, &render).unwrap();
        let now = image::load_from_memory(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(now.dimensions(), (128, 96));
        assert_eq!(state(&cat, photo), "edited");
        // The camera original is in Recently Deleted, untouched; the RAW never moved.
        let root = path.parent().unwrap().parent().unwrap().parent().unwrap();
        let kept = root
            .join(lifecycle::TRASH_DIR)
            .join(&r.op_id)
            .join("s/hif/DSC0001.JPG");
        assert_eq!(fs::read(kept).unwrap(), original);
        assert!(root.join("s/arw/DSC0001.ARW").exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn done_on_a_hif_writes_ten_bit_heif() {
        let original =
            crate::pv::heif::encode_heif(&picture(64, 48, 200), None, HEIF_QUALITY).unwrap();
        let (_tmp, mut cat, photo, path) = fixture("DSC0001.HIF", &original);
        finish(&mut cat, photo, &picture(128, 96, 60)).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(
            bytes.get(4..8),
            Some(&b"ftyp"[..]),
            "HEIF, not something else"
        );
        assert_eq!(
            crate::pv::heif::decode_heif(&bytes).unwrap().dimensions(),
            (128, 96)
        );
        assert_eq!(state(&cat, photo), "edited");
    }

    #[test]
    fn a_bad_encode_changes_nothing() {
        let original = jpeg(&picture(64, 48, 200));
        let path = Path::new("DSC0001.JPG");
        assert!(verify(path, &original, (128, 96)).is_err(), "wrong size");
        assert!(verify(path, b"not an image", (64, 48)).is_err(), "garbage");
        assert!(verify(path, &original, (64, 48)).is_ok());
    }

    #[test]
    fn display_only_and_other_formats_are_refused_before_rendering() {
        let original = jpeg(&picture(64, 48, 200));
        let (_tmp, cat, photo, path) = fixture("DSC0001.JPG", &original);
        assert!(display_to_replace(&cat, photo).is_ok());
        fs::remove_file(
            path.parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("arw/DSC0001.ARW"),
        )
        .unwrap();
        cat.conn()
            .execute("DELETE FROM files WHERE name = 'DSC0001.ARW'", [])
            .unwrap();
        let err = display_to_replace(&cat, photo).unwrap_err();
        assert!(err.to_string().contains("without a RAW"), "{err}");
        assert_eq!(output_format(Path::new("x.png")), None);
    }

    /// Real camera file: `PV_FIXTURES=~/Desktop/2026-10-23_10261023 cargo test --lib pv::edit::real -- --nocapture`
    #[cfg(target_os = "macos")]
    #[test]
    fn real_hif_keeps_its_metadata_through_done() {
        let Ok(source) = std::env::var("PV_FIXTURES") else {
            return;
        };
        let hif = PathBuf::from(source).join("hif/DSC03534.HIF");
        let original = fs::read(&hif).unwrap();
        let before = crate::pv::exif_read::read_exif(&hif).unwrap();
        let decoded = crate::pv::heif::decode_heif(&original).unwrap();
        let (_tmp, mut cat, photo, path) = fixture("DSC0001.HIF", &original);
        let t = std::time::Instant::now();
        finish(&mut cat, photo, &decoded).unwrap();
        println!(
            "encode+verify+swap {:.2} s, {} MB → {} MB",
            t.elapsed().as_secs_f64(),
            original.len() / 1_000_000,
            fs::metadata(&path).unwrap().len() / 1_000_000
        );
        let after = crate::pv::exif_read::read_exif(&path).unwrap();
        assert_eq!(after.date_original, before.date_original);
        assert_eq!(after.model, before.model);
        assert_eq!(after.iso, before.iso);
        assert_eq!(after.lens, before.lens);
        assert_eq!(
            crate::pv::heif::decode_heif(&fs::read(&path).unwrap())
                .unwrap()
                .dimensions(),
            decoded.dimensions()
        );
    }
}
