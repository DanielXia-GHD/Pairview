//! Safety test suite (docs/IMPLEMENTATION_PLAN.md §4.3) for `pv/lifecycle`. Temp dirs only.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::test_hooks::{FAKE_DEVICE, INTERRUPT, Interrupt};
use super::*;
use crate::pv::catalog::Catalog;
use crate::pv::locations;
use crate::pv::scan::l0::{ScanOptions, scan_location};

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cat: Catalog,
    location: i64,
}

impl Fixture {
    /// Files get distinct contents so byte-identical checks mean something.
    fn new(files: &[&str]) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        // Canonical, as the location stores it (`/var` is `/private/var` on macOS).
        let root = fs::canonicalize(tmp.path()).unwrap().join("DCIM");
        for (i, f) in files.iter().enumerate() {
            let p = root.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, format!("{f} #{i} {}", "x".repeat(i * 7))).unwrap();
        }
        fs::create_dir_all(&root).unwrap();
        let cat = Catalog::open_in_memory().unwrap();
        let location = locations::add(&cat, &root, Some("Test")).unwrap();
        let mut f = Fixture {
            _tmp: tmp,
            root,
            cat,
            location,
        };
        f.rescan();
        f
    }

    fn rescan(&mut self) {
        let loc = locations::get(&self.cat, self.location).unwrap().unwrap();
        let scan = scan_location(&mut self.cat, &loc, &ScanOptions::default()).unwrap();
        assets::refresh(&mut self.cat, &scan.changed_folders).unwrap();
    }

    /// Sets EXIF capture times (as L1 would) and re-pairs every shoot.
    fn exif(&mut self, dates: &[(&str, &str)]) {
        for (name, date) in dates {
            self.cat
                .conn()
                .execute(
                    "INSERT OR REPLACE INTO exif (file_id, file_mtime, date_original, read_at)
                     SELECT id, mtime, ?2, 0 FROM files WHERE name = ?1",
                    (name, date),
                )
                .unwrap();
        }
        let folders: Vec<i64> = {
            let mut s = self.cat.conn().prepare("SELECT id FROM folders").unwrap();
            s.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assets::refresh(&mut self.cat, &folders).unwrap();
    }

    fn photo(&self, stem: &str) -> i64 {
        self.cat
            .conn()
            .query_row(
                "SELECT id FROM photos WHERE stem_key = ?1 ORDER BY id LIMIT 1",
                [stem],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn photo_with(&self, role: &str, name: &str) -> i64 {
        self.cat
            .conn()
            .query_row(
                "SELECT m.photo_id FROM photo_members m JOIN files f ON f.id = m.file_id
                 WHERE m.role = ?1 AND f.name = ?2",
                [role, name],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn delete(&mut self, photos: &[i64], scope: Scope) -> Result<Done> {
        let plan = plan_photos(&self.cat, photos, scope).unwrap();
        assert_eq!(plan.ops.len(), 1, "{plan:?}");
        apply(&mut self.cat, &plan).pop().unwrap().1
    }

    /// Every file outside Recently Deleted, with its contents.
    fn library(&self) -> BTreeMap<String, Vec<u8>> {
        snapshot(&self.root, &|rel| !rel.starts_with(TRASH_DIR))
    }

    fn trash(&self) -> BTreeMap<String, Vec<u8>> {
        snapshot(&self.root.join(TRASH_DIR), &|_| true)
    }

    fn files_in_catalog(&self) -> i64 {
        self.cat
            .conn()
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap()
    }

    fn photos_in_catalog(&self) -> i64 {
        self.cat
            .conn()
            .query_row("SELECT COUNT(*) FROM photos", [], |r| r.get(0))
            .unwrap()
    }

    fn op_status(&self, kind: &str) -> Vec<String> {
        let mut s = self
            .cat
            .conn()
            .prepare("SELECT status FROM ops_log WHERE kind = ?1 ORDER BY created_at, id")
            .unwrap();
        s.query_map([kind], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn go_offline(&self) {
        let gone = self.root.parent().unwrap().join("unplugged");
        self.cat
            .conn()
            .execute("UPDATE locations SET path = ?1", [gone.to_str().unwrap()])
            .unwrap();
    }
}

fn snapshot(base: &Path, keep: &dyn Fn(&str) -> bool) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let rel = p
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if !keep(&rel) {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(rel, fs::read(&p).unwrap());
            }
        }
    }
    out
}

const SHOOT: &[&str] = &[
    "2026-10-23_1/arw/DSC0001.ARW",
    "2026-10-23_1/arw/DSC0001.xmp",
    "2026-10-23_1/arw/DSC0001.ARW.rrdata",
    "2026-10-23_1/hif/DSC0001.HIF",
    "2026-10-23_1/hif/DSC0001.HIF.rrdata",
    "2026-10-23_1/arw/DSC0002.ARW",
    "2026-10-23_1/hif/DSC0002.HIF",
];

// 1. Delete moves into Recently Deleted with a correct manifest; restore is byte-identical.

#[test]
fn delete_photo_moves_every_member_and_restore_puts_them_back() {
    let mut f = Fixture::new(SHOOT);
    let before = f.library();
    let photo = f.photo("dsc0001");

    let done = f.delete(&[photo], Scope::Photo).unwrap();
    assert_eq!(done.files, 5);

    let library = f.library();
    assert_eq!(
        library.keys().collect::<Vec<_>>(),
        [
            "2026-10-23_1/arw/DSC0002.ARW",
            "2026-10-23_1/hif/DSC0002.HIF"
        ]
    );
    let trash = f.trash();
    for rel in &SHOOT[..5] {
        let in_trash = format!("{}/{rel}", done.op_id);
        assert_eq!(trash.get(&in_trash), before.get(*rel), "{rel}");
    }
    let manifest = Manifest::read(&f.root.join(TRASH_DIR).join(&done.op_id)).unwrap();
    assert_eq!(manifest.status, manifest::Status::Committed);
    assert_eq!(manifest.reason, Reason::Deleted);
    assert_eq!(manifest.files.len(), 5);
    assert_eq!(manifest.photos.len(), 1);
    assert_eq!(manifest.photos[0].stem, "dsc0001");
    assert_eq!(manifest.photos[0].shoot, "2026-10-23_1");
    assert_eq!(
        manifest.bytes(),
        SHOOT[..5]
            .iter()
            .map(|r| before[*r].len() as u64)
            .sum::<u64>()
    );
    assert_eq!(f.op_status("delete"), ["committed"]);
    // The catalog followed.
    assert_eq!(f.photos_in_catalog(), 1);
    assert_eq!(f.files_in_catalog(), 2);

    let listed = list(&mut f.cat, f.location).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].expires_at,
        manifest.committed_at.map(|t| t + RETENTION_MS)
    );

    let restored = restore(&mut f.cat, f.location, &done.op_id, None).unwrap();
    assert_eq!((restored.files, restored.conflicts.len()), (5, 0));
    assert_eq!(f.library(), before);
    assert!(!f.root.join(TRASH_DIR).join(&done.op_id).exists());
    assert_eq!(f.photos_in_catalog(), 2);
    assert_eq!(f.files_in_catalog(), 7);
    assert!(list(&mut f.cat, f.location).unwrap().is_empty());
}

#[test]
fn keep_display_drops_the_raw_and_keeps_the_rating() {
    let mut f = Fixture::new(SHOOT);
    let before = f.library();
    let photo = f.photo("dsc0001");

    let done = f.delete(&[photo], Scope::KeepDisplay).unwrap();
    let manifest = Manifest::read(&f.root.join(TRASH_DIR).join(&done.op_id)).unwrap();
    assert_eq!(manifest.reason, Reason::Slimmed);
    assert_eq!(manifest.kind, "drop_raw");
    let mut moved: Vec<_> = manifest.files.iter().map(|m| m.path.as_str()).collect();
    moved.sort();
    assert_eq!(
        moved,
        [
            "2026-10-23_1/arw/DSC0001.ARW",
            "2026-10-23_1/arw/DSC0001.ARW.rrdata",
            "2026-10-23_1/arw/DSC0001.xmp"
        ]
    );
    let library = f.library();
    assert_eq!(
        library["2026-10-23_1/hif/DSC0001.HIF"],
        before["2026-10-23_1/hif/DSC0001.HIF"]
    );
    assert!(library.contains_key("2026-10-23_1/hif/DSC0001.HIF.rrdata"));
    // The Photo-level XMP now sits next to the HIF, identical.
    assert_eq!(
        library["2026-10-23_1/hif/DSC0001.xmp"],
        before["2026-10-23_1/arw/DSC0001.xmp"]
    );
    // Same Photo, now display only.
    let state: String = f
        .cat
        .conn()
        .query_row("SELECT state FROM photos WHERE id = ?1", [photo], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(state, "display_only");
}

#[test]
fn keep_raw_drops_display_copies_only() {
    let mut f = Fixture::new(&["s/DSC0001.ARW", "s/DSC0001.HIF", "s/DSC0001.xmp"]);
    let photo = f.photo("dsc0001");
    f.delete(&[photo], Scope::KeepRaw).unwrap();
    assert_eq!(
        f.library().keys().collect::<Vec<_>>(),
        ["s/DSC0001.ARW", "s/DSC0001.xmp"]
    );
}

#[test]
fn scopes_that_do_not_apply_are_skipped() {
    let f = Fixture::new(&[
        "s/DSC0001.ARW",
        "s/DSC0002.HIF",
        "s/DSC0003.ARW",
        "s/DSC0003.HIF",
    ]);
    f.cat
        .conn()
        .execute(
            "INSERT INTO metadata (photo_id, keep_raw, updated_at) VALUES (?1, 'keep', 0)",
            [f.photo("dsc0003")],
        )
        .unwrap();
    let ids = [
        f.photo("dsc0001"),
        f.photo("dsc0002"),
        f.photo("dsc0003"),
        9999,
    ];
    let plan = plan_photos(&f.cat, &ids, Scope::KeepDisplay).unwrap();
    assert!(plan.ops.is_empty());
    let reasons: Vec<_> = plan.skipped.iter().map(|s| s.reason).collect();
    assert_eq!(
        reasons,
        [
            SkipReason::NoDisplay,
            SkipReason::NoRaw,
            SkipReason::Locked,
            SkipReason::Gone
        ]
    );
    let plan = plan_photos(&f.cat, &ids, Scope::KeepRaw).unwrap();
    assert_eq!(plan.photos(), 1); // dsc0003: the lock protects the RAW, not the HIF.
}

#[test]
fn restore_leaves_a_retaken_name_in_recently_deleted() {
    let mut f = Fixture::new(SHOOT);
    let photo = f.photo("dsc0002");
    let done = f.delete(&[photo], Scope::Photo).unwrap();
    fs::write(f.root.join("2026-10-23_1/hif/DSC0002.HIF"), b"new").unwrap();

    let restored = restore(&mut f.cat, f.location, &done.op_id, None).unwrap();
    assert_eq!(restored.files, 1);
    assert_eq!(restored.conflicts, ["2026-10-23_1/hif/DSC0002.HIF"]);
    assert_eq!(f.library()["2026-10-23_1/hif/DSC0002.HIF"], b"new");
    let left = list(&mut f.cat, f.location).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].manifest.files.len(), 1);
}

// 2. Pairs the timestamp guard split are never treated as a pair.

#[test]
fn guard_split_pair_is_never_deleted_together() {
    let mut f = Fixture::new(&["s/DSC0001.ARW", "s/DSC0001.HIF"]);
    f.exif(&[
        ("DSC0001.ARW", "2026-10-23T10:00:00"),
        ("DSC0001.HIF", "2026-10-24T18:30:00"),
    ]);
    let raw_photo = f.photo_with("raw", "DSC0001.ARW");
    let hif_photo = f.photo_with("display", "DSC0001.HIF");
    assert_ne!(raw_photo, hif_photo);

    // Keep HIF only makes no sense for a RAW whose "HIF" is another frame.
    let plan = plan_photos(&f.cat, &[raw_photo], Scope::KeepDisplay).unwrap();
    assert!(plan.ops.is_empty());
    assert_eq!(plan.skipped[0].reason, SkipReason::NoDisplay);

    f.delete(&[raw_photo], Scope::Photo).unwrap();
    assert_eq!(f.library().keys().collect::<Vec<_>>(), ["s/DSC0001.HIF"]);
}

// 3. Cross-volume moves are refused.

#[test]
fn file_on_another_volume_refuses_the_whole_operation() {
    let mut f = Fixture::new(SHOOT);
    let before = f.library();
    let photos = [f.photo("dsc0001"), f.photo("dsc0002")];
    let plan = plan_photos(&f.cat, &photos, Scope::Photo).unwrap();
    // `hif/` is a different mount.
    FAKE_DEVICE.with(|d| *d.borrow_mut() = Some((f.root.join("2026-10-23_1/hif"), 4242)));
    let err = apply(&mut f.cat, &plan).pop().unwrap().1.unwrap_err();
    FAKE_DEVICE.with(|d| *d.borrow_mut() = None);
    assert!(err.to_string().contains("different volume"), "{err}");
    assert_eq!(f.library(), before);
    assert!(f.trash().is_empty());
    assert!(f.op_status("delete").is_empty());
}

#[cfg(unix)]
#[test]
fn symlinked_recently_deleted_is_refused() {
    let mut f = Fixture::new(SHOOT);
    let elsewhere = f.root.parent().unwrap().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, f.root.join(TRASH_DIR)).unwrap();
    let before = f.library();
    let err = f.delete(&[f.photo("dsc0001")], Scope::Photo).unwrap_err();
    assert!(err.to_string().contains("not a plain folder"), "{err}");
    assert_eq!(f.library(), before);
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

// 4. Offline location → every mutation is refused.

#[test]
fn offline_location_refuses_everything() {
    let mut f = Fixture::new(SHOOT);
    let done = f.delete(&[f.photo("dsc0002")], Scope::Photo).unwrap();
    let plan = plan_photos(&f.cat, &[f.photo("dsc0001")], Scope::Photo).unwrap();
    let everything = snapshot(&f.root, &|_| true);
    f.go_offline();

    let err = apply(&mut f.cat, &plan).pop().unwrap().1.unwrap_err();
    assert!(err.to_string().contains("offline"), "{err}");
    let err = restore(&mut f.cat, f.location, &done.op_id, None).unwrap_err();
    assert!(err.to_string().contains("offline"), "{err}");
    let err = purge(&mut f.cat, f.location, None, None).unwrap_err();
    assert!(err.to_string().contains("offline"), "{err}");
    assert!(list(&mut f.cat, f.location).is_err());

    assert_eq!(snapshot(&f.root, &|_| true), everything);
}

// 5. A crash in the middle → the op-log replay leaves no half-state.

#[test]
fn crash_mid_delete_is_rolled_back_by_recover() {
    let mut f = Fixture::new(SHOOT);
    let before = f.library();
    let photos = [f.photo("dsc0001"), f.photo("dsc0002")];
    let plan = plan_photos(&f.cat, &photos, Scope::Photo).unwrap();

    INTERRUPT.with(|i| i.set(Some(Interrupt::Crash(3))));
    let err = apply(&mut f.cat, &plan).pop().unwrap().1.unwrap_err();
    INTERRUPT.with(|i| i.set(None));
    assert!(err.is::<Crash>(), "{err}");
    // Half done, as a real crash would leave it.
    assert_eq!(f.library().len(), before.len() - 3);
    assert_eq!(f.op_status("delete"), ["pending"]);

    let r = recover(&mut f.cat).unwrap();
    assert_eq!((r.rolled_back, r.failed, r.waiting), (1, 0, 0));
    assert_eq!(f.library(), before);
    assert!(f.trash().is_empty(), "{:?}", f.trash().keys());
    assert_eq!(f.op_status("delete"), ["rolled_back"]);
    // The catalog was never changed.
    assert_eq!(f.files_in_catalog(), 7);
    // Replaying twice is harmless.
    assert_eq!(recover(&mut f.cat).unwrap().rolled_back, 0);
}

#[test]
fn crash_mid_keep_display_removes_the_xmp_copy_too() {
    let mut f = Fixture::new(SHOOT);
    let before = f.library();
    let plan = plan_photos(&f.cat, &[f.photo("dsc0001")], Scope::KeepDisplay).unwrap();
    INTERRUPT.with(|i| i.set(Some(Interrupt::Crash(1))));
    let _ = apply(&mut f.cat, &plan);
    INTERRUPT.with(|i| i.set(None));
    assert!(f.library().contains_key("2026-10-23_1/hif/DSC0001.xmp"));
    recover(&mut f.cat).unwrap();
    assert_eq!(f.library(), before);
}

#[test]
fn failed_move_is_rolled_back_at_once() {
    let mut f = Fixture::new(SHOOT);
    let before = f.library();
    let plan = plan_photos(&f.cat, &[f.photo("dsc0001")], Scope::Photo).unwrap();
    INTERRUPT.with(|i| i.set(Some(Interrupt::Fail(2))));
    let err = apply(&mut f.cat, &plan).pop().unwrap().1.unwrap_err();
    INTERRUPT.with(|i| i.set(None));
    assert!(
        format!("{err:#}").contains("nothing was changed"),
        "{err:#}"
    );
    assert_eq!(f.library(), before);
    assert!(f.trash().is_empty());
    assert_eq!(f.op_status("delete"), ["rolled_back"]);
}

#[test]
fn crash_mid_restore_is_rolled_back() {
    let mut f = Fixture::new(SHOOT);
    let done = f.delete(&[f.photo("dsc0001")], Scope::Photo).unwrap();
    let trash = f.trash();
    INTERRUPT.with(|i| i.set(Some(Interrupt::Crash(2))));
    let _ = restore(&mut f.cat, f.location, &done.op_id, None);
    INTERRUPT.with(|i| i.set(None));
    // The next operation on the location replays the op-log first.
    assert_eq!(list(&mut f.cat, f.location).unwrap().len(), 1);
    assert_eq!(f.trash(), trash);
    assert_eq!(f.op_status("restore"), ["rolled_back"]);
    // And restoring again works.
    assert_eq!(
        restore(&mut f.cat, f.location, &done.op_id, None)
            .unwrap()
            .files,
        5
    );
}

#[test]
fn pending_operation_on_an_offline_location_waits() {
    let mut f = Fixture::new(SHOOT);
    let plan = plan_photos(&f.cat, &[f.photo("dsc0001")], Scope::Photo).unwrap();
    INTERRUPT.with(|i| i.set(Some(Interrupt::Crash(1))));
    let _ = apply(&mut f.cat, &plan);
    INTERRUPT.with(|i| i.set(None));
    let real = f.root.to_str().unwrap().to_string();
    f.go_offline();
    assert_eq!(recover(&mut f.cat).unwrap().waiting, 1);
    f.cat
        .conn()
        .execute("UPDATE locations SET path = ?1", [real])
        .unwrap();
    assert_eq!(recover(&mut f.cat).unwrap().rolled_back, 1);
    assert!(f.trash().is_empty());
}

// 6. Purge deletes only paths listed in manifests older than retention.

#[test]
fn purge_deletes_only_listed_files_of_expired_operations() {
    let mut f = Fixture::new(SHOOT);
    let old = f.delete(&[f.photo("dsc0001")], Scope::Photo).unwrap();
    let new = f.delete(&[f.photo("dsc0002")], Scope::Photo).unwrap();
    let old_dir = f.root.join(TRASH_DIR).join(&old.op_id);
    // Age the first operation past retention.
    let mut m = Manifest::read(&old_dir).unwrap();
    let now = now_ms();
    m.committed_at = Some(now - RETENTION_MS - DAY_MS);
    m.write(&old_dir).unwrap();
    // Something the manifest doesn't list.
    fs::write(old_dir.join("2026-10-23_1/arw/stranger.txt"), b"keep me").unwrap();
    // A folder with no manifest at all.
    fs::create_dir_all(f.root.join(TRASH_DIR).join("by-hand")).unwrap();
    fs::write(f.root.join(TRASH_DIR).join("by-hand/x.ARW"), b"x").unwrap();
    let library = f.library();

    let purged = purge_expired(&mut f.cat, f.location, now).unwrap();
    assert_eq!((purged.ops, purged.files), (1, 5));
    assert_eq!(purged.bytes, old.bytes);
    let trash = f.trash();
    assert_eq!(
        trash
            .keys()
            .filter(|k| k.starts_with(&old.op_id))
            .collect::<Vec<_>>(),
        [&format!("{}/2026-10-23_1/arw/stranger.txt", old.op_id)]
    );
    assert!(trash.contains_key("by-hand/x.ARW"));
    assert_eq!(
        trash.keys().filter(|k| k.starts_with(&new.op_id)).count(),
        3 // two files + manifest
    );
    assert_eq!(f.library(), library);

    // Empty now: everything listed, nothing else.
    let purged = purge(&mut f.cat, f.location, None, None).unwrap();
    assert_eq!((purged.ops, purged.files), (1, 2));
    assert!(!f.root.join(TRASH_DIR).join(&new.op_id).exists());
    assert!(f.trash().contains_key("by-hand/x.ARW"));
    assert_eq!(f.library(), library);
    assert_eq!(f.op_status("purge"), ["committed", "committed"]);
}

#[test]
fn manifest_paths_that_leave_the_folder_are_refused() {
    let mut f = Fixture::new(SHOOT);
    let done = f.delete(&[f.photo("dsc0002")], Scope::Photo).unwrap();
    let dir = f.root.join(TRASH_DIR).join(&done.op_id);
    let text = fs::read_to_string(dir.join(manifest::FILE_NAME)).unwrap();
    let evil = text.replace(
        "2026-10-23_1/arw/DSC0002.ARW",
        "../../2026-10-23_1/hif/DSC0001.HIF",
    );
    fs::write(dir.join(manifest::FILE_NAME), evil).unwrap();
    let library = f.library();
    // The tampered manifest isn't read at all: nothing listed, nothing purged.
    assert!(list(&mut f.cat, f.location).unwrap().is_empty());
    assert_eq!(purge(&mut f.cat, f.location, None, None).unwrap().files, 0);
    assert!(restore(&mut f.cat, f.location, &done.op_id, None).is_err());
    assert!(restore(&mut f.cat, f.location, "../x", None).is_err());
    assert_eq!(f.library(), library);
}

#[test]
fn the_scanner_ignores_recently_deleted() {
    let mut f = Fixture::new(SHOOT);
    f.delete(&[f.photo("dsc0001")], Scope::Photo).unwrap();
    f.rescan();
    assert_eq!(f.photos_in_catalog(), 1);
    assert_eq!(f.files_in_catalog(), 2);
}

#[test]
fn the_same_photo_twice_is_one_delete() {
    let mut f = Fixture::new(SHOOT);
    let p = f.photo("dsc0002");
    assert_eq!(f.delete(&[p, p], Scope::Photo).unwrap().files, 2);
}

#[test]
fn operations_wait_for_each_other() {
    // A second thread's operation must not "recover" one that is still running.
    let f = Fixture::new(SHOOT);
    let guard = exclusive();
    let path = f.root.clone();
    let t = std::thread::spawn(move || {
        let _g = exclusive();
        path.join(TRASH_DIR).exists()
    });
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(!t.is_finished());
    drop(guard);
    assert!(!t.join().unwrap());
}

/// Real files: delete, drop RAW, restore and purge on a temp copy of a real shoot.
/// `PV_FIXTURES=~/Desktop/2026-10-23_10261023 cargo test --lib pv::lifecycle::tests::real_shoot`
#[test]
fn real_shoot_round_trip() {
    let Ok(source) = std::env::var("PV_FIXTURES") else {
        return;
    };
    let source = PathBuf::from(source);
    assert!(
        !source.starts_with("/Volumes"),
        "use a local copy, never the NAS"
    );
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap().join("DCIM");
    let shoot = root.join(source.file_name().unwrap());
    for (rel, _) in snapshot(&source, &|_| true) {
        let to = shoot.join(&rel);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(source.join(&rel), to).unwrap();
    }
    let cat = Catalog::open_in_memory().unwrap();
    let location = locations::add(&cat, &root, Some("Copy")).unwrap();
    let mut f = Fixture {
        _tmp: tmp,
        root,
        cat,
        location,
    };
    f.rescan();
    let shoots: Vec<i64> = {
        let mut s = f
            .cat
            .conn()
            .prepare("SELECT DISTINCT shoot_folder_id FROM photos")
            .unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    crate::pv::scan::l1::run(&mut f.cat, &shoots).unwrap();
    let before = f.library();
    let pairs: Vec<i64> = {
        let mut s = f
            .cat
            .conn()
            .prepare("SELECT id FROM photos WHERE state = 'raw_display' ORDER BY stem_key")
            .unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    println!("{} files, {} pairs", before.len(), pairs.len());
    assert!(pairs.len() >= 2);

    let t = std::time::Instant::now();
    let a = f.delete(&pairs[..1], Scope::Photo).unwrap();
    let b = f.delete(&pairs[1..], Scope::KeepDisplay).unwrap();
    println!(
        "delete 1 Photo: {} files; drop {} RAWs: {} files, {:.0} MB; {:.2} s",
        a.files,
        pairs.len() - 1,
        b.files,
        b.bytes as f64 / 1e6,
        t.elapsed().as_secs_f64()
    );
    let raws_left = f
        .library()
        .keys()
        .filter(|k| k.to_lowercase().ends_with(".arw"))
        .count();
    assert_eq!(raws_left, 0);

    for op in [&a.op_id, &b.op_id] {
        let r = restore(&mut f.cat, f.location, op, None).unwrap();
        assert!(r.conflicts.is_empty(), "{:?}", r.conflicts);
    }
    // KeepDisplay copied the Photo-level XMPs next to the HIFs (if any); everything else is
    // back byte for byte.
    let mut after = f.library();
    after.retain(|k, _| before.contains_key(k));
    assert_eq!(after, before);
    let raw_display: i64 = f
        .cat
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM photos WHERE state = 'raw_display'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(raw_display as usize, pairs.len());

    // A Photo deleted whole comes back as a new catalog row (its ratings live in the XMP).
    let any: i64 = f
        .cat
        .conn()
        .query_row("SELECT MIN(id) FROM photos", [], |r| r.get(0))
        .unwrap();
    f.delete(&[any], Scope::Photo).unwrap();
    let purged = purge(&mut f.cat, f.location, None, None).unwrap();
    println!(
        "empty now: {} files, {:.0} MB",
        purged.files,
        purged.bytes as f64 / 1e6
    );
    assert!(!f.root.join(TRASH_DIR).read_dir().unwrap().any(|_| true));
}

// ---- M2-02: RapidRAW's delete commands ----

impl Fixture {
    fn abs(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn delete_paths(&mut self, rels: &[&str], whole: bool) -> (Plan, Vec<PathBuf>) {
        let paths: Vec<PathBuf> = rels.iter().map(|r| self.abs(r)).collect();
        let (plan, outside) = plan::plan_paths_with(&self.cat, &paths, whole).unwrap();
        for (_, r) in apply(&mut self.cat, &plan) {
            r.unwrap();
        }
        (plan, outside)
    }
}

#[test]
fn deleting_the_raw_file_is_keep_display() {
    let mut f = Fixture::new(SHOOT);
    let (plan, _) = f.delete_paths(&["2026-10-23_1/arw/DSC0001.ARW"], false);
    assert_eq!(plan.ops[0].kind, "drop_raw");
    let lib = f.library();
    assert!(lib.contains_key("2026-10-23_1/hif/DSC0001.HIF"));
    assert!(lib.contains_key("2026-10-23_1/hif/DSC0001.xmp"));
    assert!(!lib.contains_key("2026-10-23_1/arw/DSC0001.ARW"));
}

#[test]
fn deleting_both_files_or_with_associated_is_the_whole_photo() {
    let mut f = Fixture::new(SHOOT);
    f.delete_paths(
        &[
            "2026-10-23_1/arw/DSC0001.ARW",
            "2026-10-23_1/hif/DSC0001.HIF",
        ],
        false,
    );
    f.delete_paths(&["2026-10-23_1/arw/DSC0002.ARW"], true);
    assert!(f.library().is_empty(), "{:?}", f.library().keys());
}

#[test]
fn deleting_the_display_file_is_keep_raw() {
    let mut f = Fixture::new(SHOOT);
    let (plan, _) = f.delete_paths(&["2026-10-23_1/hif/DSC0001.HIF"], false);
    assert_eq!(plan.ops[0].kind, "delete");
    let lib = f.library();
    assert!(lib.contains_key("2026-10-23_1/arw/DSC0001.ARW"));
    assert!(lib.contains_key("2026-10-23_1/arw/DSC0001.xmp"));
    assert!(!lib.contains_key("2026-10-23_1/hif/DSC0001.HIF.rrdata"));
}

#[test]
fn deleting_a_locked_raw_is_refused() {
    let f = Fixture::new(SHOOT);
    f.cat
        .conn()
        .execute(
            "INSERT INTO metadata (photo_id, keep_raw, updated_at) VALUES (?1, 'keep', 0)",
            [f.photo("dsc0001")],
        )
        .unwrap();
    let paths = [f.abs("2026-10-23_1/arw/DSC0001.ARW")];
    let (plan, _) = plan::plan_paths(&f.cat, &paths).unwrap();
    assert!(plan.ops.is_empty());
    assert_eq!(plan.skipped[0].reason, SkipReason::Locked);
}

#[test]
fn a_virtual_copy_goes_alone() {
    let mut files = SHOOT.to_vec();
    files.push("2026-10-23_1/arw/DSC0001.ARW.0a1b2c.rrdata");
    let mut f = Fixture::new(&files);
    let before = f.library();
    f.delete_paths(&["2026-10-23_1/arw/DSC0001.ARW.0a1b2c.rrdata"], false);
    let mut expected = before.clone();
    expected.remove("2026-10-23_1/arw/DSC0001.ARW.0a1b2c.rrdata");
    assert_eq!(f.library(), expected);
}

#[test]
fn paths_outside_locations_are_handed_back() {
    let f = Fixture::new(SHOOT);
    let elsewhere = f.root.parent().unwrap().join("elsewhere/x.jpg");
    let paths = [elsewhere.clone(), f.abs("2026-10-23_1/arw/DSC0002.ARW")];
    let (plan, outside) = plan::plan_paths(&f.cat, &paths).unwrap();
    assert_eq!(outside, [elsewhere]);
    assert_eq!(plan.photos(), 1);
    assert_eq!(plan::outside_locations(&f.cat, &paths).unwrap().len(), 1);
}

#[test]
fn unindexed_file_in_a_location_still_goes_to_recently_deleted() {
    let mut f = Fixture::new(SHOOT);
    fs::write(f.abs("2026-10-23_1/notes.txt"), b"n").unwrap();
    f.delete_paths(&["2026-10-23_1/notes.txt"], false);
    assert!(
        f.trash()
            .keys()
            .any(|k| k.ends_with("2026-10-23_1/notes.txt"))
    );
}

#[test]
fn deleting_a_folder_moves_everything_under_it_and_restores() {
    let mut f = Fixture::new(SHOOT);
    fs::write(f.abs("2026-10-23_1/notes.txt"), b"n").unwrap();
    let before = f.library();
    let plan = plan::plan_folder(&f.cat, &f.abs("2026-10-23_1"))
        .unwrap()
        .unwrap();
    assert_eq!(plan.ops.len(), 1);
    assert_eq!(plan.ops[0].files.len(), 8);
    assert_eq!(plan.photos(), 2);
    let done = apply(&mut f.cat, &plan).pop().unwrap().1.unwrap();
    assert!(f.library().is_empty());
    assert_eq!(f.photos_in_catalog(), 0);

    restore(&mut f.cat, f.location, &done.op_id, None).unwrap();
    assert_eq!(f.library(), before);
}

#[test]
fn folder_delete_refuses_the_location_root_and_recently_deleted() {
    let f = Fixture::new(SHOOT);
    assert!(plan::plan_folder(&f.cat, &f.root).is_err());
    fs::create_dir_all(f.root.join(TRASH_DIR).join("x")).unwrap();
    assert!(plan::plan_folder(&f.cat, &f.root.join(TRASH_DIR).join("x")).is_err());
    let outside = f.root.parent().unwrap().to_path_buf();
    assert!(plan::plan_folder(&f.cat, &outside).unwrap().is_none());
}

// ---- M2-03: the delete dialog ----

#[test]
fn dialog_choice_applies_to_pairs_and_others_go_whole() {
    let mut f = Fixture::new(&[
        "s/DSC0001.ARW",
        "s/DSC0001.HIF",
        "s/DSC0002.HIF",
        "s/DSC0003.ARW",
        "s/DSC0004.ARW",
        "s/DSC0004.HIF",
    ]);
    f.cat
        .conn()
        .execute(
            "INSERT INTO metadata (photo_id, keep_raw, updated_at) VALUES (?1, 'keep', 0)",
            [f.photo("dsc0004")],
        )
        .unwrap();
    let ids: Vec<i64> = ["dsc0001", "dsc0002", "dsc0003", "dsc0004"]
        .iter()
        .map(|s| f.photo(s))
        .collect();
    let plan = plan_delete(&f.cat, &ids, Scope::KeepDisplay).unwrap();
    assert_eq!(plan.skipped.len(), 1);
    assert_eq!(plan.skipped[0].reason, SkipReason::Locked);
    for (_, r) in apply(&mut f.cat, &plan) {
        r.unwrap();
    }
    assert_eq!(
        f.library().keys().collect::<Vec<_>>(),
        ["s/DSC0001.HIF", "s/DSC0004.ARW", "s/DSC0004.HIF"]
    );
    let reasons: Vec<Reason> = list(&mut f.cat, f.location)
        .unwrap()
        .iter()
        .map(|o| o.manifest.reason)
        .collect();
    assert_eq!(reasons.len(), 2);
    assert!(reasons.contains(&Reason::Slimmed) && reasons.contains(&Reason::Deleted));
}

#[test]
fn grid_paths_map_to_photos_only_when_all_are_photos() {
    let f = Fixture::new(SHOOT);
    let raw = f.abs("2026-10-23_1/arw/DSC0001.ARW");
    let hif = f.abs("2026-10-23_1/hif/DSC0002.HIF");
    let ids = plan::photo_ids_for_paths(&f.cat, &[raw.clone(), hif])
        .unwrap()
        .unwrap();
    assert_eq!(ids, [f.photo("dsc0001"), f.photo("dsc0002")]);
    let other = f.root.parent().unwrap().join("x.jpg");
    assert!(
        plan::photo_ids_for_paths(&f.cat, &[raw, other])
            .unwrap()
            .is_none()
    );
}

// ---- M2-04: Recently Deleted view ----

#[test]
fn preview_prefers_the_display_copy() {
    let mut f = Fixture::new(SHOOT);
    let done = f.delete(&[f.photo("dsc0001")], Scope::Photo).unwrap();
    let path = preview_file(&f.root, &done.op_id, 0).unwrap();
    assert!(
        path.ends_with("2026-10-23_1/hif/DSC0001.HIF"),
        "{}",
        path.display()
    );
    assert!(path.starts_with(f.root.join(TRASH_DIR)));
    assert!(preview_file(&f.root, &done.op_id, 5).is_err());
    assert!(preview_file(&f.root, "../../etc", 0).is_err());
}

#[test]
fn restoring_one_photo_of_an_operation_keeps_the_rest() {
    let mut f = Fixture::new(SHOOT);
    let photos = [f.photo("dsc0001"), f.photo("dsc0002")];
    let done = f.delete(&photos, Scope::Photo).unwrap();
    let op = list(&mut f.cat, f.location).unwrap().pop().unwrap();
    let second: Vec<String> = op
        .manifest
        .files
        .iter()
        .filter(|x| x.photo == Some(1))
        .map(|x| x.path.clone())
        .collect();
    restore(&mut f.cat, f.location, &done.op_id, Some(&second)).unwrap();
    assert_eq!(f.library().len(), 2);
    let left = list(&mut f.cat, f.location).unwrap().pop().unwrap();
    assert_eq!(left.manifest.files.len(), 5);
    // Indexes still point at the right Photos, so previews keep working.
    assert_eq!(left.manifest.photos.len(), 2);
    assert!(preview_file(&f.root, &done.op_id, 0).is_ok());
    assert!(preview_file(&f.root, &done.op_id, 1).is_err());
}

// ---- M2-05: orphans ----

fn state_of(f: &Fixture, photo: i64) -> String {
    f.cat
        .conn()
        .query_row("SELECT state FROM photos WHERE id = ?1", [photo], |r| {
            r.get(0)
        })
        .unwrap()
}

#[test]
fn keep_raw_only_is_not_an_orphan_but_a_lost_hif_is() {
    let mut f = Fixture::new(SHOOT);
    let kept = f.photo("dsc0001");
    f.delete(&[kept], Scope::KeepRaw).unwrap();
    assert_eq!(state_of(&f, kept), "raw_only");

    // A HIF deleted in Finder.
    let lost = f.photo("dsc0002");
    fs::remove_file(f.abs("2026-10-23_1/hif/DSC0002.HIF")).unwrap();
    f.rescan();
    assert_eq!(state_of(&f, lost), "orphan_raw");
    assert_eq!(state_of(&f, kept), "raw_only");

    // Keep as RAW-only.
    assert_eq!(
        crate::pv::assets::mark_raw_only(&f.cat, &[lost, kept]).unwrap(),
        1
    );
    assert_eq!(state_of(&f, lost), "raw_only");
    f.rescan();
    assert_eq!(state_of(&f, lost), "raw_only");

    // Restoring the HIF makes it a pair again.
    let op = list(&mut f.cat, f.location).unwrap().pop().unwrap();
    restore(&mut f.cat, f.location, &op.manifest.op_id, None).unwrap();
    assert_eq!(state_of(&f, kept), "raw_display");
}

#[test]
fn deleting_an_orphan_moves_its_raw_and_sidecars() {
    let mut f = Fixture::new(SHOOT);
    let p = f.photo("dsc0001");
    fs::remove_file(f.abs("2026-10-23_1/hif/DSC0001.HIF")).unwrap();
    fs::remove_file(f.abs("2026-10-23_1/hif/DSC0001.HIF.rrdata")).unwrap();
    f.rescan();
    assert_eq!(state_of(&f, p), "orphan_raw");
    let done = f.delete(&[p], Scope::Photo).unwrap();
    assert_eq!(done.files, 3);
    assert_eq!(
        f.library().keys().collect::<Vec<_>>(),
        [
            "2026-10-23_1/arw/DSC0002.ARW",
            "2026-10-23_1/hif/DSC0002.HIF"
        ]
    );
}

// ---- M2-06: undo ----

#[test]
fn undo_restores_whole_actions_newest_first() {
    let mut f = Fixture::new(&[
        "a/DSC0001.ARW",
        "a/DSC0001.HIF",
        "a/DSC0002.ARW",
        "a/DSC0002.HIF",
        "a/DSC0003.HIF",
    ]);
    let before = f.library();
    // One action that makes two operations (Keep HIF only for the pair, the HIF-only Photo whole).
    let first = plan_delete(
        &f.cat,
        &[f.photo("dsc0001"), f.photo("dsc0003")],
        Scope::KeepDisplay,
    )
    .unwrap();
    assert_eq!(first.ops.len(), 2);
    for (_, r) in apply(&mut f.cat, &first) {
        r.unwrap();
    }
    let after_first = f.library();
    f.delete(&[f.photo("dsc0002")], Scope::Photo).unwrap();

    let u = undo_last(&mut f.cat).unwrap().unwrap();
    assert_eq!((u.ops, u.files), (1, 2));
    assert_eq!(f.library(), after_first);

    let u = undo_last(&mut f.cat).unwrap().unwrap();
    assert_eq!((u.ops, u.files), (2, 2));
    assert_eq!(f.library(), before);

    assert!(undo_last(&mut f.cat).unwrap().is_none());
}

#[test]
fn undo_skips_actions_already_restored_or_purged() {
    let mut f = Fixture::new(SHOOT);
    let a = f.delete(&[f.photo("dsc0001")], Scope::Photo).unwrap();
    let b = f.delete(&[f.photo("dsc0002")], Scope::Photo).unwrap();
    // The newest was restored by hand in Recently Deleted.
    restore(&mut f.cat, f.location, &b.op_id, None).unwrap();
    let u = undo_last(&mut f.cat).unwrap().unwrap();
    assert_eq!(u.files, a.files);
    // Purged actions can't come back.
    f.delete(&[f.photo("dsc0002")], Scope::Photo).unwrap();
    purge(&mut f.cat, f.location, None, None).unwrap();
    assert!(undo_last(&mut f.cat).unwrap().is_none());
}

#[test]
fn undo_on_an_offline_location_does_nothing() {
    let mut f = Fixture::new(SHOOT);
    f.delete(&[f.photo("dsc0001")], Scope::Photo).unwrap();
    f.go_offline();
    assert!(undo_last(&mut f.cat).unwrap().is_none());
}
