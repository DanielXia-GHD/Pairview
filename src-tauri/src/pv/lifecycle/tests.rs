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
    for (rel, _) in snapshot(&source, &|rel| !rel.starts_with(TRASH_DIR)) {
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

// ---- Keep HIF only, then restore: one XMP again ----

#[test]
fn restoring_a_dropped_raw_folds_the_xmp_copy_back() {
    let mut f = Fixture::new(SHOOT);
    let before = f.library();
    let done = f.delete(&[f.photo("dsc0001")], Scope::KeepDisplay).unwrap();
    assert!(f.library().contains_key("2026-10-23_1/hif/DSC0001.xmp"));
    restore(&mut f.cat, f.location, &done.op_id, None).unwrap();
    // Exactly as before: byte for byte, with no second XMP.
    assert_eq!(f.library(), before);
    assert_eq!(f.files_in_catalog(), 7);
}

#[test]
fn a_rating_changed_while_the_raw_was_gone_survives_the_restore() {
    let mut f = Fixture::new(SHOOT);
    let done = f.delete(&[f.photo("dsc0001")], Scope::KeepDisplay).unwrap();
    fs::write(f.abs("2026-10-23_1/hif/DSC0001.xmp"), b"newer rating").unwrap();
    restore(&mut f.cat, f.location, &done.op_id, None).unwrap();
    let lib = f.library();
    assert_eq!(lib["2026-10-23_1/arw/DSC0001.xmp"], b"newer rating");
    assert!(!lib.contains_key("2026-10-23_1/hif/DSC0001.xmp"));
}

#[test]
fn undo_of_keep_display_is_byte_identical() {
    let mut f = Fixture::new(SHOOT);
    let before = f.library();
    f.delete(
        &[f.photo("dsc0001"), f.photo("dsc0002")],
        Scope::KeepDisplay,
    )
    .unwrap();
    undo_last(&mut f.cat).unwrap().unwrap();
    assert_eq!(f.library(), before);
}

/// M2-11, local half, on a temp copy of a real shoot:
/// `PV_FIXTURES=~/Desktop/2026-10-23_10261023 cargo test --lib pv::lifecycle::tests::exit_check -- --nocapture`
#[test]
fn exit_check_rate_slim_undo_on_a_real_shoot() {
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
    // Not the fixture's own Recently Deleted: only what's live in it.
    for (rel, _) in snapshot(&source, &|rel| !rel.starts_with(TRASH_DIR)) {
        let to = root.join(source.file_name().unwrap()).join(&rel);
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
    let ids: Vec<i64> = {
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
    assert!(ids.len() >= 2);

    // The owner rates one Photo; that writes its XMP in arw/.
    let rated = ids[0];
    let change = crate::pv::metadata::Change {
        rating: Some(4),
        ..Default::default()
    };
    for (_, r) in crate::pv::metadata::set(&mut f.cat, &[rated], &change).unwrap() {
        r.unwrap();
    }
    let before = f.library();

    // Review, then Apply on every Photo the verdict calls HIF is enough, plus the rated one.
    let items = crate::pv::slim::review(&mut f.cat, &ids).unwrap();
    for i in &items {
        println!(
            "{} {:?} {:?} {} MB",
            i.value.photo_id,
            i.value.verdict,
            i.value.chips.iter().map(|c| c.kind).collect::<Vec<_>>(),
            i.raw_bytes / 1_000_000
        );
    }
    plan::sync_sidecars(&f.cat, &ids).unwrap();
    let plan = plan_photos(&f.cat, &ids, Scope::KeepDisplay).unwrap();
    let t = std::time::Instant::now();
    for (_, r) in apply(&mut f.cat, &plan) {
        r.unwrap();
    }
    println!(
        "dropped {} RAWs, {} MB, in {:.3} s",
        plan.photos(),
        plan.bytes() / 1_000_000,
        t.elapsed().as_secs_f64()
    );
    let lib = f.library();
    assert!(!lib.keys().any(|k| k.to_lowercase().ends_with(".arw")));
    for (rel, bytes) in &before {
        if rel.to_lowercase().ends_with(".hif") {
            assert_eq!(lib.get(rel), Some(bytes), "HIF changed: {rel}");
        }
    }

    // Rated, then slimmed straight away: its sidecars went too, and an XMP sits by the HIF.
    let stem = f
        .cat
        .conn()
        .query_row("SELECT stem_key FROM photos WHERE id = ?1", [rated], |r| {
            r.get::<_, String>(0)
        })
        .unwrap();
    let in_arw: Vec<_> = lib.keys().filter(|k| k.contains("/arw/")).collect();
    assert!(in_arw.is_empty(), "left behind: {in_arw:?}");
    assert!(
        lib.keys()
            .any(|k| k.contains("/hif/") && k.to_lowercase().ends_with(&format!("{stem}.xmp"))),
        "no XMP next to the HIF: {:?}",
        lib.keys().collect::<Vec<_>>()
    );

    // The rating survives on the HIF, even for a catalog rebuilt from disk.
    assert_eq!(crate::pv::metadata::get(&f.cat, rated).unwrap().rating, 4);
    let cat2 = Catalog::open_in_memory().unwrap();
    let location2 = locations::add(&cat2, &f.root, Some("Copy")).unwrap();
    let mut g = Fixture {
        _tmp: tempfile::tempdir().unwrap(),
        root: f.root.clone(),
        cat: cat2,
        location: location2,
    };
    g.rescan();
    let shoots2: Vec<i64> = {
        let mut s = g
            .cat
            .conn()
            .prepare("SELECT DISTINCT shoot_folder_id FROM photos")
            .unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    crate::pv::metadata::sync_shoots(&mut g.cat, &shoots2).unwrap();
    let rebuilt: i64 = g
        .cat
        .conn()
        .query_row("SELECT MAX(rating) FROM metadata", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rebuilt, 4, "rating lost after a catalog rebuild");

    // Undo puts every byte back.
    let u = undo_last(&mut f.cat).unwrap().unwrap();
    println!("undo: {} files", u.files);
    assert_eq!(f.library(), before);
    assert_eq!(crate::pv::metadata::get(&f.cat, rated).unwrap().rating, 4);
}

#[test]
fn sidecars_written_after_the_last_scan_go_with_the_delete() {
    let mut f = Fixture::new(&[
        "s/arw/DSC0001.ARW",
        "s/hif/DSC0001.HIF",
        "s/arw/DSC0002.ARW",
    ]);
    // A rating saved a moment ago: on disk, not in the catalog yet.
    fs::write(f.abs("s/arw/DSC0001.xmp"), b"rating").unwrap();
    fs::write(f.abs("s/arw/DSC0001.ARW.rrdata"), b"{}").unwrap();
    fs::write(f.abs("s/hif/DSC0001.HIF.rrdata"), b"{}").unwrap();
    let p = f.photo("dsc0001");
    assert_eq!(plan::sync_sidecars(&f.cat, &[p]).unwrap(), 3);
    assert_eq!(f.photo("dsc0001"), p, "same Photo");
    assert_eq!(plan::sync_sidecars(&f.cat, &[p]).unwrap(), 0);
    let done = f.delete(&[p], Scope::Photo).unwrap();
    assert_eq!(done.files, 5);
    assert_eq!(
        f.library().keys().collect::<Vec<_>>(),
        ["s/arw/DSC0002.ARW"]
    );
}

#[test]
fn raw_alone_through_upstream_takes_sidecars_written_since_the_scan() {
    let mut f = Fixture::new(&["s/arw/DSC0001.ARW", "s/hif/DSC0001.HIF"]);
    fs::write(f.abs("s/arw/DSC0001.xmp"), b"rating").unwrap();
    f.delete_paths(&["s/arw/DSC0001.ARW"], false);
    let lib = f.library();
    assert_eq!(lib["s/hif/DSC0001.xmp"], b"rating");
    assert!(!lib.keys().any(|k| k.contains("/arw/")));
}

// ---- M3-02: overwrite ----

#[test]
fn replace_display_moves_the_original_out_and_the_render_in() {
    let mut f = Fixture::new(SHOOT);
    let before = f.library();
    let p = f.photo("dsc0002");
    let r = replace_display(&mut f.cat, p, b"the render").unwrap();
    let lib = f.library();
    assert_eq!(lib["2026-10-23_1/hif/DSC0002.HIF"], b"the render");
    assert_eq!(
        lib["2026-10-23_1/arw/DSC0002.ARW"],
        before["2026-10-23_1/arw/DSC0002.ARW"]
    );
    assert_eq!(
        lib.len(),
        before.len(),
        "no temp file left: {:?}",
        lib.keys()
    );
    let trash = f.trash();
    assert_eq!(
        trash[&format!("{}/2026-10-23_1/hif/DSC0002.HIF", r.op_id)],
        before["2026-10-23_1/hif/DSC0002.HIF"],
        "camera original, byte for byte"
    );
    let m = Manifest::read(&f.root.join(TRASH_DIR).join(&r.op_id)).unwrap();
    assert_eq!((m.reason, m.kind.as_str()), (Reason::Replaced, "overwrite"));
    assert_eq!(m.status, manifest::Status::Committed);
    assert_eq!(state_of(&f, p), "edited");
    assert_eq!(f.op_status("overwrite"), ["committed"]);
    // Still one pair in the catalog, with the new size.
    let size: i64 = f
        .cat
        .conn()
        .query_row(
            "SELECT size FROM files WHERE name = 'DSC0002.HIF'",
            [],
            |x| x.get(0),
        )
        .unwrap();
    assert_eq!(size, 10);
    assert_eq!(f.files_in_catalog(), 7);
    // A later scan changes nothing about it.
    f.rescan();
    assert_eq!(state_of(&f, p), "edited");
    // Undo is for deletes; it must not touch an overwrite.
    assert!(undo_last(&mut f.cat).unwrap().is_none());
    assert_eq!(f.library()["2026-10-23_1/hif/DSC0002.HIF"], b"the render");
}

#[test]
fn a_second_render_keeps_both_older_versions_in_recently_deleted() {
    let mut f = Fixture::new(SHOOT);
    let before = f.library();
    let p = f.photo("dsc0002");
    let a = replace_display(&mut f.cat, p, b"render one").unwrap();
    let b = replace_display(&mut f.cat, p, b"render two").unwrap();
    assert_eq!(f.library()["2026-10-23_1/hif/DSC0002.HIF"], b"render two");
    let t = f.trash();
    assert_eq!(
        t[&format!("{}/2026-10-23_1/hif/DSC0002.HIF", a.op_id)],
        before["2026-10-23_1/hif/DSC0002.HIF"]
    );
    assert_eq!(
        t[&format!("{}/2026-10-23_1/hif/DSC0002.HIF", b.op_id)],
        b"render one"
    );
}

/// Every crash point of the two renames, and a failed rename, end with the original back.
#[test]
fn overwrite_rolls_back_from_every_crash_point() {
    for (interrupt, recovered) in [
        (Interrupt::Crash(0), true),
        (Interrupt::Crash(1), true),
        (Interrupt::Fail(0), false),
        (Interrupt::Fail(1), false),
    ] {
        let mut f = Fixture::new(SHOOT);
        let before = f.library();
        let p = f.photo("dsc0002");
        INTERRUPT.with(|i| i.set(Some(interrupt)));
        let r = replace_display(&mut f.cat, p, b"the render");
        INTERRUPT.with(|i| i.set(None));
        assert!(r.is_err());
        if recovered {
            assert_eq!(recover(&mut f.cat).unwrap().rolled_back, 1);
        }
        assert_eq!(f.library(), before, "library after rollback");
        assert!(f.trash().is_empty(), "{:?}", f.trash().keys());
        // Temp files are dot-files: look for them directly.
        let hif = fs::read_dir(f.abs("2026-10-23_1/hif")).unwrap().count();
        assert_eq!(hif, 3, "temp file left behind");
        assert_eq!(f.op_status("overwrite"), ["rolled_back"]);
        assert_eq!(state_of(&f, p), "raw_display");
    }
}

#[test]
fn overwrite_refused_offline_and_without_a_display_copy() {
    let mut f = Fixture::new(&["s/DSC0001.ARW", "s/DSC0002.ARW", "s/DSC0002.HIF"]);
    let raw_only = f.photo("dsc0001");
    assert!(replace_display(&mut f.cat, raw_only, b"x").is_err());
    let p = f.photo("dsc0002");
    let everything = snapshot(&f.root, &|_| true);
    f.go_offline();
    let err = replace_display(&mut f.cat, p, b"x").unwrap_err();
    assert!(err.to_string().contains("offline"), "{err}");
    assert_eq!(snapshot(&f.root, &|_| true), everything);
}

#[test]
fn the_edited_marker_survives_a_catalog_rebuild() {
    let mut f = Fixture::new(SHOOT);
    let p = f.photo("dsc0002");
    replace_display(&mut f.cat, p, b"render").unwrap();
    crate::pv::metadata::mark_edited(&f.cat, p, Some(1234)).unwrap();
    let cat = Catalog::open_in_memory().unwrap();
    let location = locations::add(&cat, &f.root, None).unwrap();
    let mut g = Fixture {
        _tmp: tempfile::tempdir().unwrap(),
        root: f.root.clone(),
        cat,
        location,
    };
    g.rescan();
    let shoots: Vec<i64> = {
        let mut s = g
            .cat
            .conn()
            .prepare("SELECT DISTINCT shoot_folder_id FROM photos")
            .unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    crate::pv::metadata::sync_shoots(&mut g.cat, &shoots).unwrap();
    assert_eq!(state_of(&g, g.photo("dsc0002")), "edited");
    assert_eq!(state_of(&g, g.photo("dsc0001")), "raw_display");
    // Clearing it: the next rebuild sees a camera original again.
    crate::pv::metadata::mark_edited(&f.cat, p, None).unwrap();
    let text = fs::read_to_string(f.abs("2026-10-23_1/arw/DSC0002.xmp")).unwrap();
    assert!(crate::pv::xmp::read(&text).edited.is_none());
}

// ---- M3-03: Revert ----

const PAIR: &[&str] = &[
    "s/arw/DSC0001.ARW",
    "s/arw/DSC0001.ARW.rrdata",
    "s/hif/DSC0001.HIF",
    "s/arw/DSC0002.ARW",
    "s/hif/DSC0002.HIF",
];

#[test]
fn revert_brings_the_camera_original_back_and_sets_the_recipe_aside() {
    let mut f = Fixture::new(PAIR);
    let before = f.library();
    let p = f.photo("dsc0001");
    let done = replace_display(&mut f.cat, p, b"render").unwrap();
    assert!(!done.kept_as_base);
    assert!(can_revert(&mut f.cat, p).unwrap());

    let r = revert_display(&mut f.cat, p).unwrap();
    let lib = f.library();
    assert_eq!(
        lib["s/hif/DSC0001.HIF"], before["s/hif/DSC0001.HIF"],
        "camera bytes"
    );
    assert_eq!(lib["s/arw/DSC0001.ARW"], before["s/arw/DSC0001.ARW"]);
    assert!(
        !lib.contains_key("s/arw/DSC0001.ARW.rrdata"),
        "recipe set aside"
    );
    assert_eq!(state_of(&f, p), "raw_display");

    // Render and recipe are in Recently Deleted; the operation that held the original is gone.
    let t = f.trash();
    assert_eq!(t[&format!("{}/s/hif/DSC0001.HIF", r.op_id)], b"render");
    assert_eq!(
        t[&format!("{}/s/arw/DSC0001.ARW.rrdata", r.op_id)],
        before["s/arw/DSC0001.ARW.rrdata"]
    );
    assert!(!f.root.join(TRASH_DIR).join(&done.op_id).exists());
    assert!(!can_revert(&mut f.cat, p).unwrap());
    assert!(
        revert_display(&mut f.cat, p).is_err(),
        "nothing left to revert"
    );
    // Catalog and disk agree after a scan.
    f.rescan();
    assert_eq!(state_of(&f, p), "raw_display");
    assert_eq!(f.files_in_catalog(), 4);
}

#[test]
fn revert_finds_the_camera_file_after_several_renders_and_cycles() {
    let mut f = Fixture::new(PAIR);
    let camera = f.library()["s/hif/DSC0001.HIF"].clone();
    let p = f.photo("dsc0001");
    replace_display(&mut f.cat, p, b"one").unwrap();
    replace_display(&mut f.cat, p, b"two").unwrap();
    revert_display(&mut f.cat, p).unwrap();
    assert_eq!(f.library()["s/hif/DSC0001.HIF"], camera);
    // Edit again: older renders still sit in Recently Deleted, but only the camera file counts.
    replace_display(&mut f.cat, p, b"three").unwrap();
    revert_display(&mut f.cat, p).unwrap();
    assert_eq!(f.library()["s/hif/DSC0001.HIF"], camera);
}

#[test]
fn revert_rolls_back_from_every_crash_point() {
    for interrupt in [
        Interrupt::Crash(0),
        Interrupt::Crash(1),
        Interrupt::Crash(2),
        Interrupt::Fail(1),
        Interrupt::Fail(2),
    ] {
        let mut f = Fixture::new(PAIR);
        let p = f.photo("dsc0001");
        replace_display(&mut f.cat, p, b"render").unwrap();
        let library = f.library();
        let trash = f.trash();
        INTERRUPT.with(|i| i.set(Some(interrupt)));
        let r = revert_display(&mut f.cat, p);
        INTERRUPT.with(|i| i.set(None));
        assert!(r.is_err());
        recover(&mut f.cat).unwrap();
        assert_eq!(f.library(), library, "library");
        assert_eq!(f.trash(), trash, "Recently Deleted");
        assert_eq!(state_of(&f, p), "edited");
        // And it still works afterwards.
        revert_display(&mut f.cat, p).unwrap();
    }
}

#[test]
fn revert_is_refused_once_the_original_was_purged() {
    let mut f = Fixture::new(PAIR);
    let p = f.photo("dsc0001");
    replace_display(&mut f.cat, p, b"render").unwrap();
    purge(&mut f.cat, f.location, None, None).unwrap();
    assert!(!can_revert(&mut f.cat, p).unwrap());
    let err = revert_display(&mut f.cat, p).unwrap_err();
    assert!(err.to_string().contains("no longer available"), "{err}");
    assert_eq!(f.library()["s/hif/DSC0001.HIF"], b"render");
}

#[test]
fn revert_refused_offline_and_without_an_edit() {
    let mut f = Fixture::new(PAIR);
    let plain = f.photo("dsc0002");
    assert!(!can_revert(&mut f.cat, plain).unwrap());
    assert!(revert_display(&mut f.cat, plain).is_err());
    let p = f.photo("dsc0001");
    replace_display(&mut f.cat, p, b"render").unwrap();
    let everything = snapshot(&f.root, &|_| true);
    f.go_offline();
    assert!(!can_revert(&mut f.cat, p).unwrap());
    assert!(
        revert_display(&mut f.cat, p)
            .unwrap_err()
            .to_string()
            .contains("offline")
    );
    assert_eq!(snapshot(&f.root, &|_| true), everything);
}

// ---- M3-03: the hidden base of a Photo without a RAW ----

const NO_RAW: &[&str] = &["s/DSC0003.HIF", "s/DSC0004.HIF"];

#[test]
fn first_edit_without_a_raw_keeps_the_original_as_the_hidden_base() {
    let mut f = Fixture::new(NO_RAW);
    let before = f.library();
    let p = f.photo("dsc0003");
    let r = replace_display(&mut f.cat, p, b"render").unwrap();
    assert!(r.kept_as_base);
    assert_eq!(f.library()["s/DSC0003.HIF"], b"render");
    assert_eq!(
        f.library()["s/.originals/DSC0003.HIF"],
        before["s/DSC0003.HIF"]
    );
    assert!(
        f.trash().is_empty(),
        "the base isn't in Recently Deleted: {:?}",
        f.trash().keys()
    );

    // RapidRAW's reads get the base; the viewer's exact reads get the render.
    let display = f.abs("s/DSC0003.HIF");
    let mapped = crate::file_management::read_file_mapped(&display).unwrap();
    assert_eq!(&mapped[..], &before["s/DSC0003.HIF"][..]);
    let exact = crate::file_management::read_file_mapped_exact(&display).unwrap();
    assert_eq!(&exact[..], b"render");
    let other = crate::file_management::read_file_mapped(&f.abs("s/DSC0004.HIF")).unwrap();
    assert_eq!(&other[..], &before["s/DSC0004.HIF"][..]);

    // The scanner never sees it.
    f.rescan();
    assert_eq!(f.photos_in_catalog(), 2);
    assert_eq!(f.files_in_catalog(), 2);

    // Later renders replace the render; the base stays put.
    let second = replace_display(&mut f.cat, p, b"render two").unwrap();
    assert!(!second.kept_as_base);
    assert_eq!(
        f.library()["s/.originals/DSC0003.HIF"],
        before["s/DSC0003.HIF"]
    );
    assert_eq!(
        f.trash()[&format!("{}/s/DSC0003.HIF", second.op_id)],
        b"render"
    );

    // Revert: the original back, no hidden folder left.
    revert_display(&mut f.cat, p).unwrap();
    assert_eq!(f.library(), before);
    assert!(!f.abs("s/.originals").exists());
    assert!(!can_revert(&mut f.cat, p).unwrap());
}

#[test]
fn first_edit_without_a_raw_rolls_back_from_every_crash_point() {
    for (interrupt, crash) in [
        (Interrupt::Crash(0), true),
        (Interrupt::Crash(1), true),
        (Interrupt::Fail(0), false),
        (Interrupt::Fail(1), false),
    ] {
        let mut f = Fixture::new(NO_RAW);
        let before = f.library();
        let p = f.photo("dsc0003");
        INTERRUPT.with(|i| i.set(Some(interrupt)));
        let r = replace_display(&mut f.cat, p, b"render");
        INTERRUPT.with(|i| i.set(None));
        assert!(r.is_err());
        if crash {
            assert_eq!(recover(&mut f.cat).unwrap().rolled_back, 1);
        }
        assert_eq!(f.library(), before);
        assert!(!f.abs("s/.originals").exists(), "no empty folder left");
        assert_eq!(
            fs::read_dir(f.abs("s")).unwrap().count(),
            2,
            "no temp file left"
        );
    }
}

#[test]
fn deleting_an_edited_photo_without_a_raw_takes_its_base_and_undo_brings_it_back() {
    let mut f = Fixture::new(NO_RAW);
    let p = f.photo("dsc0003");
    replace_display(&mut f.cat, p, b"render").unwrap();
    let edited = f.library();
    let done = f.delete(&[p], Scope::Photo).unwrap();
    assert_eq!(done.files, 2);
    assert_eq!(f.library().keys().collect::<Vec<_>>(), ["s/DSC0004.HIF"]);
    assert!(!f.abs("s/.originals").exists());
    undo_last(&mut f.cat).unwrap().unwrap();
    assert_eq!(f.library(), edited);
}

#[test]
fn a_photo_with_a_raw_never_gets_a_base() {
    let mut f = Fixture::new(PAIR);
    let p = f.photo("dsc0001");
    replace_display(&mut f.cat, p, b"render").unwrap();
    assert!(!f.abs("s/hif/.originals").exists());
}
