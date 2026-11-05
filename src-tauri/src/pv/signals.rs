//! RAW-value signals (docs/IMPLEMENTATION_PLAN.md M2-07, PRD §6.5 Stage 3): does this Photo's RAW
//! hold something the display copy can't, or is the HIF enough?
//!
//! Image statistics come from the file the grid shows (the RAW's embedded camera preview, or the
//! rendered HIF of an edited Photo): the same pixels the camera's HIF has, at 512 px, read the
//! cheap way. They're cached in `raw_signals` per source file version. The verdict is derived on
//! request from those statistics plus EXIF ISO, rating, flag, edit state, the Keep-RAW lock and the
//! shoot intent, so a new rating or lock counts at once.
//!
//! This only *recommends*. Nothing here moves a file; slimming needs the review screen (M2-09).

use std::path::Path;

use anyhow::Result;
use image::DynamicImage;
use rayon::prelude::*;
use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::pv::catalog::{Catalog, now_ms};

/// Long edge the statistics are computed at.
const STATS_PX: u32 = 512;
const PARALLEL: usize = 8;

/// A pixel channel at or above this counts as clipped.
const CLIP_LEVEL: u8 = 250;
/// Luma at or below this counts as near black.
const SHADOW_LEVEL: u8 = 10;

/// Thresholds (PRD §6.5 table).
pub const CLIPPED_MIN: f32 = 0.005;
pub const SHADOWS_MIN: f32 = 0.15;
pub const HIGH_ISO: i64 = 3200;
/// Gray-world casts. Warm-toned rooms already measure 1.7–2.0 after the camera's AWB, so only a
/// strong, uncorrected cast (tungsten left orange, deep blue shade) counts.
const WARM: f32 = 2.3;
const COOL: f32 = 0.55;
const GREEN: f32 = 1.15;
const MAGENTA: f32 = 0.87;
const UNTOUCHED_DAYS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ImageStats {
    pub clipped: f32,
    pub shadows: f32,
    pub luma_low: u8,
    pub luma_high: u8,
    pub red_blue: f32,
    pub green_magenta: f32,
}

/// Statistics of an image (any size; it is reduced to `STATS_PX` first).
pub fn image_stats(image: &DynamicImage) -> ImageStats {
    let small = if image.width().max(image.height()) > STATS_PX {
        image.thumbnail(STATS_PX, STATS_PX)
    } else {
        image.clone()
    };
    let rgb = small.to_rgb8();
    let n = (rgb.width() * rgb.height()).max(1) as f32;
    let mut lumas = [0u32; 256];
    let (mut clipped, mut shadows) = (0u32, 0u32);
    let (mut r_sum, mut g_sum, mut b_sum, mut mid) = (0f64, 0f64, 0f64, 0u32);
    for p in rgb.pixels() {
        let [r, g, b] = p.0;
        let luma = ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8;
        lumas[luma as usize] += 1;
        if r.max(g).max(b) >= CLIP_LEVEL {
            clipped += 1;
        }
        if luma <= SHADOW_LEVEL {
            shadows += 1;
        }
        if (40..=220).contains(&luma) {
            r_sum += r as f64;
            g_sum += g as f64;
            b_sum += b as f64;
            mid += 1;
        }
    }
    let percentile = |q: f32| -> u8 {
        let target = (q * n) as u32;
        let mut seen = 0;
        for (v, &c) in lumas.iter().enumerate() {
            seen += c;
            if seen > target {
                return v as u8;
            }
        }
        255
    };
    let (red_blue, green_magenta) = if mid > 0 {
        let (r, g, b) = (r_sum.max(1.0), g_sum.max(1.0), b_sum.max(1.0));
        ((r / b) as f32, (g / ((r + b) / 2.0)) as f32)
    } else {
        (1.0, 1.0)
    };
    ImageStats {
        clipped: clipped as f32 / n,
        shadows: shadows as f32 / n,
        luma_low: percentile(0.01),
        luma_high: percentile(0.99),
        red_blue,
        green_magenta,
    }
}

// ---- Verdict ----------------------------------------------------------------------------------

/// Shoot intent (PRD §6.5 Stage 1): set by hand in M2-08, guessed by AI in M4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Intent {
    Casual,
    #[default]
    Normal,
    Important,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    KeepRaw,
    YourCall,
    HifEnough,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Toward {
    Keep,
    Drop,
}

/// One reason chip. `value` fills the chip's text (percent, ISO, stars).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chip {
    pub kind: &'static str,
    pub toward: Toward,
    pub value: Option<f64>,
}

impl Chip {
    fn keep(kind: &'static str, value: Option<f64>) -> Chip {
        Chip {
            kind,
            toward: Toward::Keep,
            value,
        }
    }

    fn drop(kind: &'static str) -> Chip {
        Chip {
            kind,
            toward: Toward::Drop,
            value: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawValue {
    pub photo_id: i64,
    pub verdict: Verdict,
    /// The owner locked it (`K`): rules never change it.
    pub locked: bool,
    /// Nobody rated, flagged or locked it: the verdict rests on the heuristics alone.
    pub unconfirmed: bool,
    pub chips: Vec<Chip>,
    /// No statistics (no previewable file); the verdict uses the rest.
    pub no_image: bool,
}

/// Everything the verdict looks at for one Photo.
#[derive(Debug, Clone, Default)]
pub struct Inputs {
    pub stats: Option<ImageStats>,
    pub iso: Option<i64>,
    pub rating: i64,
    pub flag: String,
    pub keep_raw: String,
    pub edited: bool,
    pub has_raw: bool,
    pub days_since_capture: Option<i64>,
    pub intent: Intent,
}

/// The verdict and its reasons (PRD §6.5). Strong keep signals: rated ★★★+, Pick, edited.
/// Weak ones come from the image and EXIF: one makes it "Your call", two "Keep RAW".
pub fn verdict(photo_id: i64, i: &Inputs) -> RawValue {
    let mut chips = Vec::new();
    let mut strong = 0;
    let mut weak = 0;

    if i.rating >= 3 {
        chips.push(Chip::keep("rated", Some(i.rating as f64)));
        strong += 1;
    }
    if i.flag == "pick" {
        chips.push(Chip::keep("picked", None));
        strong += 1;
    }
    if i.edited {
        chips.push(Chip::keep("edited", None));
        strong += 1;
    }
    if let Some(s) = &i.stats {
        if s.clipped >= CLIPPED_MIN {
            chips.push(Chip::keep("highlightsClipped", Some(pct(s.clipped))));
            weak += 1;
        }
        if s.shadows >= SHADOWS_MIN {
            chips.push(Chip::keep("shadowsCrushed", Some(pct(s.shadows))));
            weak += 1;
        }
        if s.luma_low <= 6 && s.luma_high >= 249 {
            chips.push(Chip::keep("highContrast", None));
            weak += 1;
        }
        let wb = if s.red_blue >= WARM {
            Some("warmLight")
        } else if s.red_blue <= COOL {
            Some("coolLight")
        } else if s.green_magenta >= GREEN {
            Some("greenTint")
        } else if s.green_magenta <= MAGENTA {
            Some("magentaTint")
        } else {
            None
        };
        if let Some(kind) = wb {
            chips.push(Chip::keep(kind, None));
            weak += 1;
        }
    }
    if let Some(iso) = i.iso.filter(|&iso| iso >= HIGH_ISO) {
        chips.push(Chip::keep("highIso", Some(iso as f64)));
        weak += 1;
    }
    if i.intent == Intent::Important {
        chips.push(Chip::keep("importantShoot", None));
    }
    let rejected = i.flag == "reject";
    if rejected {
        chips.push(Chip::drop("rejected"));
    }
    let untouched = i.rating == 0 && i.flag == "none" && !i.edited;
    if untouched && i.days_since_capture.is_some_and(|d| d >= UNTOUCHED_DAYS) {
        chips.push(Chip::drop("untouched"));
    }
    if i.intent == Intent::Casual {
        chips.push(Chip::drop("casualShoot"));
    }
    if strong == 0 && weak == 0 && !rejected && i.stats.is_some() {
        chips.push(Chip::drop("wellExposed"));
    }

    let locked = matches!(i.keep_raw.as_str(), "keep" | "drop");
    let verdict = match i.keep_raw.as_str() {
        "keep" => Verdict::KeepRaw,
        "drop" => Verdict::HifEnough,
        _ if rejected => Verdict::HifEnough,
        _ if strong > 0 => Verdict::KeepRaw,
        _ => match i.intent {
            Intent::Important => Verdict::KeepRaw,
            Intent::Normal if weak >= 2 => Verdict::KeepRaw,
            Intent::Normal if weak == 1 => Verdict::YourCall,
            Intent::Casual if weak >= 2 => Verdict::YourCall,
            _ => Verdict::HifEnough,
        },
    };
    RawValue {
        photo_id,
        verdict,
        locked,
        unconfirmed: !locked && untouched,
        chips,
        no_image: i.stats.is_none() && i.has_raw,
    }
}

fn pct(share: f32) -> f64 {
    ((share as f64) * 1000.0).round() / 10.0
}

// ---- Catalog ----------------------------------------------------------------------------------

/// Computes missing or outdated statistics for `photo_ids` (8 files at a time) and stores them.
/// Returns how many were computed. Files that can't be read are skipped (tried again next time).
pub fn ensure_stats(cat: &mut Catalog, photo_ids: &[i64]) -> Result<usize> {
    let mut todo: Vec<(i64, i64, i64, std::path::PathBuf)> = Vec::new();
    for &id in photo_ids {
        let Some(file_id) = crate::pv::thumbs::thumbnail_file_id(cat, id)? else {
            continue;
        };
        let mtime: i64 =
            cat.conn()
                .query_row("SELECT mtime FROM files WHERE id = ?1", [file_id], |r| {
                    r.get(0)
                })?;
        let path = crate::pv::assets::file_path(cat, file_id)?;
        let cached: Option<(i64, i64)> = cat
            .conn()
            .query_row(
                "SELECT source_file_id, source_mtime FROM raw_signals WHERE photo_id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if cached != Some((file_id, mtime)) {
            todo.push((id, file_id, mtime, path));
        }
    }
    if todo.is_empty() {
        return Ok(0);
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(PARALLEL)
        .build()?;
    let computed: Vec<(i64, i64, i64, ImageStats)> = pool.install(|| {
        todo.par_iter()
            .filter_map(|(id, file, mtime, path)| stats_of(path).map(|s| (*id, *file, *mtime, s)))
            .collect()
    });
    let tx = cat.conn_mut().transaction()?;
    let now = now_ms();
    for (id, file, mtime, s) in &computed {
        tx.execute(
            "INSERT OR REPLACE INTO raw_signals (photo_id, source_file_id, source_mtime, clipped,
                 shadows, luma_low, luma_high, red_blue, green_magenta, computed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                id,
                file,
                mtime,
                s.clipped,
                s.shadows,
                s.luma_low,
                s.luma_high,
                s.red_blue,
                s.green_magenta,
                now
            ],
        )?;
    }
    tx.commit()?;
    Ok(computed.len())
}

fn stats_of(path: &Path) -> Option<ImageStats> {
    let image =
        crate::pv::thumbs::quick_thumbnail(path, STATS_PX).or_else(|| image::open(path).ok())?;
    Some(image_stats(&image))
}

/// shoot, state, flag, keep_raw, captured_at, rating, has RAW.
type MetaRow = (i64, String, String, String, Option<String>, i64, bool);

/// Verdicts for `photo_ids` from cached statistics (call `ensure_stats` first) and current
/// metadata. `intent_of` gives each Photo's shoot intent.
pub fn raw_values(
    cat: &Catalog,
    photo_ids: &[i64],
    intent_of: impl Fn(i64) -> Intent,
) -> Result<Vec<RawValue>> {
    let today = chrono::Local::now().date_naive();
    let mut out = Vec::with_capacity(photo_ids.len());
    for &id in photo_ids {
        let row: Option<MetaRow> = cat
            .conn()
            .query_row(
                "SELECT p.shoot_folder_id, p.state, COALESCE(md.flag, 'none'),
                        COALESCE(md.keep_raw, 'auto'), p.captured_at, COALESCE(md.rating, 0),
                        EXISTS (SELECT 1 FROM photo_members m WHERE m.photo_id = p.id AND m.role = 'raw')
                 FROM photos p LEFT JOIN metadata md ON md.photo_id = p.id WHERE p.id = ?1",
                [id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((shoot, state, flag, keep_raw, captured_at, rating, has_raw)) = row else {
            continue;
        };
        let stats = cat
            .conn()
            .query_row(
                "SELECT clipped, shadows, luma_low, luma_high, red_blue, green_magenta
                 FROM raw_signals WHERE photo_id = ?1",
                [id],
                |r| {
                    Ok(ImageStats {
                        clipped: r.get(0)?,
                        shadows: r.get(1)?,
                        luma_low: r.get(2)?,
                        luma_high: r.get(3)?,
                        red_blue: r.get(4)?,
                        green_magenta: r.get(5)?,
                    })
                },
            )
            .optional()?;
        // ISO from the RAW's EXIF, else the display copy's.
        let iso: Option<i64> = cat
            .conn()
            .query_row(
                "SELECT e.iso FROM photo_members m JOIN exif e ON e.file_id = m.file_id
                 WHERE m.photo_id = ?1 AND m.role IN ('raw', 'display') AND e.iso IS NOT NULL
                 ORDER BY CASE m.role WHEN 'raw' THEN 0 ELSE 1 END LIMIT 1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        let days_since_capture = captured_at
            .as_deref()
            .and_then(|c| chrono::NaiveDate::parse_from_str(c.get(..10)?, "%Y-%m-%d").ok())
            .map(|d| (today - d).num_days());
        out.push(verdict(
            id,
            &Inputs {
                stats,
                iso,
                rating,
                flag,
                keep_raw,
                edited: state == "edited",
                has_raw,
                days_since_capture,
                intent: intent_of(shoot),
            },
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn flat(r: u8, g: u8, b: u8) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(64, 64, Rgb([r, g, b])))
    }

    fn calm() -> ImageStats {
        image_stats(&flat(120, 120, 120))
    }

    #[test]
    fn stats_of_flat_and_clipped_images() {
        let s = calm();
        assert_eq!((s.clipped, s.shadows), (0.0, 0.0));
        assert!((s.red_blue - 1.0).abs() < 0.01 && (s.green_magenta - 1.0).abs() < 0.01);

        let mut img = RgbImage::from_pixel(100, 100, Rgb([120, 120, 120]));
        for x in 0..100 {
            for y in 0..10 {
                img.put_pixel(x, y, Rgb([255, 255, 255]));
            }
            for y in 90..100 {
                img.put_pixel(x, y, Rgb([0, 0, 0]));
            }
        }
        let s = image_stats(&DynamicImage::ImageRgb8(img));
        assert!((s.clipped - 0.10).abs() < 0.01, "{s:?}");
        assert!((s.shadows - 0.10).abs() < 0.01, "{s:?}");
        assert_eq!((s.luma_low, s.luma_high), (0, 255));

        let warm = image_stats(&flat(220, 120, 80));
        assert!(warm.red_blue > WARM);
    }

    fn inputs(stats: ImageStats) -> Inputs {
        Inputs {
            stats: Some(stats),
            flag: "none".into(),
            keep_raw: "auto".into(),
            has_raw: true,
            days_since_capture: Some(3),
            ..Default::default()
        }
    }

    fn kinds(v: &RawValue) -> Vec<&'static str> {
        v.chips.iter().map(|c| c.kind).collect()
    }

    #[test]
    fn a_calm_well_exposed_photo_is_hif_enough() {
        let v = verdict(1, &inputs(calm()));
        assert_eq!(v.verdict, Verdict::HifEnough);
        assert_eq!(kinds(&v), ["wellExposed"]);
        assert!(v.unconfirmed && !v.locked);
    }

    #[test]
    fn one_weak_signal_is_your_call_two_keep() {
        let mut i = inputs(calm());
        i.iso = Some(6400);
        let v = verdict(1, &i);
        assert_eq!(v.verdict, Verdict::YourCall);
        assert_eq!(v.chips[0].value, Some(6400.0));
        i.stats = Some(ImageStats {
            clipped: 0.03,
            ..calm()
        });
        let v = verdict(1, &i);
        assert_eq!(v.verdict, Verdict::KeepRaw);
        assert_eq!(kinds(&v), ["highlightsClipped", "highIso"]);
        assert_eq!(v.chips[0].value, Some(3.0));
    }

    #[test]
    fn human_input_wins() {
        let mut i = inputs(calm());
        i.rating = 4;
        assert_eq!(verdict(1, &i).verdict, Verdict::KeepRaw);
        assert!(!verdict(1, &i).unconfirmed);

        let mut i = inputs(ImageStats {
            clipped: 0.2,
            shadows: 0.3,
            ..calm()
        });
        i.flag = "reject".into();
        assert_eq!(verdict(1, &i).verdict, Verdict::HifEnough);

        i.keep_raw = "keep".into();
        let v = verdict(1, &i);
        assert_eq!((v.verdict, v.locked), (Verdict::KeepRaw, true));

        let mut i = inputs(calm());
        i.rating = 5;
        i.keep_raw = "drop".into();
        assert_eq!(verdict(1, &i).verdict, Verdict::HifEnough);
    }

    #[test]
    fn shoot_intent_moves_the_line() {
        let mut i = inputs(calm());
        i.intent = Intent::Important;
        assert_eq!(verdict(1, &i).verdict, Verdict::KeepRaw);
        i.intent = Intent::Casual;
        i.iso = Some(6400);
        i.stats = Some(ImageStats {
            shadows: 0.4,
            ..calm()
        });
        assert_eq!(verdict(1, &i).verdict, Verdict::YourCall);
        i.rating = 3;
        assert_eq!(verdict(1, &i).verdict, Verdict::KeepRaw);
    }

    #[test]
    fn untouched_after_30_days_leans_drop() {
        let mut i = inputs(calm());
        i.days_since_capture = Some(45);
        assert!(kinds(&verdict(1, &i)).contains(&"untouched"));
        i.rating = 1;
        assert!(!kinds(&verdict(1, &i)).contains(&"untouched"));
    }

    #[test]
    fn computes_and_caches_stats_from_real_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap().join("DCIM");
        std::fs::create_dir_all(root.join("s")).unwrap();
        flat(250, 250, 250)
            .save(root.join("s/DSC0001.JPG"))
            .unwrap();
        flat(110, 115, 120)
            .save(root.join("s/DSC0002.JPG"))
            .unwrap();
        let mut cat = Catalog::open_in_memory().unwrap();
        let loc = crate::pv::locations::add(&cat, &root, None).unwrap();
        let loc = crate::pv::locations::get(&cat, loc).unwrap().unwrap();
        let scan = crate::pv::scan::l0::scan_location(
            &mut cat,
            &loc,
            &crate::pv::scan::l0::ScanOptions::default(),
        )
        .unwrap();
        crate::pv::assets::refresh(&mut cat, &scan.changed_folders).unwrap();
        let ids: Vec<i64> = {
            let mut s = cat
                .conn()
                .prepare("SELECT id FROM photos ORDER BY stem_key")
                .unwrap();
            s.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(ensure_stats(&mut cat, &ids).unwrap(), 2);
        assert_eq!(ensure_stats(&mut cat, &ids).unwrap(), 0, "cached");
        let values = raw_values(&cat, &ids, |_| Intent::Normal).unwrap();
        assert!(
            kinds(&values[0]).contains(&"highlightsClipped"),
            "{values:?}"
        );
        assert_eq!(values[1].verdict, Verdict::HifEnough);
    }
}

/// `PV_FIXTURES=~/Desktop/2026-10-23_10261023 cargo test --lib pv::signals::real -- --nocapture`
#[cfg(test)]
#[test]
fn real_shoot_verdicts() {
    let Ok(source) = std::env::var("PV_FIXTURES") else {
        return;
    };
    let root = std::path::PathBuf::from(source);
    assert!(!root.starts_with("/Volumes"));
    let mut cat = Catalog::open_in_memory().unwrap();
    let loc = crate::pv::locations::add(&cat, &root, None).unwrap();
    let loc = crate::pv::locations::get(&cat, loc).unwrap().unwrap();
    let scan = crate::pv::scan::l0::scan_location(
        &mut cat,
        &loc,
        &crate::pv::scan::l0::ScanOptions::default(),
    )
    .unwrap();
    crate::pv::assets::refresh(&mut cat, &scan.changed_folders).unwrap();
    let shoots: Vec<i64> = vec![scan.changed_folders[0]];
    let _ = crate::pv::scan::l1::run(&mut cat, &shoots);
    let ids: Vec<i64> = {
        let mut s = cat
            .conn()
            .prepare("SELECT id FROM photos ORDER BY stem_key")
            .unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let t = std::time::Instant::now();
    ensure_stats(&mut cat, &ids).unwrap();
    println!(
        "stats for {} photos in {:.2} s",
        ids.len(),
        t.elapsed().as_secs_f64()
    );
    for v in raw_values(&cat, &ids, |_| Intent::Normal).unwrap() {
        let s: Option<(f64, f64, i64, i64, f64, f64)> = cat
            .conn()
            .query_row(
                "SELECT clipped, shadows, luma_low, luma_high, red_blue, green_magenta FROM raw_signals WHERE photo_id = ?1",
                [v.photo_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()
            .unwrap();
        println!(
            "{} {:?} {:?} {:?}",
            v.photo_id,
            v.verdict,
            v.chips
                .iter()
                .map(|c| (c.kind, c.value))
                .collect::<Vec<_>>(),
            s
        );
    }
}
