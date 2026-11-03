//! L0 scanner: records a location's folders and media files from directory listings only. No
//! file is opened (docs/IMPLEMENTATION_PLAN.md M1-03).
//!
//! Incremental: a folder whose mtime hasn't changed since it was listed is not listed again.
//! Its known sub-folders are still visited, because a change deep in the tree doesn't touch the
//! parent's mtime. A re-scan with no changes writes nothing to the catalog.
//!
//! Edits that rewrite a file in place without touching its folder (rare: cameras and Pairview
//! write new files) are only picked up by a full scan (`ScanOptions::full`).

// The scheduler and watcher (M1-06) and commands (M1-09) are the first users.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, Transaction};

use crate::formats::{NON_RAW_EXTENSIONS, RAW_EXTENSIONS};
use crate::pv::catalog::{Catalog, now_ms};
use crate::pv::locations::Location;

/// Folder names never indexed: NAS system folders and Pairview's own Recently Deleted.
/// Names starting with `.` (dotfiles, `._` AppleDouble, `.DS_Store`) are skipped as well.
pub const DEFAULT_EXCLUDES: &[&str] = &[
    "@eaDir",
    "#recycle",
    "#snapshot",
    "$RECYCLE.BIN",
    "System Volume Information",
    ".pairview-deleted",
];

/// Format sub-folders that belong to their parent shoot (PRD §5.1).
pub const FORMAT_SUBDIRS: &[&str] = &["arw", "raw", "hif", "heif", "jpg", "jpeg"];

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "m4v", "mts", "m2ts", "avi", "mkv"];
const SIDECAR_EXTENSIONS: &[&str] = &["xmp", "rrdata"];

/// A folder modified within this window of its last listing may have changed again in the same
/// mtime tick (SMB often has 1 s or 2 s resolution), so it's listed again next time.
const SETTLE_MS: i64 = 2_000;

#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// List every folder, ignoring mtimes.
    pub full: bool,
    /// Extra folder names, or paths relative to the location, to skip.
    pub excludes: Vec<String>,
}

impl ScanOptions {
    /// Reads per-location excludes from `settings_json` (`{"excludes": ["exports", …]}`).
    pub fn for_location(location: &Location) -> Self {
        let excludes = serde_json::from_str::<serde_json::Value>(&location.settings_json)
            .ok()
            .and_then(|v| v.get("excludes").cloned())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        Self {
            full: false,
            excludes,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanStats {
    pub folders_listed: usize,
    pub folders_unchanged: usize,
    pub folders_added: usize,
    pub folders_removed: usize,
    pub files_added: usize,
    pub files_changed: usize,
    pub files_removed: usize,
    /// Files skipped this time because they may still be arriving (see `Stability`).
    pub files_held: usize,
    /// Folders whose files or sub-folders changed. Their shoots need pairing again (M1-04).
    pub changed_folders: Vec<i64>,
}

impl ScanStats {
    pub fn changed(&self) -> bool {
        self.changes() > 0
    }

    fn changes(&self) -> usize {
        self.folders_added
            + self.folders_removed
            + self.files_added
            + self.files_changed
            + self.files_removed
    }
}

/// Catalog `files.kind` for an extension, or `None` for files Pairview doesn't index.
pub fn file_kind(ext: &str) -> Option<&'static str> {
    let ext = ext.to_ascii_lowercase();
    if RAW_EXTENSIONS.iter().any(|(e, _)| *e == ext) {
        Some("raw")
    } else if NON_RAW_EXTENSIONS.contains(&ext.as_str()) {
        Some("display")
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Some("video")
    } else if SIDECAR_EXTENSIONS.contains(&ext.as_str()) {
        Some("sidecar")
    } else {
        None
    }
}

pub fn is_format_subdir(name: &str) -> bool {
    FORMAT_SUBDIRS.iter().any(|d| d.eq_ignore_ascii_case(name))
}

/// Holds back files that may still be arriving (an FTP upload from the camera, a copy in
/// progress): a recently modified file is recorded only once two scans in a row see the same
/// size and mtime. Older files are never held, so a first scan of an existing library isn't
/// delayed.
#[derive(Debug, Default)]
pub struct Stability {
    recent_ms: i64,
    seen: HashMap<PathBuf, (i64, i64)>,
}

impl Stability {
    /// `recent`: how old a file's mtime must be before it's trusted without a second look.
    /// The watcher uses twice its poll interval.
    pub fn new(recent: std::time::Duration) -> Self {
        Self {
            recent_ms: recent.as_millis() as i64,
            seen: HashMap::new(),
        }
    }

    fn hold(&mut self, path: &Path, size: i64, mtime: i64, now: i64) -> bool {
        // abs(): a NAS clock running ahead makes fresh files look like they're from the future.
        if (now - mtime).abs() > self.recent_ms {
            self.seen.remove(path);
            return false;
        }
        self.seen.insert(path.to_path_buf(), (size, mtime)) != Some((size, mtime))
    }
}

/// Scans `location` into the catalog in one transaction.
pub fn scan_location(
    cat: &mut Catalog,
    location: &Location,
    opts: &ScanOptions,
) -> Result<ScanStats> {
    scan(cat, location, opts, &mut |_, _, _| false)
}

/// Like `scan_location`, but holds back files that may still be arriving (see `Stability`).
pub fn scan_location_stable(
    cat: &mut Catalog,
    location: &Location,
    opts: &ScanOptions,
    stability: &mut Stability,
) -> Result<ScanStats> {
    let now = now_ms();
    scan(cat, location, opts, &mut |path, size, mtime| {
        stability.hold(path, size, mtime, now)
    })
}

type Hold<'a> = dyn FnMut(&Path, i64, i64) -> bool + 'a;

fn scan(
    cat: &mut Catalog,
    location: &Location,
    opts: &ScanOptions,
    hold: &mut Hold,
) -> Result<ScanStats> {
    let root = location.path.clone();
    if !fs::metadata(&root)
        .with_context(|| format!("cannot read {}", root.display()))?
        .is_dir()
    {
        bail!("{} is not a folder", root.display());
    }

    let tx = cat.conn_mut().transaction()?;
    let mut stats = ScanStats::default();
    let root_id = match folder_id(&tx, location.id, "")? {
        Some(id) => id,
        None => {
            stats.folders_added += 1;
            insert_folder(&tx, location.id, None, "", &default_root_name(location))?
        }
    };

    let mut stack = vec![(root_id, root, String::new())];
    while let Some((id, dir, rel)) = stack.pop() {
        scan_folder(
            &tx,
            location.id,
            id,
            &dir,
            &rel,
            opts,
            hold,
            &mut stats,
            &mut stack,
        )?;
    }
    tx.commit()?;
    Ok(stats)
}

struct DbFolder {
    mtime: Option<i64>,
    scanned_at: Option<i64>,
}

struct DbFile {
    id: i64,
    size: i64,
    mtime: i64,
}

#[allow(clippy::too_many_arguments)]
fn scan_folder(
    tx: &Transaction,
    location_id: i64,
    id: i64,
    dir: &Path,
    rel: &str,
    opts: &ScanOptions,
    hold: &mut Hold,
    stats: &mut ScanStats,
    stack: &mut Vec<(i64, PathBuf, String)>,
) -> Result<()> {
    let mtime = mtime_ms(&fs::metadata(dir)?);
    let db: DbFolder = tx.query_row(
        "SELECT mtime, scanned_at FROM folders WHERE id = ?1",
        [id],
        |r| {
            Ok(DbFolder {
                mtime: r.get(0)?,
                scanned_at: r.get(1)?,
            })
        },
    )?;
    let settled = db
        .scanned_at
        .is_some_and(|scanned| mtime <= scanned - SETTLE_MS);

    if !opts.full && db.mtime == Some(mtime) && settled {
        stats.folders_unchanged += 1;
        let before = stats.changes();
        let mut stmt = tx.prepare_cached("SELECT id, name FROM folders WHERE parent_id = ?1")?;
        let children = stmt
            .query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (child_id, name) in children {
            let child = dir.join(&name);
            if child.is_dir() {
                stack.push((child_id, child, join_rel(rel, &name)));
            } else {
                // Gone although the parent's mtime says otherwise (clock skew): forget it.
                tx.execute("DELETE FROM folders WHERE id = ?1", [child_id])?;
                stats.folders_removed += 1;
            }
        }
        if stats.changes() != before {
            stats.changed_folders.push(id);
        }
        return Ok(());
    }

    stats.folders_listed += 1;
    let before = stats.changes();
    let mut subdirs: Vec<String> = Vec::new();
    let mut files: Vec<(String, i64, i64)> = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("cannot list {}", dir.display()))? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue; // Not valid UTF-8: can't be stored or shown reliably.
        };
        if name.starts_with('.') {
            continue;
        }
        // Symlinks are not followed, so a link loop can't make the scan run forever.
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if !is_excluded(&name, &join_rel(rel, &name), opts) {
                subdirs.push(name);
            }
        } else if file_type.is_file()
            && Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .and_then(file_kind)
                .is_some()
        {
            let meta = entry.metadata()?;
            files.push((name, meta.len() as i64, mtime_ms(&meta)));
        }
    }

    // Sub-folders.
    let mut known: HashMap<String, i64> = {
        let mut stmt = tx.prepare_cached("SELECT name, id FROM folders WHERE parent_id = ?1")?;
        stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for name in subdirs {
        let child_rel = join_rel(rel, &name);
        let child_id = match known.remove(&name) {
            Some(child_id) => child_id,
            None => {
                stats.folders_added += 1;
                insert_folder(tx, location_id, Some(id), &child_rel, &name)?
            }
        };
        stack.push((child_id, dir.join(&name), child_rel));
    }
    for (_, gone) in known {
        tx.execute("DELETE FROM folders WHERE id = ?1", [gone])?;
        stats.folders_removed += 1;
    }

    // Files.
    let mut known: HashMap<String, DbFile> = {
        let mut stmt =
            tx.prepare_cached("SELECT name, id, size, mtime FROM files WHERE folder_id = ?1")?;
        stmt.query_map([id], |r| {
            Ok((
                r.get(0)?,
                DbFile {
                    id: r.get(1)?,
                    size: r.get(2)?,
                    mtime: r.get(3)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let held_before = stats.files_held;
    for (name, size, file_mtime) in files {
        if hold(&dir.join(&name), size, file_mtime) {
            // Not recorded yet, and an existing row is left as it was.
            known.remove(&name);
            stats.files_held += 1;
            continue;
        }
        match known.remove(&name) {
            Some(f) if f.size == size && f.mtime == file_mtime => {}
            Some(f) => {
                tx.execute(
                    "UPDATE files SET size = ?2, mtime = ?3 WHERE id = ?1",
                    (f.id, size, file_mtime),
                )?;
                stats.files_changed += 1;
            }
            None => {
                let (stem, ext) = split_name(&name);
                tx.execute(
                    "INSERT INTO files (folder_id, name, stem_key, ext, kind, size, mtime)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    (
                        id,
                        &name,
                        stem.to_lowercase(),
                        ext.to_ascii_lowercase(),
                        file_kind(ext),
                        size,
                        file_mtime,
                    ),
                )?;
                stats.files_added += 1;
            }
        }
    }
    for (_, gone) in known {
        tx.execute("DELETE FROM files WHERE id = ?1", [gone.id])?;
        stats.files_removed += 1;
    }

    if stats.changes() != before {
        stats.changed_folders.push(id);
    }
    // With files held back, leave the folder unmarked so the next scan lists it again.
    if stats.files_held != held_before {
        return Ok(());
    }
    if db.mtime != Some(mtime) || !settled {
        tx.execute(
            "UPDATE folders SET mtime = ?2, scanned_at = ?3 WHERE id = ?1",
            (id, mtime, now_ms()),
        )?;
    }
    Ok(())
}

fn folder_id(tx: &Transaction, location_id: i64, rel: &str) -> Result<Option<i64>> {
    Ok(tx
        .query_row(
            "SELECT id FROM folders WHERE location_id = ?1 AND rel_path = ?2",
            (location_id, rel),
            |r| r.get(0),
        )
        .optional()?)
}

fn insert_folder(
    tx: &Transaction,
    location_id: i64,
    parent_id: Option<i64>,
    rel: &str,
    name: &str,
) -> Result<i64> {
    tx.execute(
        "INSERT INTO folders (location_id, parent_id, rel_path, name, format_subdir)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        (
            location_id,
            parent_id,
            rel,
            name,
            parent_id.is_some() && is_format_subdir(name),
        ),
    )?;
    Ok(tx.last_insert_rowid())
}

fn is_excluded(name: &str, rel: &str, opts: &ScanOptions) -> bool {
    DEFAULT_EXCLUDES
        .iter()
        .any(|e| e.eq_ignore_ascii_case(name))
        || opts
            .excludes
            .iter()
            .any(|e| e.eq_ignore_ascii_case(name) || e.trim_matches('/') == rel)
}

fn default_root_name(location: &Location) -> String {
    location
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `DSC03545.ARW` → (`DSC03545`, `ARW`); `DSC03545.ARW.rrdata` → (`DSC03545.ARW`, `rrdata`).
pub fn split_name(name: &str) -> (&str, &str) {
    name.rsplit_once('.').unwrap_or((name, ""))
}

fn join_rel(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn mtime_ms(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pv::locations;
    use std::time::{Duration, Instant, SystemTime};

    /// Sets a path's mtime into the past, so tests see settled folders.
    fn age(path: &Path, by: Duration) {
        let t = filetime::FileTime::from_system_time(SystemTime::now() - by);
        filetime::set_file_mtime(path, t).unwrap();
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        cat: Catalog,
        loc: Location,
    }

    fn fixture(files: &[&str]) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("DCIM");
        for f in files {
            let p = root.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            if !f.ends_with('/') {
                fs::write(&p, b"x").unwrap();
            }
        }
        fs::create_dir_all(&root).unwrap();
        let cat = Catalog::open_in_memory().unwrap();
        let id = locations::add(&cat, &root, None).unwrap();
        let loc = locations::get(&cat, id).unwrap().unwrap();
        Fixture {
            _tmp: tmp,
            root,
            cat,
            loc,
        }
    }

    fn age_tree(root: &Path) {
        for entry in walkdir::WalkDir::new(root).contents_first(true) {
            age(entry.unwrap().path(), Duration::from_secs(3600));
        }
    }

    fn names(cat: &Catalog, sql: &str) -> Vec<String> {
        let mut stmt = cat.conn().prepare(sql).unwrap();
        let mut v: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        v.sort();
        v
    }

    fn scan(f: &mut Fixture) -> ScanStats {
        scan_location(&mut f.cat, &f.loc, &ScanOptions::for_location(&f.loc)).unwrap()
    }

    #[test]
    fn records_media_and_skips_everything_else() {
        let mut f = fixture(&[
            "2026-10-24_10361024/arw/DSC03545.ARW",
            "2026-10-24_10361024/hif/DSC03545.HIF",
            "2026-10-24_10361024/arw/DSC03545.ARW.rrdata",
            "10561031/DSC03600.ARW",
            "10561031/DSC03600.JPG",
            "10561031/C0001.MP4",
            "10561031/DSC03600.xmp",
            "10561031/notes.txt",
            "10561031/.DS_Store",
            "10561031/._DSC03600.ARW",
            "@eaDir/DSC03600.ARW/SYNOPHOTO_THUMB_M.jpg",
            "#recycle/old.jpg",
            ".pairview-deleted/op-1/DSC0001.ARW",
            ".hidden/x.jpg",
        ]);
        let stats = scan(&mut f);
        assert_eq!(stats.files_added, 7);

        assert_eq!(
            names(&f.cat, "SELECT rel_path FROM folders"),
            [
                "",
                "10561031",
                "2026-10-24_10361024",
                "2026-10-24_10361024/arw",
                "2026-10-24_10361024/hif"
            ]
        );
        assert_eq!(
            names(
                &f.cat,
                "SELECT name || ':' || kind || ':' || stem_key || ':' || ext FROM files"
            ),
            [
                "C0001.MP4:video:c0001:mp4",
                "DSC03545.ARW.rrdata:sidecar:dsc03545.arw:rrdata",
                "DSC03545.ARW:raw:dsc03545:arw",
                "DSC03545.HIF:display:dsc03545:hif",
                "DSC03600.ARW:raw:dsc03600:arw",
                "DSC03600.JPG:display:dsc03600:jpg",
                "DSC03600.xmp:sidecar:dsc03600:xmp",
            ]
        );
        assert_eq!(
            names(
                &f.cat,
                "SELECT rel_path FROM folders WHERE format_subdir = 1"
            ),
            ["2026-10-24_10361024/arw", "2026-10-24_10361024/hif"]
        );
    }

    #[test]
    fn per_location_excludes() {
        let mut f = fixture(&[
            "exports/a.jpg",
            "2026/exports/b.jpg",
            "2026/c.jpg",
            "keep/d.jpg",
        ]);
        f.loc.settings_json = r#"{"excludes": ["2026/exports", "EXPORTS"]}"#.into();
        scan(&mut f);
        assert_eq!(names(&f.cat, "SELECT name FROM files"), ["c.jpg", "d.jpg"]);
    }

    #[test]
    fn rescan_without_changes_writes_nothing() {
        let mut f = fixture(&[
            "a/arw/DSC00001.ARW",
            "a/hif/DSC00001.HIF",
            "b/DSC00002.ARW",
            "b/DSC00002.JPG",
        ]);
        age_tree(&f.root);
        scan(&mut f);
        let writes = f.cat.conn().total_changes();
        let stats = scan(&mut f);
        assert_eq!(f.cat.conn().total_changes(), writes, "{stats:?}");
        assert_eq!(stats.folders_listed, 0);
        assert_eq!(stats.folders_unchanged, 5);
    }

    #[test]
    fn rescan_picks_up_adds_removes_and_changes() {
        let mut f = fixture(&[
            "a/DSC00001.ARW",
            "a/DSC00001.JPG",
            "b/DSC00002.ARW",
            "c/x.jpg",
        ]);
        age_tree(&f.root);
        scan(&mut f);

        fs::write(f.root.join("a/DSC00003.ARW"), b"x").unwrap(); // added
        fs::remove_file(f.root.join("a/DSC00001.JPG")).unwrap(); // removed
        fs::write(f.root.join("b/DSC00002.ARW"), b"longer").unwrap(); // changed in place…
        age(&f.root.join("b"), Duration::from_secs(1)); // …and its folder touched
        fs::remove_dir_all(f.root.join("c")).unwrap(); // folder removed

        let stats = scan(&mut f);
        assert_eq!(
            (
                stats.files_added,
                stats.files_removed,
                stats.files_changed,
                stats.folders_removed
            ),
            (1, 1, 1, 1),
            "{stats:?}"
        );
        assert_eq!(
            names(&f.cat, "SELECT name FROM files"),
            ["DSC00001.ARW", "DSC00002.ARW", "DSC00003.ARW"]
        );
    }

    #[test]
    fn full_scan_finds_in_place_edits_the_quick_scan_skips() {
        let mut f = fixture(&["a/DSC00001.JPG"]);
        age_tree(&f.root);
        scan(&mut f);
        fs::write(f.root.join("a/DSC00001.JPG"), b"rewritten").unwrap();
        age(&f.root.join("a/DSC00001.JPG"), Duration::from_secs(60));

        assert_eq!(scan(&mut f).files_changed, 0);
        let opts = ScanOptions {
            full: true,
            ..Default::default()
        };
        let stats = scan_location(&mut f.cat, &f.loc, &opts).unwrap();
        assert_eq!(stats.files_changed, 1);
    }

    #[test]
    fn freshly_modified_folder_is_listed_again_until_settled() {
        let mut f = fixture(&["a/DSC00001.JPG"]);
        scan(&mut f);
        // The folders were modified moments before the first listing, so they could still
        // change within the same mtime tick: the next scan lists them again.
        assert!(scan(&mut f).folders_listed > 0);
    }

    /// `PV_FIXTURES=~/Desktop/2026-10-23_10261023 cargo test --lib pv::scan -- --nocapture`
    #[test]
    fn real_fixture_scan_is_fast() {
        let Ok(dir) = std::env::var("PV_FIXTURES") else {
            return;
        };
        let mut cat = Catalog::open_in_memory().unwrap();
        let id = locations::add(&cat, Path::new(&dir), None).unwrap();
        let loc = locations::get(&cat, id).unwrap().unwrap();
        let opts = ScanOptions::for_location(&loc);

        let start = Instant::now();
        let first = scan_location(&mut cat, &loc, &opts).unwrap();
        let first_time = start.elapsed();
        let writes = cat.conn().total_changes();
        let start = Instant::now();
        let second = scan_location(&mut cat, &loc, &opts).unwrap();
        println!(
            "first {first_time:?} {first:?}\nsecond {:?} {second:?}",
            start.elapsed()
        );

        assert!(first_time < Duration::from_secs(2));
        assert!(first.files_added > 0);
        assert!(!second.changed());
        assert_eq!(cat.conn().total_changes(), writes);
    }
}
