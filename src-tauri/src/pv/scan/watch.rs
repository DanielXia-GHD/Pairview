//! Keeps a location's catalog current while Pairview runs (docs/IMPLEMENTATION_PLAN.md M1-06).
//!
//! Each location gets one background thread with its own catalog connection. It syncs (L0 scan,
//! pairing, L1 metadata) on start, then whenever FSEvents reports a change (local disks only)
//! or the poll interval passes. A NAS share is polled only: macOS doesn't see changes made on
//! the server side, such as the camera's FTP uploads.
//!
//! Files that may still be arriving are held back (`l0::Stability`) and checked again after a
//! few seconds, so an upload shows up once it's complete.

// The app setup and commands (M1-09) start the watchers and forward the events.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::Result;
use notify::{RecursiveMode, Watcher as _};
use serde::Serialize;

use crate::pv::assets;
use crate::pv::catalog::Catalog;
use crate::pv::locations::{self, Location};
use crate::pv::metadata;
use crate::pv::scan::{l0, l1};

/// Tauri event sent to the frontend when a location's folders changed.
pub const FOLDER_CHANGED_EVENT: &str = "pv://folder-changed";

pub const DEFAULT_POLL: Duration = Duration::from_secs(60);

/// How soon to look again while files are held back as possibly still arriving.
const RECHECK: Duration = Duration::from_secs(2);

/// FSEvents fire in bursts while files are copied: wait this long for a burst to end.
const DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderChanged {
    pub location_id: i64,
    /// Folders whose files or sub-folders changed.
    pub folders: Vec<i64>,
    /// Shoots containing them (their Photos were re-paired).
    pub shoots: Vec<i64>,
}

#[derive(Debug, Clone)]
pub struct WatchOptions {
    pub poll_interval: Duration,
    pub fs_events: bool,
}

impl WatchOptions {
    /// Poll interval from `settings_json` (`{"pollSeconds": 60}`); FSEvents only for local disks.
    pub fn for_location(location: &Location) -> Self {
        let poll_seconds = serde_json::from_str::<serde_json::Value>(&location.settings_json)
            .ok()
            .and_then(|v| v.get("pollSeconds")?.as_u64())
            .filter(|&s| s >= 5);
        let network = location
            .volume_id
            .as_deref()
            .is_some_and(|id| id.contains("://"));
        Self {
            poll_interval: poll_seconds.map_or(DEFAULT_POLL, Duration::from_secs),
            fs_events: !network,
        }
    }
}

enum Msg {
    FsEvent,
    Stop,
}

/// Stops its thread when dropped.
pub struct LocationWatcher {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
    _fs_events: Option<notify::RecommendedWatcher>,
}

impl Drop for LocationWatcher {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts watching a location. `on_change` runs on the watcher thread after each sync that
/// changed something.
pub fn watch(
    catalog_path: &Path,
    location: &Location,
    opts: WatchOptions,
    on_change: impl FnMut(FolderChanged) + Send + 'static,
) -> Result<LocationWatcher> {
    let (tx, rx) = mpsc::channel();

    let fs_events = if opts.fs_events {
        let events_tx = tx.clone();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
                    let _ = events_tx.send(Msg::FsEvent);
                }
            })?;
        watcher.watch(&location.path, RecursiveMode::Recursive)?;
        Some(watcher)
    } else {
        None
    };

    let catalog_path: PathBuf = catalog_path.to_path_buf();
    let location_id = location.id;
    let poll = opts.poll_interval;
    let thread = thread::Builder::new()
        .name(format!("pv-watch-{location_id}"))
        .spawn(move || {
            let mut on_change = on_change;
            let mut cat = match Catalog::open(&catalog_path) {
                Ok(cat) => cat,
                Err(e) => {
                    log::error!("Pairview watcher {location_id}: {e:#}");
                    return;
                }
            };
            let mut stability = l0::Stability::new(poll * 2);
            loop {
                let Ok(Some(location)) = locations::get(&cat, location_id) else {
                    return; // The location was removed.
                };
                let mut wait = poll;
                match sync_once(&mut cat, &location, &mut stability) {
                    Ok(outcome) => {
                        if outcome.files_held {
                            wait = RECHECK.min(poll);
                        }
                        if let Some(change) = outcome.change {
                            on_change(change);
                        }
                    }
                    Err(e) => log::warn!("Pairview watcher {location_id}: {e:#}"),
                }
                match rx.recv_timeout(wait) {
                    Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                    Ok(Msg::FsEvent) => {
                        thread::sleep(DEBOUNCE);
                        if rx.try_iter().any(|m| matches!(m, Msg::Stop)) {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
        })?;

    Ok(LocationWatcher {
        tx,
        thread: Some(thread),
        _fs_events: fs_events,
    })
}

#[derive(Debug, Default)]
pub struct SyncOutcome {
    pub change: Option<FolderChanged>,
    pub files_held: bool,
}

/// One sync of a location: L0 scan, pairing for changed shoots, L1 metadata and sidecar
/// ratings for them. Does nothing while the location is offline.
pub fn sync_once(
    cat: &mut Catalog,
    location: &Location,
    stability: &mut l0::Stability,
) -> Result<SyncOutcome> {
    if !locations::status(location, &locations::mounts()).is_online() {
        return Ok(SyncOutcome::default());
    }
    let opts = l0::ScanOptions::for_location(location);
    let scan = l0::scan_location_stable(cat, location, &opts, stability)?;
    let mut outcome = SyncOutcome {
        change: None,
        files_held: scan.files_held > 0,
    };
    if !scan.changed() {
        return Ok(outcome);
    }
    assets::refresh(cat, &scan.changed_folders)?;
    let mut shoots = Vec::new();
    for &folder in &scan.changed_folders {
        let shoot: Option<i64> = cat
            .conn()
            .query_row(
                "SELECT CASE WHEN format_subdir = 1 THEN parent_id ELSE id END
                 FROM folders WHERE id = ?1",
                [folder],
                |r| r.get(0),
            )
            .ok();
        if let Some(shoot) = shoot
            && !shoots.contains(&shoot)
        {
            shoots.push(shoot);
        }
    }
    l1::run(cat, &shoots)?;
    // Ratings and labels changed by Lightroom, RapidRAW, … (reads only).
    metadata::sync_shoots(cat, &shoots)?;
    outcome.change = Some(FolderChanged {
        location_id: location.id,
        folders: scan.changed_folders,
        shoots,
    });
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pv::catalog::FILE_NAME;
    use std::fs;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::time::{Instant, SystemTime};

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        catalog: PathBuf,
        loc: Location,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("DCIM");
        fs::create_dir_all(root.join("10561031")).unwrap();
        let catalog = tmp.path().join(FILE_NAME);
        let cat = Catalog::open(&catalog).unwrap();
        let id = locations::add(&cat, &root, None).unwrap();
        let loc = locations::get(&cat, id).unwrap().unwrap();
        Fixture {
            _tmp: tmp,
            root,
            catalog,
            loc,
        }
    }

    fn photo_count(catalog: &Path) -> i64 {
        Catalog::open(catalog)
            .unwrap()
            .conn()
            .query_row("SELECT COUNT(*) FROM photos", [], |r| r.get(0))
            .unwrap()
    }

    fn wait_for(
        rx: &mpsc::Receiver<FolderChanged>,
        timeout: Duration,
        mut done: impl FnMut() -> bool,
    ) {
        let deadline = Instant::now() + timeout;
        while !done() {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out");
            let _ = rx.recv_timeout(left.min(Duration::from_millis(200)));
        }
    }

    #[test]
    fn growing_file_is_held_until_its_size_settles() {
        let f = fixture();
        let mut cat = Catalog::open(&f.catalog).unwrap();
        let mut stability = l0::Stability::new(Duration::from_secs(60));
        let opts = l0::ScanOptions::default();
        let path = f.root.join("10561031/DSC03600.ARW");
        let mut file = fs::File::create(&path).unwrap();
        let mut scan = |cat: &mut Catalog| {
            l0::scan_location_stable(cat, &f.loc, &opts, &mut stability).unwrap()
        };

        file.write_all(&[0; 1000]).unwrap();
        let s = scan(&mut cat);
        assert_eq!((s.files_held, s.files_added), (1, 0));

        file.write_all(&[0; 1000]).unwrap(); // still uploading
        let s = scan(&mut cat);
        assert_eq!((s.files_held, s.files_added), (1, 0));

        let s = scan(&mut cat); // unchanged since the last poll
        assert_eq!((s.files_held, s.files_added), (0, 1));
        let s = scan(&mut cat);
        assert_eq!((s.files_held, s.files_added, s.files_changed), (0, 0, 0));
    }

    #[test]
    fn old_files_are_never_held() {
        let f = fixture();
        let path = f.root.join("10561031/DSC03600.ARW");
        fs::write(&path, b"x").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(old)).unwrap();
        let mut cat = Catalog::open(&f.catalog).unwrap();
        let mut stability = l0::Stability::new(Duration::from_secs(60));
        let s = l0::scan_location_stable(&mut cat, &f.loc, &Default::default(), &mut stability)
            .unwrap();
        assert_eq!((s.files_held, s.files_added), (0, 1));
    }

    #[test]
    fn polling_picks_up_copied_files() {
        let f = fixture();
        let (tx, rx) = mpsc::channel();
        let opts = WatchOptions {
            poll_interval: Duration::from_millis(200),
            fs_events: false,
        };
        let _watcher = watch(&f.catalog, &f.loc, opts, move |c| {
            let _ = tx.send(c);
        })
        .unwrap();

        fs::write(f.root.join("10561031/DSC03600.ARW"), b"raw").unwrap();
        fs::write(f.root.join("10561031/DSC03600.HIF"), b"hif").unwrap();
        wait_for(&rx, Duration::from_secs(5), || photo_count(&f.catalog) == 1);
    }

    #[test]
    fn fs_events_pick_up_a_local_upload_without_waiting_for_the_poll() {
        let f = fixture();
        let (tx, rx) = mpsc::channel();
        let opts = WatchOptions {
            poll_interval: Duration::from_secs(600),
            fs_events: true,
        };
        let _watcher = watch(&f.catalog, &f.loc, opts, move |c| {
            let _ = tx.send(c);
        })
        .unwrap();
        thread::sleep(Duration::from_millis(500)); // initial sync

        let path = f.root.join("10561031/DSC03601.HIF");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(&[0; 4096]).unwrap();
        drop(file);
        // Held once (fresh), then accepted on the re-check a couple of seconds later.
        wait_for(&rx, Duration::from_secs(10), || {
            photo_count(&f.catalog) == 1
        });
    }

    #[test]
    fn options_from_location_settings() {
        let mut loc = fixture().loc;
        assert!(WatchOptions::for_location(&loc).fs_events);
        assert_eq!(WatchOptions::for_location(&loc).poll_interval, DEFAULT_POLL);
        loc.volume_id = Some("smb://nas/Public".into());
        loc.settings_json = r#"{"pollSeconds": 30}"#.into();
        let opts = WatchOptions::for_location(&loc);
        assert!(!opts.fs_events);
        assert_eq!(opts.poll_interval, Duration::from_secs(30));
    }
}
