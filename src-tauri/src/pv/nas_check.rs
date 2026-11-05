//! M1-18 exit check: performance and pairing numbers against the real NAS (PRD §9).
//!
//! Read-only on the NAS: the catalog lives in memory, thumbnails and decodes stay in memory.
//! Skipped unless `PV_NAS` is set.
//!
//! ```sh
//! PV_NAS=/Volumes/Public/Photos/A/DCIM PV_NAS_DAY=2026-10-31_10561031 \
//!   cargo test --lib pv::nas_check -- --nocapture
//! ```
//! `PV_NAS_DAY` is the shoot folder (inside `PV_NAS`) used for the "open a day" numbers.

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use rayon::prelude::*;

    use crate::pv::catalog::Catalog;
    use crate::pv::scan::l0::{ScanOptions, scan_location};
    use crate::pv::{assets, locations, scan::l1, thumbs};

    const THUMB_PX: u32 = 512;

    fn secs(d: Duration) -> String {
        format!("{:.2} s", d.as_secs_f64())
    }

    #[test]
    fn nas_exit_check() {
        let Ok(root) = std::env::var("PV_NAS") else {
            return;
        };
        let root = Path::new(&root);
        assert!(
            !root.starts_with(std::env::temp_dir()),
            "PV_NAS should be the NAS"
        );
        let mut cat = Catalog::open_in_memory().unwrap();
        let count = |cat: &Catalog, sql: &str| -> i64 {
            cat.conn().query_row(sql, [], |r| r.get(0)).unwrap()
        };

        // Identity of the share (what a location is recognised by after a remount).
        let identity = locations::identify(root).unwrap();
        println!("identity: {identity:?}");

        // L0: listings of the whole root.
        let t = Instant::now();
        let id = locations::add(&cat, root, None).unwrap();
        let loc = locations::get(&cat, id).unwrap().unwrap();
        let scan = scan_location(&mut cat, &loc, &ScanOptions::for_location(&loc)).unwrap();
        let l0 = t.elapsed();
        let t = Instant::now();
        assets::refresh(&mut cat, &scan.changed_folders).unwrap();
        let pairing = t.elapsed();
        println!(
            "L0 scan: {} for {} files in {} folders; pairing {}",
            secs(l0),
            count(&cat, "SELECT COUNT(*) FROM files"),
            count(&cat, "SELECT COUNT(*) FROM folders"),
            secs(pairing)
        );

        // Pairing per shoot.
        let mut stmt = cat
            .conn()
            .prepare(
                "SELECT f.rel_path,
                        COUNT(*),
                        SUM(p.state = 'raw_display'), SUM(p.state = 'edited'),
                        SUM(p.state = 'raw_only'), SUM(p.state = 'display_only'),
                        SUM(p.state = 'orphan_raw'), SUM(p.guard_mismatch),
                        (SELECT COUNT(*) FROM files x JOIN folders xf ON xf.id = x.folder_id
                          WHERE x.kind = 'raw' AND (xf.id = f.id OR xf.parent_id = f.id))
                 FROM photos p JOIN folders f ON f.id = p.shoot_folder_id
                 GROUP BY f.id ORDER BY f.rel_path",
            )
            .unwrap();
        println!(
            "shoot | photos | pairs | edited | raw only | display only | orphans | mismatch | RAW files"
        );
        let rows: Vec<(String, Vec<i64>)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    (1..=8).map(|i| r.get::<_, i64>(i).unwrap_or(0)).collect(),
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        drop(stmt);
        for (shoot, n) in &rows {
            println!(
                "{shoot} | {}",
                n.iter().map(i64::to_string).collect::<Vec<_>>().join(" | ")
            );
        }

        // L1: EXIF for every file under the root.
        let shoots: Vec<i64> = {
            let mut s = cat
                .conn()
                .prepare("SELECT DISTINCT shoot_folder_id FROM photos")
                .unwrap();
            s.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let t = Instant::now();
        let l1 = l1::run(&mut cat, &shoots).unwrap();
        let l1_time = t.elapsed();
        println!(
            "L1 EXIF: {} for {} files ({} failed), {:.1} MB read  [target ≤ 5 min]",
            secs(l1_time),
            l1.files_read,
            l1.files_failed,
            l1.bytes_read as f64 / 1e6
        );
        println!(
            "dates: {} uncertain of {} photos",
            count(&cat, "SELECT COUNT(*) FROM photos WHERE date_uncertain = 1"),
            count(&cat, "SELECT COUNT(*) FROM photos")
        );

        // Open a day: thumbnails, 8 in parallel like the grid.
        let Ok(day) = std::env::var("PV_NAS_DAY") else {
            return;
        };
        let ids: Vec<i64> = {
            let mut s = cat
                .conn()
                .prepare(
                    "SELECT p.id FROM photos p JOIN folders f ON f.id = p.shoot_folder_id
                     WHERE f.name = ?1 ORDER BY p.id",
                )
                .unwrap();
            s.query_map([&day], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert!(!ids.is_empty(), "no photos in shoot {day}");
        let sources: Vec<_> = ids
            .iter()
            .filter_map(|&id| thumbs::thumbnail_file(&cat, id).unwrap())
            .collect();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(8)
            .build()
            .unwrap();
        for run in ["first", "second"] {
            let t = Instant::now();
            let made = pool.install(|| {
                sources
                    .par_iter()
                    .filter(|p| thumbs::quick_thumbnail(p, THUMB_PX).is_some())
                    .count()
            });
            println!(
                "{day} thumbnails ({run} run): {} for {made}/{} photos  [target < 5 s cold]",
                secs(t.elapsed()),
                sources.len()
            );
        }

        // Viewer: a cold display copy, read whole (what the webview gets) and decoded full size.
        let displays: Vec<_> = {
            let mut s = cat
                .conn()
                .prepare(
                    "SELECT p.display_file_id FROM photos p JOIN folders f ON f.id = p.shoot_folder_id
                     WHERE f.name = ?1 AND p.display_file_id IS NOT NULL ORDER BY p.id DESC LIMIT 5",
                )
                .unwrap();
            let ids: Vec<i64> = s
                .query_map([&day], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            ids.into_iter()
                .map(|f| assets::file_path(&cat, f).unwrap())
                .collect()
        };
        for path in &displays {
            let t = Instant::now();
            let bytes = std::fs::read(path).unwrap();
            let read = t.elapsed();
            let t = Instant::now();
            let decoded = if crate::pv::heif::is_heif_file(path) {
                crate::pv::heif::decode_heif(&bytes).map(|i| i.width())
            } else {
                image::load_from_memory(&bytes)
                    .map(|i| i.width())
                    .map_err(Into::into)
            };
            println!(
                "viewer {}: read {:.1} MB in {}, decode {} ({:?} px wide)  [target ≤ 0.6 s cold]",
                path.file_name().unwrap().to_string_lossy(),
                bytes.len() as f64 / 1e6,
                secs(read),
                secs(t.elapsed()),
                decoded.ok()
            );
        }
    }
}
