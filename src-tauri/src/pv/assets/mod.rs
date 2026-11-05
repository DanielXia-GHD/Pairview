//! Shoots and Photos: turns the files the L0 scanner found into Photos in the catalog
//! (PRD §5, docs/IMPLEMENTATION_PLAN.md M1-04).
//!
//! A shoot is a folder plus its format sub-folders (`arw/`, `hif/`, …). Rebuilding a shoot keeps
//! the ids of Photos whose files are still there, so ratings and other metadata stay attached.
//! The RAW keeps the id when the timestamp guard splits a pair.

// The commands (M1-09) are the first users.
#![allow(dead_code)]

pub mod pairing;
pub mod shoot;
pub mod state;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, NaiveDateTime};
use rusqlite::{OptionalExtension, Transaction};

use crate::pv::catalog::{Catalog, now_ms};
use pairing::{FileRef, Guard, Kind, PlannedPhoto};
use shoot::DateHint;
use state::State;

/// Pairs whose capture times differ by more than this are not the same frame (PRD §5.3).
const GUARD_TOLERANCE_MS: i64 = 2_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PairStats {
    pub shoots: usize,
    pub photos_added: usize,
    pub photos_updated: usize,
    pub photos_removed: usize,
}

/// Re-pairs the shoots that contain `changed_folders` (from `ScanStats::changed_folders`).
pub fn refresh(cat: &mut Catalog, changed_folders: &[i64]) -> Result<PairStats> {
    let tx = cat.conn_mut().transaction()?;
    let mut shoots = Vec::new();
    for &folder in changed_folders {
        if let Some(shoot) = shoot_of(&tx, folder)?
            && !shoots.contains(&shoot)
        {
            shoots.push(shoot);
        }
    }
    let mut stats = PairStats::default();
    for shoot in shoots {
        let s = rebuild_shoot(&tx, shoot)?;
        stats.shoots += 1;
        stats.photos_added += s.photos_added;
        stats.photos_updated += s.photos_updated;
        stats.photos_removed += s.photos_removed;
    }
    tx.commit()?;
    Ok(stats)
}

/// The shoot a folder belongs to: its parent for a format sub-folder, otherwise itself.
pub fn shoot_of(tx: &Transaction, folder_id: i64) -> Result<Option<i64>> {
    let row: Option<(Option<i64>, bool)> = tx
        .query_row(
            "SELECT parent_id, format_subdir FROM folders WHERE id = ?1",
            [folder_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.map(|(parent, format_subdir)| match parent {
        Some(parent) if format_subdir => parent,
        _ => folder_id,
    }))
}

struct Existing {
    id: i64,
    state: Option<State>,
    guard_mismatch: bool,
    representative: Option<i64>,
    display: Option<i64>,
    members: HashMap<i64, String>,
}

pub fn rebuild_shoot(tx: &Transaction, shoot_id: i64) -> Result<PairStats> {
    update_date_hint(tx, shoot_id)?;

    let files = shoot_files(tx, shoot_id)?;
    let exif = guard_exif(tx, &files)?;
    let plans = pairing::plan(&files, |raw, display| {
        guard(exif.get(&raw), exif.get(&display))
    });
    let existing = existing_photos(tx, shoot_id)?;

    // Match planned Photos to existing ones through their main files (RAW first).
    let mut owner: HashMap<i64, usize> = HashMap::new();
    for (i, e) in existing.iter().enumerate() {
        for (&file, role) in &e.members {
            if role != "sidecar" {
                owner.insert(file, i);
            }
        }
    }
    let mut used = HashSet::new();
    let assigned: Vec<(Option<usize>, &PlannedPhoto)> = plans
        .iter()
        .map(|p| {
            let matched = main_files(p)
                .filter_map(|f| owner.get(&f).copied())
                .find(|i| !used.contains(i));
            if let Some(i) = matched {
                used.insert(i);
            }
            (matched, p)
        })
        .collect();

    let mut stats = PairStats::default();

    // Deletes first: a file may move between Photos, and it can only be a member of one.
    for (i, e) in existing.iter().enumerate() {
        if !used.contains(&i) {
            tx.execute("DELETE FROM photos WHERE id = ?1", [e.id])?;
            stats.photos_removed += 1;
        }
    }
    for (matched, plan) in &assigned {
        if let Some(i) = matched {
            let wanted = members(plan);
            for (file, role) in &existing[*i].members {
                if wanted.get(file) != Some(role) {
                    tx.execute(
                        "DELETE FROM photo_members WHERE photo_id = ?1 AND file_id = ?2",
                        (existing[*i].id, file),
                    )?;
                }
            }
        }
    }

    let now = now_ms();
    for (matched, plan) in assigned {
        let previous = matched.and_then(|i| existing[i].state);
        let state = state::derive(plan, previous);
        let representative = plan.raw.or(plan.display);
        let photo_id = match matched {
            Some(i) => {
                let e = &existing[i];
                if e.state != Some(state)
                    || e.guard_mismatch != plan.guard_mismatch
                    || e.representative != representative
                    || e.display != plan.display
                {
                    tx.execute(
                        "UPDATE photos SET state = ?2, guard_mismatch = ?3,
                             representative_file_id = ?4, display_file_id = ?5, updated_at = ?6
                         WHERE id = ?1",
                        (
                            e.id,
                            state.as_str(),
                            plan.guard_mismatch,
                            representative,
                            plan.display,
                            now,
                        ),
                    )?;
                    stats.photos_updated += 1;
                }
                e.id
            }
            None => {
                tx.execute(
                    "INSERT INTO photos (shoot_folder_id, stem_key, state, guard_mismatch,
                         representative_file_id, display_file_id, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                    (
                        shoot_id,
                        &plan.stem_key,
                        state.as_str(),
                        plan.guard_mismatch,
                        representative,
                        plan.display,
                        now,
                    ),
                )?;
                stats.photos_added += 1;
                tx.last_insert_rowid()
            }
        };
        let have = matched.map(|i| &existing[i].members);
        for (file, role) in members(plan) {
            if have.and_then(|m| m.get(&file)) != Some(&role) {
                tx.execute(
                    "INSERT INTO photo_members (photo_id, file_id, role) VALUES (?1, ?2, ?3)",
                    (photo_id, file, role),
                )?;
            }
        }
    }
    update_capture_times(tx, shoot_id)?;
    Ok(stats)
}

fn main_files(p: &PlannedPhoto) -> impl Iterator<Item = i64> + '_ {
    p.raw
        .into_iter()
        .chain(p.display)
        .chain(p.extras.iter().copied())
}

fn members(p: &PlannedPhoto) -> HashMap<i64, String> {
    let mut m = HashMap::new();
    m.extend(p.raw.map(|f| (f, "raw".to_string())));
    m.extend(p.display.map(|f| (f, "display".to_string())));
    m.extend(p.extras.iter().map(|&f| (f, "extra".to_string())));
    m.extend(p.sidecars.iter().map(|&f| (f, "sidecar".to_string())));
    m
}

fn shoot_files(tx: &Transaction, shoot_id: i64) -> Result<Vec<FileRef>> {
    let mut stmt = tx.prepare_cached(
        "SELECT f.id, f.name, f.stem_key, f.ext, f.kind
         FROM files f JOIN folders d ON d.id = f.folder_id
         WHERE d.id = ?1 OR (d.parent_id = ?1 AND d.format_subdir = 1)",
    )?;
    let rows = stmt.query_map([shoot_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    let mut files = Vec::new();
    for row in rows {
        let (id, name, stem_key, ext, kind) = row?;
        if let Some(kind) = Kind::from_catalog(&kind) {
            files.push(FileRef {
                id,
                name,
                stem_key,
                ext,
                kind,
            });
        }
    }
    Ok(files)
}

fn existing_photos(tx: &Transaction, shoot_id: i64) -> Result<Vec<Existing>> {
    let mut stmt = tx.prepare_cached(
        "SELECT id, state, guard_mismatch, representative_file_id, display_file_id
         FROM photos WHERE shoot_folder_id = ?1 ORDER BY id",
    )?;
    let mut photos: Vec<Existing> = stmt
        .query_map([shoot_id], |r| {
            Ok(Existing {
                id: r.get(0)?,
                state: State::parse(&r.get::<_, String>(1)?),
                guard_mismatch: r.get(2)?,
                representative: r.get(3)?,
                display: r.get(4)?,
                members: HashMap::new(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = tx.prepare_cached(
        "SELECT m.photo_id, m.file_id, m.role
         FROM photo_members m JOIN photos p ON p.id = m.photo_id
         WHERE p.shoot_folder_id = ?1",
    )?;
    let index: HashMap<i64, usize> = photos.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
    for row in stmt.query_map([shoot_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (photo, file, role) = row?;
        photos[index[&photo]].members.insert(file, role);
    }
    Ok(photos)
}

struct GuardExif {
    taken_ms: Option<i64>,
    serial: Option<String>,
}

fn guard_exif(tx: &Transaction, files: &[FileRef]) -> Result<HashMap<i64, GuardExif>> {
    let mut stmt =
        tx.prepare_cached("SELECT date_original, subsec_ms, serial FROM exif WHERE file_id = ?1")?;
    let mut out = HashMap::new();
    for f in files.iter().filter(|f| f.kind != Kind::Sidecar) {
        let row: Option<(Option<String>, Option<i64>, Option<String>)> = stmt
            .query_row([f.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        if let Some((date, subsec, serial)) = row {
            let taken_ms = date
                .and_then(|d| NaiveDateTime::parse_from_str(&d, "%Y-%m-%dT%H:%M:%S").ok())
                .map(|d| d.and_utc().timestamp_millis() + subsec.unwrap_or(0));
            out.insert(f.id, GuardExif { taken_ms, serial });
        }
    }
    Ok(out)
}

fn guard(raw: Option<&GuardExif>, display: Option<&GuardExif>) -> Guard {
    let (Some(raw), Some(display)) = (raw, display) else {
        return Guard::Unknown;
    };
    if let (Some(a), Some(b)) = (&raw.serial, &display.serial)
        && a != b
    {
        return Guard::Mismatch;
    }
    match (raw.taken_ms, display.taken_ms) {
        (Some(a), Some(b)) if (a - b).abs() > GUARD_TOLERANCE_MS => Guard::Mismatch,
        (Some(_), Some(_)) => Guard::Match,
        _ => Guard::Unknown,
    }
}

/// Sets each Photo's capture time: EXIF (RAW first, then the display copy), else the folder's
/// date hint, else the file's mtime. `date_uncertain` marks Photos whose EXIF date is more than a
/// day from the folder's date, and Photos dated only by mtime.
fn update_capture_times(tx: &Transaction, shoot_id: i64) -> Result<()> {
    let hint: Option<String> = tx.query_row(
        "SELECT date_hint FROM folders WHERE id = ?1",
        [shoot_id],
        |r| r.get(0),
    )?;
    let hint_date = hint
        .as_deref()
        .and_then(|h| chrono::NaiveDate::parse_from_str(h, "%Y-%m-%d").ok());
    let mut stmt = tx.prepare_cached(
        "SELECT p.id, p.captured_at, p.date_source, p.date_uncertain,
                COALESCE(r.date_original, d.date_original),
                CASE WHEN r.date_original IS NOT NULL THEN r.subsec_ms ELSE d.subsec_ms END,
                (SELECT mtime FROM files WHERE id = p.representative_file_id)
         FROM photos p
         LEFT JOIN exif r ON r.file_id = p.representative_file_id
         LEFT JOIN exif d ON d.file_id = p.display_file_id
         WHERE p.shoot_folder_id = ?1",
    )?;
    type Row = (
        i64,
        Option<String>,
        Option<String>,
        bool,
        Option<String>,
        Option<i64>,
        Option<i64>,
    );
    let rows: Vec<Row> = stmt
        .query_map([shoot_id], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (id, captured, source, uncertain, exif_date, subsec, mtime) in rows {
        let (new_captured, new_source, new_uncertain) = if let Some(date) = exif_date {
            let far_from_hint = match (
                hint_date,
                NaiveDateTime::parse_from_str(&date, "%Y-%m-%dT%H:%M:%S"),
            ) {
                (Some(h), Ok(d)) => (d.date() - h).num_days().abs() > 1,
                _ => false,
            };
            let captured = match subsec {
                Some(ms) => format!("{date}.{ms:03}"),
                None => date,
            };
            (Some(captured), Some("exif"), far_from_hint)
        } else if let Some(h) = &hint {
            (Some(format!("{h}T00:00:00")), Some("folder"), false)
        } else {
            let local = mtime.and_then(DateTime::from_timestamp_millis).map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%dT%H:%M:%S")
                    .to_string()
            });
            (local, Some("mtime"), true)
        };
        if captured != new_captured || source.as_deref() != new_source || uncertain != new_uncertain
        {
            tx.execute(
                "UPDATE photos SET captured_at = ?2, date_source = ?3, date_uncertain = ?4
                 WHERE id = ?1",
                (id, new_captured, new_source, new_uncertain),
            )?;
        }
    }
    Ok(())
}

fn update_date_hint(tx: &Transaction, shoot_id: i64) -> Result<()> {
    let (name, mtime): (String, Option<i64>) = tx.query_row(
        "SELECT name, mtime FROM folders WHERE id = ?1",
        [shoot_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let near = mtime
        .and_then(DateTime::from_timestamp_millis)
        .unwrap_or_else(chrono::Utc::now)
        .date_naive();
    let hint = DateHint::parse(&name)
        .and_then(|h| h.resolve(near))
        .map(|d| d.format("%Y-%m-%d").to_string());
    tx.execute(
        "UPDATE folders SET date_hint = ?2 WHERE id = ?1 AND date_hint IS NOT ?2",
        (shoot_id, hint),
    )?;
    Ok(())
}

/// Orphan review "Keep as RAW-only" (M2-05): `⚠ RAW` Photos become plain RAW-only. Catalog
/// only; no file changes. Returns how many changed.
pub fn mark_raw_only(cat: &Catalog, photo_ids: &[i64]) -> Result<usize> {
    let mut n = 0;
    for id in photo_ids {
        n += cat.conn().execute(
            "UPDATE photos SET state = 'raw_only', updated_at = ?2
             WHERE id = ?1 AND state = 'orphan_raw'",
            (id, crate::pv::catalog::now_ms()),
        )?;
    }
    Ok(n)
}

/// Absolute path of a catalog file at the location's current mount path.
pub fn file_path(cat: &Catalog, file_id: i64) -> Result<PathBuf> {
    let (location, rel, name): (String, String, String) = cat.conn().query_row(
        "SELECT l.path, d.rel_path, f.name
         FROM files f JOIN folders d ON d.id = f.folder_id JOIN locations l ON l.id = d.location_id
         WHERE f.id = ?1",
        [file_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let mut path = PathBuf::from(location);
    if !rel.is_empty() {
        path.push(rel);
    }
    path.push(name);
    Ok(path)
}

/// The path the grid and editor use for a Photo: the RAW if present (A4).
pub fn representative_path(cat: &Catalog, photo_id: i64) -> Result<Option<PathBuf>> {
    let file: Option<i64> = cat.conn().query_row(
        "SELECT representative_file_id FROM photos WHERE id = ?1",
        [photo_id],
        |r| r.get(0),
    )?;
    file.map(|f| file_path(cat, f)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pv::locations::{self, Location};
    use crate::pv::scan::l0::{ScanOptions, ScanStats, scan_location};
    use std::fs;
    use std::path::Path;

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        cat: Catalog,
        loc: Location,
    }

    impl Fixture {
        fn new(files: &[&str]) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join("DCIM");
            fs::create_dir_all(&root).unwrap();
            for f in files {
                let p = root.join(f);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(&p, b"x").unwrap();
            }
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

        fn sync(&mut self) -> (ScanStats, PairStats) {
            let opts = ScanOptions {
                full: true,
                ..Default::default()
            };
            let scan = scan_location(&mut self.cat, &self.loc, &opts).unwrap();
            let pair = refresh(&mut self.cat, &scan.changed_folders).unwrap();
            (scan, pair)
        }

        /// `shoot: RAW DISPLAY state [extras…] [!]`, sorted.
        fn photos(&self) -> Vec<String> {
            let mut stmt = self
                .cat
                .conn()
                .prepare(
                    "SELECT p.id, s.rel_path, p.state, p.guard_mismatch,
                            (SELECT name FROM files WHERE id = p.representative_file_id),
                            (SELECT name FROM files WHERE id = p.display_file_id)
                     FROM photos p JOIN folders s ON s.id = p.shoot_folder_id",
                )
                .unwrap();
            let mut out: Vec<String> = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, bool>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                })
                .unwrap()
                .map(|r| {
                    let (id, shoot, state, mismatch, rep, display) = r.unwrap();
                    let mut extras: Vec<String> = self
                        .cat
                        .conn()
                        .prepare(
                            "SELECT f.name FROM photo_members m JOIN files f ON f.id = m.file_id
                             WHERE m.photo_id = ?1 AND m.role IN ('extra', 'sidecar')",
                        )
                        .unwrap()
                        .query_map([id], |r| r.get(0))
                        .unwrap()
                        .map(Result::unwrap)
                        .collect();
                    extras.sort();
                    let mut s = format!(
                        "{shoot}: {} {} {state}",
                        rep.unwrap_or("-".into()),
                        display.unwrap_or("-".into())
                    );
                    for e in extras {
                        s += &format!(" +{e}");
                    }
                    if mismatch {
                        s += " !";
                    }
                    s
                })
                .collect();
            out.sort();
            out
        }

        fn photo_id(&self, stem: &str) -> i64 {
            self.cat
                .conn()
                .query_row(
                    "SELECT id FROM photos WHERE stem_key = ?1 ORDER BY id LIMIT 1",
                    [stem],
                    |r| r.get(0),
                )
                .unwrap()
        }

        fn rm(&self, rel: &str) {
            fs::remove_file(self.root.join(rel)).unwrap();
        }
    }

    #[test]
    fn flat_shoot() {
        let mut f = Fixture::new(&[
            "10561031/DSC03600.ARW",
            "10561031/DSC03600.HIF",
            "10561031/DSC03601.ARW",
            "10561031/DSC03601.JPG",
        ]);
        f.sync();
        assert_eq!(
            f.photos(),
            [
                "10561031: DSC03600.ARW DSC03600.HIF raw_display",
                "10561031: DSC03601.ARW DSC03601.JPG raw_display",
            ]
        );
    }

    #[test]
    fn split_shoot_pairs_across_format_subfolders() {
        let mut f = Fixture::new(&[
            "2026-10-24_10361024/arw/DSC03545.ARW",
            "2026-10-24_10361024/hif/DSC03545.HIF",
            "2026-10-24_10361024/arw/DSC03546.ARW",
            "2026-10-24_10361024/hif/DSC03546.HIF",
            "2026-10-24_10361024/arw/DSC03545.ARW.rrdata",
        ]);
        let (_, stats) = f.sync();
        assert_eq!(stats.photos_added, 2);
        assert_eq!(
            f.photos(),
            [
                "2026-10-24_10361024: DSC03545.ARW DSC03545.HIF raw_display +DSC03545.ARW.rrdata",
                "2026-10-24_10361024: DSC03546.ARW DSC03546.HIF raw_display",
            ]
        );
    }

    #[test]
    fn singles_mixed_case_and_three_copies() {
        let mut f = Fixture::new(&[
            "a/DSC00001.ARW",
            "a/dsc00001.hif",
            "a/DSC00002.ARW",
            "a/DSC00003.HIF",
            "a/DSC00004.ARW",
            "a/DSC00004.HIF",
            "a/DSC00004.JPG",
            "a/C0001.MP4",
        ]);
        f.sync();
        assert_eq!(
            f.photos(),
            [
                "a: DSC00001.ARW dsc00001.hif raw_display",
                "a: DSC00002.ARW - raw_only",
                "a: DSC00003.HIF DSC00003.HIF display_only",
                "a: DSC00004.ARW DSC00004.HIF raw_display +DSC00004.JPG",
            ]
        );
    }

    #[test]
    fn subfolders_are_their_own_shoots() {
        let mut f = Fixture::new(&["trip/DSC00001.ARW", "trip/day2/DSC00001.HIF"]);
        f.sync();
        assert_eq!(
            f.photos(),
            [
                "trip/day2: DSC00001.HIF DSC00001.HIF display_only",
                "trip: DSC00001.ARW - raw_only",
            ]
        );
    }

    #[test]
    fn display_deleted_outside_the_app_leaves_an_orphan_and_keeps_the_photo() {
        let mut f = Fixture::new(&["a/DSC00001.ARW", "a/DSC00001.HIF", "a/DSC00002.ARW"]);
        f.sync();
        let id = f.photo_id("dsc00001");
        f.cat
            .conn()
            .execute(
                "INSERT INTO metadata (photo_id, rating, updated_at) VALUES (?1, 4, 0)",
                [id],
            )
            .unwrap();

        f.rm("a/DSC00001.HIF");
        f.sync();
        assert_eq!(
            f.photos(),
            ["a: DSC00001.ARW - orphan_raw", "a: DSC00002.ARW - raw_only"]
        );
        assert_eq!(f.photo_id("dsc00001"), id);
        let rating: i64 = f
            .cat
            .conn()
            .query_row(
                "SELECT rating FROM metadata WHERE photo_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rating, 4);

        // The display copy comes back (restored from a backup): a normal pair again.
        fs::write(f.root.join("a/DSC00001.HIF"), b"x").unwrap();
        f.sync();
        assert_eq!(f.photos()[0], "a: DSC00001.ARW DSC00001.HIF raw_display");
        assert_eq!(f.photo_id("dsc00001"), id);
    }

    #[test]
    fn raw_dropped_keeps_the_photo_as_display_only() {
        let mut f = Fixture::new(&["a/DSC00001.ARW", "a/DSC00001.HIF"]);
        f.sync();
        let id = f.photo_id("dsc00001");
        f.rm("a/DSC00001.ARW");
        f.sync();
        assert_eq!(f.photos(), ["a: DSC00001.HIF DSC00001.HIF display_only"]);
        assert_eq!(f.photo_id("dsc00001"), id);
    }

    #[test]
    fn whole_photo_deleted_is_removed() {
        let mut f = Fixture::new(&["a/DSC00001.ARW", "a/DSC00001.HIF", "a/DSC00002.HIF"]);
        f.sync();
        f.rm("a/DSC00001.ARW");
        f.rm("a/DSC00001.HIF");
        let (_, stats) = f.sync();
        assert_eq!(stats.photos_removed, 1);
        assert_eq!(f.photos(), ["a: DSC00002.HIF DSC00002.HIF display_only"]);
    }

    #[test]
    fn guard_mismatch_splits_and_the_raw_keeps_the_id() {
        let mut f = Fixture::new(&["a/DSC00001.ARW", "a/DSC00001.HIF"]);
        f.sync();
        let id = f.photo_id("dsc00001");
        let set_exif = |cat: &Catalog, name: &str, date: &str| {
            cat.conn()
                .execute(
                    "INSERT OR REPLACE INTO exif (file_id, file_mtime, date_original, read_at)
                     SELECT id, mtime, ?2, 0 FROM files WHERE name = ?1",
                    (name, date),
                )
                .unwrap();
        };
        // Within 2 s: still one Photo.
        set_exif(&f.cat, "DSC00001.ARW", "2026-10-31T10:00:00");
        set_exif(&f.cat, "DSC00001.HIF", "2026-10-31T10:00:01");
        let shoot = f
            .cat
            .conn()
            .query_row("SELECT shoot_folder_id FROM photos", [], |r| r.get(0))
            .unwrap();
        refresh(&mut f.cat, &[shoot]).unwrap();
        assert_eq!(f.photos(), ["a: DSC00001.ARW DSC00001.HIF raw_display"]);

        // A counter wrap: same name, a day apart.
        set_exif(&f.cat, "DSC00001.HIF", "2026-11-01T09:00:00");
        refresh(&mut f.cat, &[shoot]).unwrap();
        assert_eq!(
            f.photos(),
            [
                "a: DSC00001.ARW - raw_only !",
                "a: DSC00001.HIF DSC00001.HIF display_only !"
            ]
        );
        assert_eq!(f.photo_id("dsc00001"), id);
        let raw_photo: String = f
            .cat
            .conn()
            .query_row("SELECT state FROM photos WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(raw_photo, "raw_only");
    }

    #[test]
    fn repairing_without_changes_writes_nothing() {
        let mut f = Fixture::new(&[
            "2026-10-24_10361024/arw/DSC03545.ARW",
            "2026-10-24_10361024/hif/DSC03545.HIF",
            "10561031/DSC03600.ARW",
        ]);
        let (scan, _) = f.sync();
        let writes = f.cat.conn().total_changes();
        let stats = refresh(&mut f.cat, &scan.changed_folders).unwrap();
        assert_eq!(f.cat.conn().total_changes(), writes);
        assert_eq!(
            (
                stats.photos_added,
                stats.photos_updated,
                stats.photos_removed
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn date_hints_and_paths() {
        let mut f = Fixture::new(&[
            "2026-10-24_10361024/arw/DSC03545.ARW",
            "10561031/DSC03600.HIF",
            "Exports/x.jpg",
        ]);
        f.sync();
        let hint = |rel: &str| -> Option<String> {
            f.cat
                .conn()
                .query_row(
                    "SELECT date_hint FROM folders WHERE rel_path = ?1",
                    [rel],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(hint("2026-10-24_10361024").as_deref(), Some("2026-10-24"));
        // The decade comes from the folder's mtime (now), so this assumes the test runs in 2026–2035.
        assert!(hint("10561031").unwrap().ends_with("6-10-31"));
        assert_eq!(hint("Exports"), None);

        let id = f.photo_id("dsc03545");
        assert_eq!(
            representative_path(&f.cat, id).unwrap().unwrap(),
            f.loc.path.join("2026-10-24_10361024/arw/DSC03545.ARW")
        );
        assert!(Path::new(&representative_path(&f.cat, id).unwrap().unwrap()).exists());
    }

    /// `PV_FIXTURES=~/Desktop/2026-10-23_10261023 cargo test --lib pv::assets -- --nocapture`
    #[test]
    fn real_fixture_pairs_completely() {
        let Ok(dir) = std::env::var("PV_FIXTURES") else {
            return;
        };
        let mut cat = Catalog::open_in_memory().unwrap();
        let id = locations::add(&cat, Path::new(&dir), None).unwrap();
        let loc = locations::get(&cat, id).unwrap().unwrap();
        let scan = scan_location(&mut cat, &loc, &ScanOptions::for_location(&loc)).unwrap();
        let stats = refresh(&mut cat, &scan.changed_folders).unwrap();
        let count = |sql: &str| -> i64 { cat.conn().query_row(sql, [], |r| r.get(0)).unwrap() };
        let raws = count("SELECT COUNT(*) FROM files WHERE kind = 'raw'");
        let pairs = count("SELECT COUNT(*) FROM photos WHERE state = 'raw_display'");
        println!("{stats:?}: {pairs} pairs of {raws} RAWs");
        assert!(raws > 0);
        assert_eq!(pairs, raws);
        assert_eq!(count("SELECT COUNT(*) FROM photos"), pairs);
    }
}
