//! L1 metadata worker: reads EXIF for the files of Photos and stores it in the catalog
//! (docs/IMPLEMENTATION_PLAN.md M1-05).
//!
//! Work is ordered for a fast first impression: shoots in the order given (the opened folder
//! first), and within that, one file per Photo (its representative) before the remaining
//! members. The second pass feeds the timestamp guard, which `assets::rebuild_shoot` applies.
//! Reads run 8 at a time, each reading only the start of its file (`pv::exif_read`).
//!
//! Results are cached per file mtime: a file is read again only when it changes.

// The scheduler (M1-06) and commands (M1-09) are the first users.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::Result;
use rayon::prelude::*;

use crate::pv::assets;
use crate::pv::catalog::{Catalog, now_ms};
use crate::pv::exif_read::{ExifInfo, read_exif};

const PARALLEL_READS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// The representative file of each Photo: enough for dates and sorting.
    Representative,
    /// Every other member: the guard compares these with the representative.
    Members,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct L1Stats {
    pub files_read: usize,
    pub files_failed: usize,
    pub bytes_read: u64,
}

struct Job {
    file_id: i64,
    mtime: i64,
    path: PathBuf,
}

fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(PARALLEL_READS)
            .thread_name(|i| format!("pv-l1-{i}"))
            .build()
            .expect("L1 thread pool")
    })
}

/// Runs both passes over `shoots`, in priority order.
pub fn run(cat: &mut Catalog, shoots: &[i64]) -> Result<L1Stats> {
    let mut stats = L1Stats::default();
    for pass in [Pass::Representative, Pass::Members] {
        for &shoot in shoots {
            let s = run_pass(cat, shoot, pass)?;
            stats.files_read += s.files_read;
            stats.files_failed += s.files_failed;
            stats.bytes_read += s.bytes_read;
        }
    }
    Ok(stats)
}

/// One pass over one shoot: read what's missing or stale, store it, and re-derive the shoot's
/// Photos (capture times, guard splits). Writes nothing when nothing needed reading.
pub fn run_pass(cat: &mut Catalog, shoot: i64, pass: Pass) -> Result<L1Stats> {
    let jobs = pending(cat, shoot, pass)?;
    if jobs.is_empty() {
        return Ok(L1Stats::default());
    }
    // An I/O error (NAS gone) stores nothing, so the file is tried again later. A file that
    // reads fine but has no EXIF is stored as such and not read again until it changes.
    let results: Vec<(Job, Option<ExifInfo>)> = pool().install(|| {
        jobs.into_par_iter()
            .map(|job| {
                let info = read_exif(&job.path).ok();
                (job, info)
            })
            .collect()
    });

    let mut stats = L1Stats::default();
    let tx = cat.conn_mut().transaction()?;
    let now = now_ms();
    for (job, info) in results {
        let Some(info) = info else {
            stats.files_failed += 1;
            continue;
        };
        stats.files_read += 1;
        stats.bytes_read += info.bytes_read;
        tx.execute(
            "INSERT OR REPLACE INTO exif (file_id, file_mtime, date_original, subsec_ms,
                 offset_original, make, model, serial, lens, orientation, width, height, iso,
                 exposure_s, f_number, focal_mm, gps_lat, gps_lon, gps_alt, read_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                     ?17, ?18, ?19, ?20)",
            rusqlite::params![
                job.file_id,
                job.mtime,
                info.date_original,
                info.subsec_ms,
                info.offset_original,
                info.make,
                info.model,
                info.serial,
                info.lens,
                info.orientation,
                info.width,
                info.height,
                info.iso,
                info.exposure_s,
                info.f_number,
                info.focal_mm,
                info.gps_lat,
                info.gps_lon,
                info.gps_alt,
                now,
            ],
        )?;
    }
    if stats.files_read > 0 {
        assets::rebuild_shoot(&tx, shoot)?;
    }
    tx.commit()?;
    Ok(stats)
}

/// Files of the shoot's Photos whose EXIF is missing or older than the file.
fn pending(cat: &Catalog, shoot: i64, pass: Pass) -> Result<Vec<Job>> {
    let which = match pass {
        Pass::Representative => "f.id = p.representative_file_id",
        Pass::Members => "m.role IN ('raw', 'display', 'extra')",
    };
    let sql = format!(
        "SELECT DISTINCT f.id, f.mtime, l.path, d.rel_path, f.name
         FROM photos p
         JOIN photo_members m ON m.photo_id = p.id
         JOIN files f ON f.id = m.file_id
         JOIN folders d ON d.id = f.folder_id
         JOIN locations l ON l.id = d.location_id
         LEFT JOIN exif e ON e.file_id = f.id
         WHERE p.shoot_folder_id = ?1 AND {which}
           AND (e.file_id IS NULL OR e.file_mtime != f.mtime)
         ORDER BY f.name"
    );
    let mut stmt = cat.conn().prepare_cached(&sql)?;
    let jobs = stmt
        .query_map([shoot], |r| {
            let mut path = PathBuf::from(r.get::<_, String>(2)?);
            let rel: String = r.get(3)?;
            if !rel.is_empty() {
                path.push(rel);
            }
            path.push(r.get::<_, String>(4)?);
            Ok(Job {
                file_id: r.get(0)?,
                mtime: r.get(1)?,
                path,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pv::locations;
    use crate::pv::scan::l0::{ScanOptions, scan_location};
    use exif::{Field, In, Tag, Value};
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant};

    /// A tiny JPEG carrying `DateTimeOriginal` (+ sub-seconds). Saved as `.ARW` it stands in for
    /// a RAW: the reader goes by content, not extension.
    fn write_jpeg(path: &Path, date: &str, subsec: &str) {
        let fields = [
            Field {
                tag: Tag::DateTimeOriginal,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![date.as_bytes().to_vec()]),
            },
            Field {
                tag: Tag::SubSecTimeOriginal,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![subsec.as_bytes().to_vec()]),
            },
            Field {
                tag: Tag::Model,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"ILCE-7CM2".to_vec()]),
            },
        ];
        let mut writer = exif::experimental::Writer::new();
        for f in &fields {
            writer.push_field(f);
        }
        let mut tiff = std::io::Cursor::new(Vec::new());
        writer.write(&mut tiff, false).unwrap();
        let tiff = tiff.into_inner();

        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(8, 8)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let jpeg = jpeg.into_inner();

        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        out.extend_from_slice(b"Exif\0\0");
        out.extend_from_slice(&tiff);
        out.extend_from_slice(&jpeg[2..]);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, out).unwrap();
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        cat: Catalog,
        loc: locations::Location,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join("DCIM");
            fs::create_dir_all(&root).unwrap();
            let cat = Catalog::open_in_memory().unwrap();
            let id = locations::add(&cat, &root, None).unwrap();
            let loc = locations::get(&cat, id).unwrap().unwrap();
            Self {
                _tmp: tmp,
                root,
                cat,
                loc,
            }
        }

        /// L0 + pairing; returns the shoot ids that changed.
        fn sync(&mut self) -> Vec<i64> {
            let opts = ScanOptions {
                full: true,
                ..Default::default()
            };
            let scan = scan_location(&mut self.cat, &self.loc, &opts).unwrap();
            assets::refresh(&mut self.cat, &scan.changed_folders).unwrap();
            let mut stmt = self
                .cat
                .conn()
                .prepare("SELECT DISTINCT shoot_folder_id FROM photos ORDER BY 1")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        }

        fn photos(&self) -> Vec<(String, Option<String>, String, bool, bool)> {
            let mut stmt = self
                .cat
                .conn()
                .prepare(
                    "SELECT stem_key, captured_at, date_source, date_uncertain, guard_mismatch
                     FROM photos ORDER BY stem_key, id",
                )
                .unwrap();
            stmt.query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
        }
    }

    #[test]
    fn dates_come_from_exif_and_pairs_within_two_seconds_stay_together() {
        let mut f = Fixture::new();
        write_jpeg(
            &f.root.join("2026-10-31_10561031/arw/DSC00001.ARW"),
            "2026:10:31 10:00:00",
            "25",
        );
        write_jpeg(
            &f.root.join("2026-10-31_10561031/hif/DSC00001.JPG"),
            "2026:10:31 10:00:01",
            "9",
        );
        write_jpeg(
            &f.root.join("2026-10-31_10561031/arw/DSC00002.ARW"),
            "2026:10:31 10:05:00",
            "",
        );
        let shoots = f.sync();
        assert_eq!(f.photos()[0].2, "folder"); // before L1: the folder's date

        let stats = run(&mut f.cat, &shoots).unwrap();
        assert_eq!((stats.files_read, stats.files_failed), (3, 0));
        assert_eq!(
            f.photos(),
            [
                (
                    "dsc00001".into(),
                    Some("2026-10-31T10:00:00.250".into()),
                    "exif".into(),
                    false,
                    false
                ),
                (
                    "dsc00002".into(),
                    Some("2026-10-31T10:05:00".into()),
                    "exif".into(),
                    false,
                    false
                ),
            ]
        );
    }

    #[test]
    fn mismatched_pair_is_flagged_and_split() {
        let mut f = Fixture::new();
        write_jpeg(
            &f.root.join("10561031/DSC00001.ARW"),
            "2026:10:31 10:00:00",
            "",
        );
        write_jpeg(
            &f.root.join("10561031/DSC00001.JPG"),
            "2026:11:01 09:00:00",
            "",
        );
        let shoots = f.sync();

        // After the first pass the RAW's date is known but the pair still looks fine.
        run_pass(&mut f.cat, shoots[0], Pass::Representative).unwrap();
        assert_eq!(f.photos().len(), 1);

        run_pass(&mut f.cat, shoots[0], Pass::Members).unwrap();
        let photos = f.photos();
        assert_eq!(photos.len(), 2);
        assert!(photos.iter().all(|p| p.4), "{photos:?}");
        let states: Vec<String> = f
            .cat
            .conn()
            .prepare("SELECT state FROM photos ORDER BY state")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(states, ["display_only", "raw_only"]);
    }

    #[test]
    fn results_are_cached_until_the_file_changes() {
        let mut f = Fixture::new();
        let raw = f.root.join("a/DSC00001.ARW");
        write_jpeg(&raw, "2026:10:31 10:00:00", "");
        write_jpeg(&f.root.join("a/DSC00001.JPG"), "2026:10:31 10:00:00", "");
        let shoots = f.sync();
        run(&mut f.cat, &shoots).unwrap();

        let writes = f.cat.conn().total_changes();
        assert_eq!(run(&mut f.cat, &shoots).unwrap(), L1Stats::default());
        assert_eq!(f.cat.conn().total_changes(), writes);

        // The RAW is replaced (different date); the scan sees a new mtime, L1 reads it again.
        std::thread::sleep(Duration::from_millis(20));
        write_jpeg(&raw, "2026:10:31 11:00:00", "");
        let shoots = f.sync();
        assert_eq!(run(&mut f.cat, &shoots).unwrap().files_read, 1);
        assert!(f.photos().iter().all(|p| p.4)); // now an hour apart
    }

    #[test]
    fn far_from_the_folder_date_is_uncertain_and_unreadable_files_fall_back() {
        let mut f = Fixture::new();
        write_jpeg(
            &f.root.join("2026-10-31_x/DSC00001.JPG"),
            "2025:01:01 10:00:00",
            "",
        );
        fs::write(f.root.join("2026-10-31_x/DSC00002.JPG"), b"not an image").unwrap();
        let shoots = f.sync();
        let stats = run(&mut f.cat, &shoots).unwrap();
        assert_eq!(stats.files_read, 2); // the broken file is stored as "no EXIF"
        let photos = f.photos();
        assert_eq!((photos[0].2.as_str(), photos[0].3), ("exif", true));
        assert_eq!(
            (photos[1].1.as_deref(), photos[1].2.as_str(), photos[1].3),
            (Some("2026-10-31T00:00:00"), "folder", false)
        );
    }

    /// `PV_FIXTURES=~/Desktop/2026-10-23_10261023 cargo test --lib pv::scan::l1 -- --nocapture`
    #[test]
    fn real_fixture_dates_and_guard() {
        let Ok(dir) = std::env::var("PV_FIXTURES") else {
            return;
        };
        let mut cat = Catalog::open_in_memory().unwrap();
        let id = locations::add(&cat, Path::new(&dir), None).unwrap();
        let loc = locations::get(&cat, id).unwrap().unwrap();
        let scan = scan_location(&mut cat, &loc, &ScanOptions::for_location(&loc)).unwrap();
        assets::refresh(&mut cat, &scan.changed_folders).unwrap();
        let shoots: Vec<i64> = cat
            .conn()
            .prepare("SELECT DISTINCT shoot_folder_id FROM photos")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();

        let start = Instant::now();
        let stats = run(&mut cat, &shoots).unwrap();
        println!("{stats:?} in {:?}", start.elapsed());
        let count = |sql: &str| -> i64 { cat.conn().query_row(sql, [], |r| r.get(0)).unwrap() };
        let photos = count("SELECT COUNT(*) FROM photos");
        assert!(photos > 0);
        assert_eq!(
            count("SELECT COUNT(*) FROM photos WHERE date_source = 'exif'"),
            photos
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM photos WHERE guard_mismatch = 1"),
            0
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM photos WHERE date_uncertain = 1"),
            0
        );
        assert_eq!(stats.files_failed, 0);
    }
}
