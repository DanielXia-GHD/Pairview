//! Ready to slim and Review for slimming (docs/IMPLEMENTATION_PLAN.md M2-09, PRD §6.5 Stage 4).
//!
//! Shoots past the 30-day grace period with RAW+HIF pairs are *suggested*; the review screen
//! shows each pair's RAW-value verdict; only the owner's **Apply** drops RAWs, and only those
//! the owner left selected. Drops go through `pv/lifecycle` (Keep HIF only → Recently Deleted,
//! "Slimmed RAWs", 30 days). There is no automatic or AI-only slimming (PRD D11).

use anyhow::Result;
use serde::Serialize;

use crate::pv::catalog::Catalog;
use crate::pv::signals::{self, RawValue};

/// Grace period before a shoot is suggested (PRD §6.5).
pub const GRACE_DAYS: i64 = 30;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyShoot {
    pub folder_id: i64,
    pub location_id: i64,
    pub name: String,
    /// Folder path at the location's current mount point.
    pub path: String,
    /// `YYYY-MM-DD`: the folder's date, else its latest capture day.
    pub date: String,
    /// RAW+display pairs not locked Keep RAW.
    pub pairs: i64,
    /// Bytes of those pairs' RAWs: the most slimming could free.
    pub raw_bytes: i64,
}

/// Shoots past the grace period with pairs to review, oldest first. Shoots whose review was
/// finished are left out. Catalog only (no file reads).
pub fn ready(cat: &Catalog, today: chrono::NaiveDate) -> Result<Vec<ReadyShoot>> {
    let cutoff = (today - chrono::Duration::days(GRACE_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    let mut stmt = cat.conn().prepare(
        "SELECT s.id, s.location_id, s.name, l.path, s.rel_path,
                COALESCE(s.date_hint, substr(MAX(p.captured_at), 1, 10)) AS day,
                COUNT(DISTINCT p.id),
                COALESCE(SUM(CASE WHEN m.role = 'raw' THEN f.size END), 0)
         FROM photos p
         JOIN folders s ON s.id = p.shoot_folder_id
         JOIN locations l ON l.id = s.location_id
         LEFT JOIN metadata md ON md.photo_id = p.id
         LEFT JOIN photo_members m ON m.photo_id = p.id AND m.role = 'raw'
         LEFT JOIN files f ON f.id = m.file_id
         WHERE p.state IN ('raw_display', 'edited')
           AND COALESCE(md.keep_raw, 'auto') != 'keep'
           AND s.slim_reviewed_at IS NULL
         GROUP BY s.id
         HAVING day IS NOT NULL AND day <= ?1
         ORDER BY day, s.name",
    )?;
    let rows = stmt
        .query_map([cutoff], |r| {
            let root: String = r.get(3)?;
            let rel: String = r.get(4)?;
            Ok(ReadyShoot {
                folder_id: r.get(0)?,
                location_id: r.get(1)?,
                name: r.get(2)?,
                path: if rel.is_empty() {
                    root
                } else {
                    format!("{root}/{rel}")
                },
                date: r.get(5)?,
                pairs: r.get(6)?,
                raw_bytes: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlimItem {
    pub value: RawValue,
    /// What dropping this RAW frees: the RAW and its own sidecars.
    pub raw_bytes: i64,
}

/// The Photos of a shoot, for "Ready to slim → open".
pub fn shoot_photos(cat: &Catalog, shoot_id: i64) -> Result<Vec<i64>> {
    let mut stmt = cat
        .conn()
        .prepare("SELECT id FROM photos WHERE shoot_folder_id = ?1 ORDER BY captured_at, id")?;
    let ids = stmt
        .query_map([shoot_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// Review items for the RAW+display pairs among `photo_ids` (other Photos have no RAW to drop
/// while keeping a display copy, so they're left out). Computes missing image statistics.
pub fn review(cat: &mut Catalog, photo_ids: &[i64]) -> Result<Vec<SlimItem>> {
    let pairs: Vec<i64> = photo_ids
        .iter()
        .copied()
        .filter(|&id| {
            cat.conn()
                .query_row("SELECT state FROM photos WHERE id = ?1", [id], |r| {
                    r.get::<_, String>(0)
                })
                .is_ok_and(|s| s == "raw_display" || s == "edited")
        })
        .collect();
    signals::ensure_stats(cat, &pairs)?;
    let values = signals::raw_values(cat, &pairs, |shoot| {
        crate::pv::intent::get(cat, shoot).unwrap_or_default()
    })?;
    let mut items = Vec::with_capacity(values.len());
    for value in values {
        let raw_bytes: i64 = cat.conn().query_row(
            "SELECT COALESCE(SUM(f.size), 0) FROM photo_members m JOIN files f ON f.id = m.file_id
             WHERE m.photo_id = ?1 AND m.role = 'raw'",
            [value.photo_id],
            |r| r.get(0),
        )?;
        items.push(SlimItem { value, raw_bytes });
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pv::lifecycle::{self, Scope, SkipReason};
    use crate::pv::locations;
    use crate::pv::scan::l0::{ScanOptions, scan_location};
    use std::fs;

    fn fixture(files: &[&str]) -> (tempfile::TempDir, Catalog, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap().join("DCIM");
        for f in files {
            let p = root.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, f.as_bytes()).unwrap();
        }
        let mut cat = Catalog::open_in_memory().unwrap();
        let loc = locations::add(&cat, &root, None).unwrap();
        let loc = locations::get(&cat, loc).unwrap().unwrap();
        let scan = scan_location(&mut cat, &loc, &ScanOptions::default()).unwrap();
        crate::pv::assets::refresh(&mut cat, &scan.changed_folders).unwrap();
        (tmp, cat, root)
    }

    fn id(cat: &Catalog, stem: &str) -> i64 {
        cat.conn()
            .query_row("SELECT id FROM photos WHERE stem_key = ?1", [stem], |r| {
                r.get(0)
            })
            .unwrap()
    }

    fn today() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 11, 5).unwrap()
    }

    #[test]
    fn ready_lists_old_unreviewed_shoots_with_pairs() {
        let (_tmp, cat, _root) = fixture(&[
            "2026-09-01_old/DSC0001.ARW",
            "2026-09-01_old/DSC0001.HIF",
            "2026-09-01_old/DSC0002.ARW",
            "2026-09-01_old/DSC0002.HIF",
            "2026-10-30_new/DSC0003.ARW",
            "2026-10-30_new/DSC0003.HIF",
            "2026-08-01_rawonly/DSC0004.ARW",
            "2026-08-02_locked/DSC0005.ARW",
            "2026-08-02_locked/DSC0005.HIF",
        ]);
        cat.conn()
            .execute(
                "INSERT INTO metadata (photo_id, keep_raw, updated_at) VALUES (?1, 'keep', 0)",
                [id(&cat, "dsc0005")],
            )
            .unwrap();
        let ready = ready(&cat, today()).unwrap();
        assert_eq!(ready.len(), 1, "{ready:?}");
        assert_eq!(
            (ready[0].name.as_str(), ready[0].pairs),
            ("2026-09-01_old", 2)
        );
        assert_eq!(ready[0].date, "2026-09-01");
        assert_eq!(
            ready[0].raw_bytes,
            ("2026-09-01_old/DSC0001.ARW".len() * 2) as i64
        );

        crate::pv::intent::mark_slim_reviewed(&cat, ready[0].folder_id, 1).unwrap();
        assert!(super::ready(&cat, today()).unwrap().is_empty());
    }

    #[test]
    fn review_covers_pairs_only_and_apply_drops_the_chosen_raws() {
        let (_tmp, mut cat, root) = fixture(&[
            "s/DSC0001.ARW",
            "s/DSC0001.HIF",
            "s/DSC0002.ARW",
            "s/DSC0002.HIF",
            "s/DSC0003.HIF",
        ]);
        let ids = [
            id(&cat, "dsc0001"),
            id(&cat, "dsc0002"),
            id(&cat, "dsc0003"),
        ];
        let items = review(&mut cat, &ids).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].raw_bytes, "s/DSC0001.ARW".len() as i64);

        // The owner keeps DSC0002's RAW and applies.
        let plan = lifecycle::plan_photos(&cat, &ids[..1], Scope::KeepDisplay).unwrap();
        for (_, r) in lifecycle::apply(&mut cat, &plan) {
            r.unwrap();
        }
        assert!(!root.join("s/DSC0001.ARW").exists());
        assert!(root.join("s/DSC0001.HIF").exists());
        assert!(root.join("s/DSC0002.ARW").exists());
    }

    #[test]
    fn an_edit_newer_than_its_render_is_never_dropped() {
        let (_tmp, mut cat, root) = fixture(&["s/DSC0001.ARW", "s/DSC0001.HIF"]);
        let p = id(&cat, "dsc0001");
        cat.conn()
            .execute("UPDATE photos SET state = 'edited' WHERE id = ?1", [p])
            .unwrap();
        // The recipe is saved after the HIF was rendered.
        fs::write(root.join("s/DSC0001.ARW.rrdata"), b"{}").unwrap();
        let loc = locations::list(&cat).unwrap().pop().unwrap();
        let scan = scan_location(&mut cat, &loc, &ScanOptions::default()).unwrap();
        crate::pv::assets::refresh(&mut cat, &scan.changed_folders).unwrap();
        cat.conn()
            .execute(
                "UPDATE files SET mtime = mtime + 60000 WHERE name = 'DSC0001.ARW.rrdata'",
                [],
            )
            .unwrap();
        cat.conn()
            .execute("UPDATE photos SET state = 'edited' WHERE id = ?1", [p])
            .unwrap();
        let plan = lifecycle::plan_photos(&cat, &[p], Scope::KeepDisplay).unwrap();
        assert!(plan.ops.is_empty());
        assert_eq!(plan.skipped[0].reason, SkipReason::NeedsRender);
    }
}
