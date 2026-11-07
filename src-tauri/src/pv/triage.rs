//! AI triage, first pass (docs/IMPLEMENTATION_PLAN.md M4, PRD §6.5 Stage 1 and §6.7): bursts, the
//! best of each, suggested rejects and a dimmed "AI ★", from the pixels alone.
//!
//! **This only suggests.** Nothing here writes to a Photo's files, or to its ratings or flags: a
//! suggestion becomes a reject only through the owner's own action. Signals are cached in the
//! catalog per source file version, which is rebuildable. Everything else is derived on request
//! with the current ratings and flags, so human input counts at once.
//!
//! The thresholds are reasoned starting points checked on a few real and synthetic frames, not
//! tuned on real bursts. See docs/M4-triage.md.

use std::path::{Path, PathBuf};

use anyhow::Result;
use image::{DynamicImage, GrayImage};
use rayon::prelude::*;
use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::pv::catalog::{Catalog, now_ms};

/// Long edge everything is measured at, so sharpness compares across files of different sizes.
const WORK_PX: u32 = 768;
/// Tiles along the long edge. Sharpness is taken from the best few: a subject in focus against a
/// soft background is a sharp photo, which a whole-image measure would call blurry.
const TILES: u32 = 12;
const BEST_TILES: usize = 3;
const PARALLEL: usize = 8;

/// Frames this close in time are a burst if they also look alike.
const BURST_GAP_S: f64 = 2.0;
const BURST_HASH_BITS: u32 = 20;
/// Further apart in time, they must look nearly identical: the same shot taken again.
const NEAR_DUPLICATE_GAP_S: f64 = 60.0;
const NEAR_DUPLICATE_HASH_BITS: u32 = 6;

/// Below this many measured Photos a shoot's median says little, so nothing is called blurry.
const MIN_PHOTOS_FOR_BLUR: usize = 6;
/// Scenes differ a lot in texture, so this has to be low: on 60 real frames from different shoots,
/// 0.1 flags 1 sharp frame and all 60 heavily blurred copies, where 0.2 flags 7 sharp ones.
const BLURRY_AGAINST_SHOOT: f32 = 0.1;
/// Frames of a burst share a scene, so they can be held to much more.
const BLURRY_AGAINST_BURST: f32 = 0.35;

/// A misfire: nearly one flat tone, or nearly black or white (lens cap, the ground).
const MISFIRE_CONTRAST: f32 = 5.0;
const MISFIRE_DARK: f32 = 6.0;
const MISFIRE_BRIGHT: f32 = 250.0;

const HARSH_CLIPPED: f32 = 0.05;
const HARSH_SHADOWS: f32 = 0.6;

// ---- Measuring a frame ------------------------------------------------------------------------

/// What's measured from a Photo's pixels. Expensive, so cached.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Signals {
    /// Mean Laplacian variance of the sharpest few tiles.
    pub sharpness: f32,
    /// Difference hash: similar frames differ in few of these 64 bits.
    pub hash: u64,
    pub mean_luma: f32,
    /// Standard deviation of luma.
    pub contrast: f32,
    pub clipped: f32,
    pub shadows: f32,
}

fn laplacian_variance(gray: &GrayImage, x0: u32, y0: u32, x1: u32, y1: u32) -> f32 {
    let (x0, y0) = (x0.max(1), y0.max(1));
    let (x1, y1) = (x1.min(gray.width() - 1), y1.min(gray.height() - 1));
    if x1 <= x0 || y1 <= y0 {
        return 0.0;
    }
    let (mut sum, mut square, mut n) = (0f64, 0f64, 0f64);
    let at = |x: u32, y: u32| gray.get_pixel(x, y).0[0] as i32;
    for y in y0..y1 {
        for x in x0..x1 {
            let v =
                (at(x, y - 1) + at(x, y + 1) + at(x - 1, y) + at(x + 1, y) - 4 * at(x, y)) as f64;
            sum += v;
            square += v * v;
            n += 1.0;
        }
    }
    let mean = sum / n;
    (square / n - mean * mean) as f32
}

fn sharpness_of(gray: &GrayImage) -> f32 {
    let (w, h) = gray.dimensions();
    if w < 8 || h < 8 {
        return 0.0;
    }
    let tile = (w.max(h) / TILES).max(4);
    let mut tiles = Vec::new();
    for ty in 0..h / tile {
        for tx in 0..w / tile {
            tiles.push(laplacian_variance(
                gray,
                tx * tile,
                ty * tile,
                (tx + 1) * tile,
                (ty + 1) * tile,
            ));
        }
    }
    tiles.sort_by(|a, b| b.total_cmp(a));
    let best = &tiles[..BEST_TILES.min(tiles.len())];
    best.iter().sum::<f32>() / best.len().max(1) as f32
}

/// Each bit says whether a tiny grey version gets brighter to the right.
fn difference_hash(image: &DynamicImage) -> u64 {
    let small = image::imageops::resize(
        &image.to_luma8(),
        9,
        8,
        image::imageops::FilterType::Triangle,
    );
    let mut hash = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            hash <<= 1;
            if small.get_pixel(x + 1, y).0[0] > small.get_pixel(x, y).0[0] {
                hash |= 1;
            }
        }
    }
    hash
}

pub fn signals_of_image(image: &DynamicImage) -> Signals {
    let small = if image.width().max(image.height()) > WORK_PX {
        crate::pv::thumbs::shrink(image, WORK_PX)
    } else {
        image.clone()
    };
    let gray = small.to_luma8();
    let n = gray.pixels().len().max(1) as f64;
    let mean = gray.pixels().map(|p| p.0[0] as f64).sum::<f64>() / n;
    let variance = gray
        .pixels()
        .map(|p| (p.0[0] as f64 - mean).powi(2))
        .sum::<f64>()
        / n;
    let stats = crate::pv::signals::image_stats(&small);
    Signals {
        sharpness: sharpness_of(&gray),
        hash: difference_hash(&small),
        mean_luma: mean as f32,
        contrast: variance.sqrt() as f32,
        clipped: stats.clipped,
        shadows: stats.shadows,
    }
}

fn signals_of(path: &Path) -> Option<Signals> {
    // A camera preview is a real camera rendering at a useful size, read the cheap way.
    let image =
        crate::pv::thumbs::quick_thumbnail(path, 1024).or_else(|| image::open(path).ok())?;
    Some(signals_of_image(&image))
}

// ---- Judging a shoot --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RejectReason {
    Misfire,
    Blurry,
    BurstNotBest,
}

/// The owner's own say on a Photo, read fresh each time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Human {
    pub rating: i64,
    pub flag: String,
    pub keep_raw: String,
}

impl Human {
    /// Anything the owner has said in a Photo's favour.
    fn wants_it(&self) -> bool {
        self.rating > 0 || self.flag == "pick" || self.keep_raw == "keep"
    }
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub photo_id: i64,
    pub captured_at: Option<chrono::NaiveDateTime>,
    pub signals: Signals,
    pub human: Human,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub photo_id: i64,
    /// 0 to 5, separate from the owner's stars and never written over them.
    pub ai_stars: u8,
    /// The burst this Photo is in: the id of its first frame.
    pub burst: Option<i64>,
    pub burst_size: usize,
    pub best_of_burst: bool,
    pub reject: Option<RejectReason>,
}

fn is_misfire(s: &Signals) -> bool {
    s.contrast < MISFIRE_CONTRAST || s.mean_luma < MISFIRE_DARK || s.mean_luma > MISFIRE_BRIGHT
}

/// Sharpness, marked down for a frame that lost a lot to clipping.
fn quality(s: &Signals) -> f32 {
    s.sharpness * (1.0 - (s.clipped * 4.0).min(0.5))
}

/// Consecutive frames grouped by time and likeness. Indexes into `frames`, which is in time order.
fn bursts(frames: &[&Frame]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in 0..frames.len() {
        let joins = i > 0 && {
            let (a, b) = (frames[i - 1], frames[i]);
            match (a.captured_at, b.captured_at) {
                (Some(ta), Some(tb)) => {
                    let gap = (tb - ta).num_milliseconds() as f64 / 1000.0;
                    let bits = (a.signals.hash ^ b.signals.hash).count_ones();
                    (gap <= BURST_GAP_S && bits <= BURST_HASH_BITS)
                        || (gap <= NEAR_DUPLICATE_GAP_S && bits <= NEAR_DUPLICATE_HASH_BITS)
                }
                _ => false,
            }
        };
        match (joins, groups.last_mut()) {
            (true, Some(group)) => group.push(i),
            _ => groups.push(vec![i]),
        }
    }
    groups.retain(|g| g.len() > 1);
    groups
}

/// The best frame of a burst. The owner's own choice beats any measurement.
fn best_of(group: &[usize], frames: &[&Frame]) -> usize {
    let rank = |f: &Frame| {
        (
            (f.human.flag == "pick") as u8,
            f.human.rating,
            (f.human.flag != "reject") as u8,
        )
    };
    *group
        .iter()
        .max_by(|&&a, &&b| {
            rank(frames[a])
                .cmp(&rank(frames[b]))
                .then(quality(&frames[a].signals).total_cmp(&quality(&frames[b].signals)))
        })
        .expect("a burst has frames")
}

/// Suggestions for one shoot's measured frames.
pub fn judge_shoot(frames: &[Frame]) -> Vec<Suggestion> {
    let mut ordered: Vec<&Frame> = frames.iter().collect();
    ordered.sort_by_key(|f| (f.captured_at.is_none(), f.captured_at, f.photo_id));

    let mut sharpness: Vec<f32> = ordered
        .iter()
        .filter(|f| !is_misfire(&f.signals))
        .map(|f| f.signals.sharpness)
        .collect();
    sharpness.sort_by(f32::total_cmp);
    let median = sharpness.get(sharpness.len() / 2).copied();
    let rank_of = |s: f32| -> f32 {
        if sharpness.len() < 2 {
            return 0.5;
        }
        sharpness.partition_point(|&v| v < s) as f32 / (sharpness.len() - 1) as f32
    };

    let groups = bursts(&ordered);
    let mut out: Vec<Suggestion> = ordered
        .iter()
        .map(|f| {
            let s = &f.signals;
            let misfire = is_misfire(s);
            let soft_for_shoot = sharpness.len() >= MIN_PHOTOS_FOR_BLUR
                && median.is_some_and(|m| s.sharpness < m * BLURRY_AGAINST_SHOOT);
            let mut stars = 1 + (rank_of(s.sharpness).min(1.0) * 3.0).round() as i32;
            if s.clipped > HARSH_CLIPPED || s.shadows > HARSH_SHADOWS {
                stars -= 1;
            }
            if soft_for_shoot {
                stars = 1;
            }
            if misfire {
                stars = 0;
            }
            Suggestion {
                photo_id: f.photo_id,
                ai_stars: stars.clamp(0, 5) as u8,
                burst: None,
                burst_size: 0,
                best_of_burst: false,
                reject: if misfire {
                    Some(RejectReason::Misfire)
                } else if soft_for_shoot {
                    Some(RejectReason::Blurry)
                } else {
                    None
                },
            }
        })
        .collect();

    for group in &groups {
        let best = best_of(group, &ordered);
        let best_quality = quality(&ordered[best].signals);
        for &i in group {
            let sug = &mut out[i];
            sug.burst = Some(ordered[group[0]].photo_id);
            sug.burst_size = group.len();
            if i == best {
                sug.best_of_burst = true;
                sug.ai_stars = (sug.ai_stars + 1).min(5);
            } else {
                sug.ai_stars = sug.ai_stars.saturating_sub(1).max(sug.ai_stars.min(1));
                if sug.reject.is_none() {
                    let soft = quality(&ordered[i].signals) < best_quality * BLURRY_AGAINST_BURST;
                    sug.reject = Some(if soft {
                        RejectReason::Blurry
                    } else {
                        RejectReason::BurstNotBest
                    });
                }
            }
        }
    }

    // Human input wins: nothing the owner kept is suggested for rejection, and something they
    // already rejected needs no suggestion.
    for (sug, f) in out.iter_mut().zip(&ordered) {
        if f.human.wants_it() || f.human.flag == "reject" {
            sug.reject = None;
        }
    }
    out
}

// ---- The catalog ------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    photo_id: i64,
    file_id: i64,
    mtime: i64,
    path: PathBuf,
}

/// The Photos of a shoot, in no particular order.
pub fn shoot_photos(cat: &Catalog, folder_id: i64) -> Result<Vec<i64>> {
    let mut s = cat
        .conn()
        .prepare("SELECT id FROM photos WHERE shoot_folder_id = ?1")?;
    let ids = s
        .query_map([folder_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// Which of `photo_ids` have no signals for their current source file.
pub fn pending(cat: &Catalog, photo_ids: &[i64]) -> Result<Vec<Pending>> {
    let mut todo = Vec::new();
    for &id in photo_ids {
        let Some(file_id) = crate::pv::thumbs::thumbnail_file_id(cat, id)? else {
            continue;
        };
        let mtime: i64 =
            cat.conn()
                .query_row("SELECT mtime FROM files WHERE id = ?1", [file_id], |r| {
                    r.get(0)
                })?;
        let cached: Option<(i64, i64)> = cat
            .conn()
            .query_row(
                "SELECT source_file_id, source_mtime FROM triage_signals WHERE photo_id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if cached != Some((file_id, mtime)) {
            todo.push(Pending {
                photo_id: id,
                file_id,
                mtime,
                path: crate::pv::assets::file_path(cat, file_id)?,
            });
        }
    }
    Ok(todo)
}

/// Reads and measures, without holding the catalog: on a NAS this is the slow part.
pub fn measure(todo: &[Pending]) -> Result<Vec<(Pending, Signals)>> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(PARALLEL)
        .build()?;
    Ok(pool.install(|| {
        todo.par_iter()
            .filter_map(|p| signals_of(&p.path).map(|s| (p.clone(), s)))
            .collect()
    }))
}

pub fn store(cat: &mut Catalog, measured: &[(Pending, Signals)]) -> Result<()> {
    let tx = cat.conn_mut().transaction()?;
    let now = now_ms();
    for (p, s) in measured {
        tx.execute(
            "INSERT OR REPLACE INTO triage_signals (photo_id, source_file_id, source_mtime,
                 sharpness, hash, mean_luma, contrast, clipped, shadows, computed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                p.photo_id,
                p.file_id,
                p.mtime,
                s.sharpness,
                s.hash as i64,
                s.mean_luma,
                s.contrast,
                s.clipped,
                s.shadows,
                now
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn parse_time(text: &str) -> Option<chrono::NaiveDateTime> {
    ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
        .iter()
        .find_map(|f| chrono::NaiveDateTime::parse_from_str(text, f).ok())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShootTriage {
    pub folder_id: i64,
    pub photos: usize,
    pub measured: usize,
    pub suggestions: Vec<Suggestion>,
}

/// Suggestions for a shoot from what's already measured. Fast, and reads no image.
pub fn results(cat: &Catalog, folder_id: i64) -> Result<ShootTriage> {
    let photos = shoot_photos(cat, folder_id)?.len();
    let mut s = cat.conn().prepare(
        "SELECT p.id, p.captured_at, t.sharpness, t.hash, t.mean_luma, t.contrast, t.clipped,
                t.shadows, COALESCE(md.rating, 0), COALESCE(md.flag, 'none'),
                COALESCE(md.keep_raw, 'auto')
         FROM photos p
         JOIN triage_signals t ON t.photo_id = p.id
         LEFT JOIN metadata md ON md.photo_id = p.id
         WHERE p.shoot_folder_id = ?1",
    )?;
    let frames: Vec<Frame> = s
        .query_map([folder_id], |r| {
            let captured: Option<String> = r.get(1)?;
            Ok(Frame {
                photo_id: r.get(0)?,
                captured_at: captured.as_deref().and_then(parse_time),
                signals: Signals {
                    sharpness: r.get(2)?,
                    hash: r.get::<_, i64>(3)? as u64,
                    mean_luma: r.get(4)?,
                    contrast: r.get(5)?,
                    clipped: r.get(6)?,
                    shadows: r.get(7)?,
                },
                human: Human {
                    rating: r.get(8)?,
                    flag: r.get(9)?,
                    keep_raw: r.get(10)?,
                },
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ShootTriage {
        folder_id,
        photos,
        measured: frames.len(),
        suggestions: judge_shoot(&frames),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, NaiveDate};
    use image::{Rgb, RgbImage};

    fn at(seconds: i64) -> Option<chrono::NaiveDateTime> {
        let start = NaiveDate::from_ymd_opt(2026, 10, 24)?.and_hms_opt(12, 0, 0)?;
        Some(start + Duration::seconds(seconds))
    }

    fn signals(sharpness: f32, hash: u64) -> Signals {
        Signals {
            sharpness,
            hash,
            mean_luma: 110.0,
            contrast: 45.0,
            clipped: 0.0,
            shadows: 0.05,
        }
    }

    fn frame(photo_id: i64, seconds: i64, sharpness: f32, hash: u64) -> Frame {
        Frame {
            photo_id,
            captured_at: at(seconds),
            signals: signals(sharpness, hash),
            human: Human::default(),
        }
    }

    fn of(out: &[Suggestion], id: i64) -> &Suggestion {
        out.iter().find(|s| s.photo_id == id).unwrap()
    }

    /// A hash `n` bits away from 0.
    fn bits(n: u32) -> u64 {
        (0..n).fold(0, |h, i| h | 1 << i)
    }

    #[test]
    fn a_burst_is_close_in_time_and_alike() {
        let out = judge_shoot(&[
            frame(1, 0, 100.0, 0),
            frame(2, 1, 300.0, bits(4)),
            frame(3, 2, 200.0, bits(8)),
            // Close in time, but a different picture.
            frame(4, 3, 200.0, u64::MAX),
            // Alike, but long after.
            frame(5, 500, 200.0, u64::MAX),
        ]);
        assert_eq!(of(&out, 1).burst, Some(1));
        assert_eq!(of(&out, 3).burst_size, 3);
        assert!(of(&out, 2).best_of_burst, "the sharpest");
        assert_eq!(of(&out, 4).burst, None);
        assert_eq!(of(&out, 5).burst, None);
    }

    #[test]
    fn the_same_shot_taken_again_is_grouped_only_if_nearly_identical() {
        let out = judge_shoot(&[frame(1, 0, 100.0, 0), frame(2, 40, 100.0, bits(3))]);
        assert_eq!(of(&out, 2).burst, Some(1));
        let out = judge_shoot(&[frame(1, 0, 100.0, 0), frame(2, 40, 100.0, bits(12))]);
        assert_eq!(
            of(&out, 2).burst,
            None,
            "time apart and only somewhat alike"
        );
    }

    #[test]
    fn frames_without_a_time_never_join_a_burst() {
        let mut undated = frame(2, 0, 100.0, 0);
        undated.captured_at = None;
        let out = judge_shoot(&[frame(1, 0, 100.0, 0), undated]);
        assert_eq!(of(&out, 1).burst, None);
        assert_eq!(of(&out, 2).burst, None);
    }

    #[test]
    fn the_rest_of_a_burst_are_suggested_for_rejection() {
        let out = judge_shoot(&[
            frame(1, 0, 200.0, 0),
            frame(2, 1, 300.0, 0),
            frame(3, 2, 50.0, 0),
        ]);
        assert_eq!(of(&out, 1).reject, Some(RejectReason::BurstNotBest));
        assert_eq!(of(&out, 2).reject, None);
        assert_eq!(
            of(&out, 3).reject,
            Some(RejectReason::Blurry),
            "much softer than the best"
        );
    }

    #[test]
    fn human_input_wins() {
        // The owner's pick is the best of its burst, whatever the numbers say.
        let mut picked = frame(1, 0, 50.0, 0);
        picked.human.flag = "pick".into();
        let out = judge_shoot(&[picked, frame(2, 1, 300.0, 0)]);
        assert!(of(&out, 1).best_of_burst);
        assert_eq!(of(&out, 1).reject, None);
        assert_eq!(of(&out, 2).reject, Some(RejectReason::BurstNotBest));

        // Rated or locked: never suggested, however bad it measures.
        let mut flat = signals(0.0, 0);
        flat.contrast = 1.0;
        for human in [
            Human {
                rating: 1,
                ..Human::default()
            },
            Human {
                keep_raw: "keep".into(),
                ..Human::default()
            },
        ] {
            let kept = Frame {
                photo_id: 9,
                captured_at: at(0),
                signals: flat,
                human,
            };
            assert_eq!(judge_shoot(&[kept])[0].reject, None);
        }
    }

    #[test]
    fn a_photo_already_rejected_needs_no_suggestion() {
        let mut rejected = frame(1, 0, 50.0, 0);
        rejected.human.flag = "reject".into();
        let out = judge_shoot(&[rejected, frame(2, 1, 300.0, 0)]);
        assert_eq!(of(&out, 1).reject, None);
        assert!(of(&out, 2).best_of_burst);
    }

    #[test]
    fn blurry_is_judged_against_the_shoot_and_needs_enough_photos() {
        let spread = |i: i64| i * 1000;
        let mut frames: Vec<Frame> = (1..=6)
            .map(|i| frame(i, spread(i), 300.0, (i as u64) * 0x0101_0101_0101_0101))
            .collect();
        frames[5].signals.sharpness = 20.0;
        let out = judge_shoot(&frames);
        assert_eq!(of(&out, 6).reject, Some(RejectReason::Blurry));
        assert_eq!(of(&out, 6).ai_stars, 1);
        assert_eq!(of(&out, 1).reject, None);

        // In a small shoot one soft frame isn't enough evidence.
        let out = judge_shoot(&frames[3..]);
        assert_eq!(of(&out, 6).reject, None);
    }

    #[test]
    fn a_misfire_is_suggested_and_gets_no_stars() {
        let mut cap = frame(1, 0, 0.5, 0);
        cap.signals.mean_luma = 2.0;
        cap.signals.contrast = 1.0;
        let out = judge_shoot(&[cap, frame(2, 100, 300.0, u64::MAX)]);
        assert_eq!(of(&out, 1).reject, Some(RejectReason::Misfire));
        assert_eq!(of(&out, 1).ai_stars, 0);
    }

    #[test]
    fn stars_stay_in_range_and_follow_sharpness() {
        let frames: Vec<Frame> = (1..=8)
            .map(|i| {
                frame(
                    i,
                    i * 1000,
                    i as f32 * 40.0,
                    (i as u64) * 0x0101_0101_0101_0101,
                )
            })
            .collect();
        let out = judge_shoot(&frames);
        assert!(out.iter().all(|s| s.ai_stars <= 5));
        assert!(of(&out, 8).ai_stars > of(&out, 1).ai_stars);
    }

    #[test]
    fn an_empty_shoot_is_fine() {
        assert!(judge_shoot(&[]).is_empty());
    }

    /// Detailed enough to have edges at every scale, unlike a flat test card.
    fn detailed(seed: u32) -> DynamicImage {
        let mut image = RgbImage::new(1200, 800);
        for (x, y, p) in image.enumerate_pixels_mut() {
            let v = (x.wrapping_mul(7919) ^ y.wrapping_mul(104_729) ^ seed)
                .wrapping_mul(2_654_435_761)
                >> 24;
            // Large shapes the hash can read, plus fine detail the sharpness measure can.
            let shape = (x / 150 % 2) * 70 + (y / 200 % 2) * 50;
            let base = 40 + shape + (x / 12 % 2) * 30;
            *p = Rgb([(base + v / 8) as u8; 3]);
        }
        DynamicImage::ImageRgb8(image)
    }

    #[test]
    fn a_blurred_frame_measures_softer_and_its_hash_stays_close() {
        let sharp = detailed(1);
        let soft = sharp.blur(8.0);
        let (a, b) = (signals_of_image(&sharp), signals_of_image(&soft));
        assert!(
            b.sharpness < a.sharpness * 0.2,
            "{} {}",
            a.sharpness,
            b.sharpness
        );
        assert!((a.hash ^ b.hash).count_ones() <= BURST_HASH_BITS);
    }

    #[test]
    fn a_small_sharp_area_makes_a_sharp_photo() {
        let mut image = RgbImage::from_pixel(1200, 800, Rgb([120, 120, 120]));
        let detail = detailed(1).to_rgb8();
        for y in 300..500 {
            for x in 500..700 {
                image.put_pixel(x, y, *detail.get_pixel(x, y));
            }
        }
        let subject = signals_of_image(&DynamicImage::ImageRgb8(image));
        let all = signals_of_image(&detailed(1));
        assert!(
            subject.sharpness > all.sharpness * 0.5,
            "{} {}",
            subject.sharpness,
            all.sharpness
        );
    }

    #[test]
    fn a_lens_cap_measures_as_a_misfire() {
        let cap = DynamicImage::ImageRgb8(RgbImage::from_pixel(1200, 800, Rgb([3, 3, 3])));
        assert!(is_misfire(&signals_of_image(&cap)));
        assert!(!is_misfire(&signals_of_image(&detailed(1))));
    }

    /// Everything under `root` as (path, length, content).
    fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push((path.clone(), std::fs::read(&path).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn triage_changes_no_file_and_caches_per_file_version() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap().join("DCIM");
        std::fs::create_dir_all(root.join("s")).unwrap();
        detailed(1).save(root.join("s/DSC0001.JPG")).unwrap();
        detailed(1)
            .blur(8.0)
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
        let folder: i64 = cat
            .conn()
            .query_row("SELECT DISTINCT shoot_folder_id FROM photos", [], |r| {
                r.get(0)
            })
            .unwrap();
        let ids = shoot_photos(&cat, folder).unwrap();
        assert_eq!(ids.len(), 2);
        let before = snapshot(&root);

        // Nothing is measured yet, so there's nothing to say.
        let empty = results(&cat, folder).unwrap();
        assert_eq!((empty.photos, empty.measured), (2, 0));

        let todo = pending(&cat, &ids).unwrap();
        assert_eq!(todo.len(), 2);
        let measured = measure(&todo).unwrap();
        store(&mut cat, &measured).unwrap();
        assert!(pending(&cat, &ids).unwrap().is_empty(), "cached");

        let out = results(&cat, folder).unwrap();
        assert_eq!(out.measured, 2);
        assert_eq!(out.suggestions.len(), 2);
        assert_eq!(
            snapshot(&root),
            before,
            "triage must not touch a Photo's files"
        );

        // A new rating counts at once, with no new measuring.
        let soft = measured
            .iter()
            .min_by(|a, b| a.1.sharpness.total_cmp(&b.1.sharpness))
            .unwrap()
            .0
            .photo_id;
        cat.conn()
            .execute(
                "INSERT OR REPLACE INTO metadata (photo_id, rating, flag, keep_raw, updated_at)
                 VALUES (?1, 3, 'none', 'auto', 0)",
                [soft],
            )
            .unwrap();
        let out = results(&cat, folder).unwrap();
        let s = out.suggestions.iter().find(|s| s.photo_id == soft).unwrap();
        assert_eq!(s.reject, None);
    }
}

/// How the signals behave on real frames, as numbers only: no image is written anywhere.
/// `PV_LOOK_DIR=<pairs> cargo test --lib pv::triage::calibrate -- --nocapture`
#[cfg(all(test, target_os = "macos"))]
#[test]
fn calibrate_on_real_frames() {
    let Ok(dir) = std::env::var("PV_LOOK_DIR") else {
        return;
    };
    assert!(!dir.starts_with("/Volumes"), "local copies only");
    let mut pairs: Vec<_> = std::fs::read_dir(Path::new(&dir).join("pairs"))
        .unwrap()
        .flatten()
        .map(|e| e.path().join("cam.HIF"))
        .filter(|p| p.exists())
        .collect();
    pairs.sort();
    let rows: Vec<(f32, f32, f32, u32, u32, f32)> = pairs
        .par_iter()
        .filter_map(|path| {
            let image = crate::pv::thumbs::quick_thumbnail(path, 1024)?;
            let a = signals_of_image(&image);
            let slight = signals_of_image(&image.blur(1.5));
            let heavy = signals_of_image(&image.blur(4.0));
            Some((
                a.sharpness,
                slight.sharpness / a.sharpness.max(1e-3),
                heavy.sharpness / a.sharpness.max(1e-3),
                (a.hash ^ slight.hash).count_ones(),
                (a.hash ^ heavy.hash).count_ones(),
                a.contrast,
            ))
        })
        .collect();
    let mid = |mut v: Vec<f32>| -> (f32, f32, f32) {
        v.sort_by(f32::total_cmp);
        (v[v.len() / 10], v[v.len() / 2], v[v.len() * 9 / 10])
    };
    println!("{} real frames; p10, median, p90", rows.len());
    let (_, median, _) = mid(rows.iter().map(|r| r.0).collect());
    for ratio in [0.2f32, 0.1, 0.05, 0.03] {
        let plain = rows.iter().filter(|r| r.0 < median * ratio).count();
        let slight = rows.iter().filter(|r| r.0 * r.1 < median * ratio).count();
        let heavy = rows.iter().filter(|r| r.0 * r.2 < median * ratio).count();
        println!(
            "under {ratio} x median: {plain} sharp frames, {slight} with blur 1.5, {heavy} with blur 4, of {}",
            rows.len()
        );
    }
    println!(
        "sharpness                {:?}",
        mid(rows.iter().map(|r| r.0).collect())
    );
    println!(
        "contrast                 {:?}",
        mid(rows.iter().map(|r| r.5).collect())
    );
    println!(
        "sharpness kept, blur 1.5 {:?}",
        mid(rows.iter().map(|r| r.1).collect())
    );
    println!(
        "sharpness kept, blur 4   {:?}",
        mid(rows.iter().map(|r| r.2).collect())
    );
    println!(
        "hash bits moved, blur 1.5 {:?}",
        mid(rows.iter().map(|r| r.3 as f32).collect())
    );
    println!(
        "hash bits moved, blur 4   {:?}",
        mid(rows.iter().map(|r| r.4 as f32).collect())
    );
    // Different scenes must not look like a burst.
    let hashes: Vec<u64> = pairs
        .par_iter()
        .filter_map(|p| crate::pv::thumbs::quick_thumbnail(p, 1024))
        .map(|i| signals_of_image(&i).hash)
        .collect();
    let mut apart = Vec::new();
    for (i, a) in hashes.iter().enumerate() {
        for b in &hashes[i + 1..] {
            apart.push((a ^ b).count_ones() as f32);
        }
    }
    let under = apart
        .iter()
        .filter(|&&b| b <= BURST_HASH_BITS as f32)
        .count();
    let near = apart
        .iter()
        .filter(|&&b| b <= NEAR_DUPLICATE_HASH_BITS as f32)
        .count();
    println!(
        "bits between different scenes {:?}; {under} of {} within the burst limit, {near} within the near-duplicate limit",
        mid(apart.clone()),
        apart.len()
    );
}

/// On a real shoot, through the real scan: numbers only, and no file may change.
/// `PV_FIXTURES=~/Desktop/2026-10-23_10261023 cargo test --lib pv::triage::real -- --nocapture`
#[cfg(test)]
#[test]
fn real_shoot_triage() {
    let Ok(source) = std::env::var("PV_FIXTURES") else {
        return;
    };
    let root = PathBuf::from(source);
    assert!(!root.starts_with("/Volumes"), "local copies only");
    let listing = |root: &Path| -> Vec<(PathBuf, u64, std::time::SystemTime)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let (path, meta) = (entry.path(), entry.metadata().unwrap());
                if meta.is_dir() {
                    stack.push(path);
                } else {
                    out.push((path, meta.len(), meta.modified().unwrap()));
                }
            }
        }
        out.sort();
        out
    };
    let before = listing(&root);
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
    let _ = crate::pv::scan::l1::run(&mut cat, &scan.changed_folders);
    let folder: i64 = cat
        .conn()
        .query_row("SELECT shoot_folder_id FROM photos LIMIT 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let ids = shoot_photos(&cat, folder).unwrap();
    let t = std::time::Instant::now();
    let measured = measure(&pending(&cat, &ids).unwrap()).unwrap();
    println!(
        "{} of {} photos measured in {:.2} s",
        measured.len(),
        ids.len(),
        t.elapsed().as_secs_f64()
    );
    store(&mut cat, &measured).unwrap();
    let out = results(&cat, folder).unwrap();
    for (p, signal) in &measured {
        let (stem, captured): (String, Option<String>) = cat
            .conn()
            .query_row(
                "SELECT stem_key, captured_at FROM photos WHERE id = ?1",
                [p.photo_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let s = out
            .suggestions
            .iter()
            .find(|s| s.photo_id == p.photo_id)
            .unwrap();
        println!(
            "{stem} {captured:?} parsed={} sharpness {:.0} contrast {:.0} clipped {:.3} | stars {} burst {:?}x{} best {} reject {:?}",
            captured.as_deref().and_then(parse_time).is_some(),
            signal.sharpness,
            signal.contrast,
            signal.clipped,
            s.ai_stars,
            s.burst,
            s.burst_size,
            s.best_of_burst,
            s.reject,
        );
    }
    let hashes: Vec<u64> = measured.iter().map(|m| m.1.hash).collect();
    let mut bits: Vec<u32> = Vec::new();
    for (i, a) in hashes.iter().enumerate() {
        for b in &hashes[i + 1..] {
            bits.push((a ^ b).count_ones());
        }
    }
    println!("hash bits between these frames: {bits:?}");
    assert_eq!(listing(&root), before, "triage must not touch the shoot");
}

/// The migration and the queries on a copy of a real catalog. Reads no image.
/// `PV_CATALOG_COPY=<copy> cargo test --lib pv::triage::on_a_real_catalog -- --nocapture`
#[cfg(test)]
#[test]
fn on_a_real_catalog() {
    let Ok(copy) = std::env::var("PV_CATALOG_COPY") else {
        return;
    };
    let cat = Catalog::open(Path::new(&copy)).unwrap();
    println!("schema version {}", cat.schema_version().unwrap());
    let mut s = cat
        .conn()
        .prepare("SELECT DISTINCT shoot_folder_id FROM photos")
        .unwrap();
    let folders: Vec<i64> = s
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let (mut photos, mut measured) = (0, 0);
    for folder in &folders {
        let out = results(&cat, *folder).unwrap();
        photos += out.photos;
        measured += out.measured;
    }
    println!(
        "{} shoots, {photos} photos, {measured} measured",
        folders.len()
    );
}
