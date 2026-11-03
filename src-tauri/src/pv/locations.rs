//! Pairview: Library Locations (PRD §6.1.1, docs/IMPLEMENTATION_PLAN.md M1-02).
//!
//! A location is remembered by **volume identity + path relative to the volume**, not only by
//! its mount path. When the NAS comes back as `/Volumes/Public-1`, the same folder is found again
//! and the catalog (ratings, cache) is kept, because folders are stored relative to the location.
//!
//! Anything that touches a network volume can block for a long time when the server is gone, so
//! every file-system probe here runs with a timeout.

// The commands and Locations manager (M1-09, M1-10) are the first users.
#![allow(dead_code)]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::OptionalExtension;

use crate::pv::catalog::{Catalog, now_ms};

/// How long a location may take to answer before it counts as unreachable.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub mount_point: PathBuf,
    /// `uuid:<volume UUID>` for local disks, `smb://server/share` (or afp/nfs/webdav) for shares.
    pub volume_id: String,
    /// Location path relative to `mount_point`, `/`-separated, `""` for the volume root.
    pub rel_path: String,
    pub network: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub mount_point: PathBuf,
    pub volume_id: String,
    pub network: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub id: i64,
    pub name: String,
    pub path: PathBuf,
    pub volume_id: Option<String>,
    pub volume_rel_path: Option<String>,
    pub sort_order: i64,
    pub settings_json: String,
    pub last_scan_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Online,
    /// The volume isn't mounted (NAS share disconnected, drive unplugged). Offer Connect.
    NotMounted,
    /// The volume is mounted but the folder is gone (moved or renamed). Offer Relocate.
    Missing,
    /// Something else is mounted at this path. Treated as offline so nothing is changed there.
    DifferentVolume,
    /// No answer within `PROBE_TIMEOUT`, or an I/O error.
    Unreachable,
}

impl Status {
    pub fn is_online(self) -> bool {
        self == Status::Online
    }
}

/// Runs `f` on a helper thread and gives up after `timeout`. A hung probe (dead SMB server)
/// leaves its thread blocked in the kernel until the OS gives up; the caller is never blocked.
pub fn probe<T: Send + 'static>(
    timeout: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("pv-location-probe".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .ok()?;
    rx.recv_timeout(timeout).ok()
}

/// Works out which volume `path` is on and where it sits on that volume.
pub fn identify(path: &Path) -> Result<Identity> {
    let path = path.to_path_buf();
    let shown = path.display().to_string();
    probe(PROBE_TIMEOUT, move || identify_now(&path))
        .ok_or_else(|| anyhow!("{shown} did not respond within {PROBE_TIMEOUT:?}"))?
}

fn identify_now(path: &Path) -> Result<Identity> {
    let path = path
        .canonicalize()
        .with_context(|| format!("cannot open {}", path.display()))?;
    let mount = imp::mount_of(&path)?;
    // APFS firmlinks: /Users/… lives on /System/Volumes/Data without that prefix in its path.
    let rel = path
        .strip_prefix(&mount.mount_point)
        .or_else(|_| path.strip_prefix("/"))
        .map_err(|_| anyhow!("{} is not an absolute path", path.display()))?;
    Ok(Identity {
        mount_point: mount.mount_point,
        volume_id: mount.volume_id,
        rel_path: rel_to_string(rel),
        network: mount.network,
    })
}

fn rel_to_string(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Currently mounted volumes. Never blocks on unreachable shares (`MNT_NOWAIT`).
pub fn mounts() -> Vec<Mount> {
    imp::mounts()
}

/// `smb://server/share` for a network location, so the UI can ask the OS to mount it.
pub fn connect_url(volume_id: &str) -> Option<&str> {
    ["smb://", "afp://", "nfs://"]
        .iter()
        .any(|scheme| volume_id.starts_with(scheme))
        .then_some(volume_id)
}

pub fn status(location: &Location, mounts: &[Mount]) -> Status {
    let path = location.path.clone();
    let expected = location.volume_id.clone();
    let answer = probe(PROBE_TIMEOUT, move || -> io::Result<Option<String>> {
        if !std::fs::metadata(&path)?.is_dir() {
            return Err(io::ErrorKind::NotFound.into());
        }
        std::fs::read_dir(&path)?;
        Ok(match expected {
            Some(_) => identify_now(&path).ok().map(|id| id.volume_id),
            None => None,
        })
    });
    let Some(answer) = answer else {
        return Status::Unreachable;
    };
    match (answer, &location.volume_id) {
        (Ok(found), Some(expected)) => match found {
            Some(found) if &found == expected => Status::Online,
            Some(_) => Status::DifferentVolume,
            None => Status::Unreachable,
        },
        (Ok(_), None) => Status::Online,
        (Err(e), expected) if e.kind() == io::ErrorKind::NotFound => match expected {
            Some(id) if !mounts.iter().any(|m| &m.volume_id == id) => Status::NotMounted,
            _ => Status::Missing,
        },
        (Err(_), _) => Status::Unreachable,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Suggestion {
    pub path: PathBuf,
    /// Same volume identity. `false` means the same share name on another server address
    /// (e.g. the NAS's IP instead of its Bonjour name), which the owner should check.
    pub exact: bool,
}

/// Where a location that went missing might be now: the same volume (or share) mounted at a
/// different path, with the location's folder present. The owner confirms before `relocate`.
pub fn suggest_relocation(location: &Location, mounts: &[Mount]) -> Vec<Suggestion> {
    let (Some(volume_id), Some(rel)) = (&location.volume_id, &location.volume_rel_path) else {
        return Vec::new();
    };
    let share = share_name(volume_id);
    let mut out: Vec<Suggestion> = mounts
        .iter()
        .filter_map(|m| {
            let exact = &m.volume_id == volume_id;
            let same_share = m.network && share.is_some() && share_name(&m.volume_id) == share;
            (exact || same_share).then(|| Suggestion {
                path: m.mount_point.join(rel),
                exact,
            })
        })
        .filter(|s| s.path != location.path)
        .filter(|s| {
            let p = s.path.clone();
            probe(PROBE_TIMEOUT, move || p.is_dir()).unwrap_or(false)
        })
        .collect();
    out.sort_by_key(|s| !s.exact);
    out
}

fn share_name(volume_id: &str) -> Option<&str> {
    let rest = volume_id.split_once("://")?.1;
    let share = rest.split_once('/')?.1;
    (!share.is_empty()).then_some(share)
}

// ---- Catalog ---------------------------------------------------------------------------------

const SELECT: &str = "SELECT id, name, path, volume_id, volume_rel_path, sort_order, settings_json,
                             last_scan_at
                      FROM locations";

fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Location> {
    Ok(Location {
        id: row.get(0)?,
        name: row.get(1)?,
        path: PathBuf::from(row.get::<_, String>(2)?),
        volume_id: row.get(3)?,
        volume_rel_path: row.get(4)?,
        sort_order: row.get(5)?,
        settings_json: row.get(6)?,
        last_scan_at: row.get(7)?,
    })
}

pub fn list(cat: &Catalog) -> Result<Vec<Location>> {
    let mut stmt = cat
        .conn()
        .prepare(&format!("{SELECT} ORDER BY sort_order, id"))?;
    let rows = stmt.query_map([], from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get(cat: &Catalog, id: i64) -> Result<Option<Location>> {
    Ok(cat
        .conn()
        .query_row(&format!("{SELECT} WHERE id = ?1"), [id], from_row)
        .optional()?)
}

/// Adds a folder as a location. Nested locations are refused: the same files would be indexed
/// twice and could end up in two Photos.
pub fn add(cat: &Catalog, path: &Path, name: Option<&str>) -> Result<i64> {
    let identity = identify(path)?;
    let path = path.canonicalize()?;
    check_not_nested(cat, &path, None)?;
    let name = name
        .map(str::to_owned)
        .unwrap_or_else(|| default_name(&path));
    let next_order: i64 = cat.conn().query_row(
        "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM locations",
        [],
        |r| r.get(0),
    )?;
    cat.conn().execute(
        "INSERT INTO locations (name, path, volume_id, volume_rel_path, sort_order, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (
            &name,
            path_str(&path)?,
            &identity.volume_id,
            &identity.rel_path,
            next_order,
            now_ms(),
        ),
    )?;
    Ok(cat.conn().last_insert_rowid())
}

/// Points a location at its new path (PRD §6.1.1 Relocate). Folders, files, Photos and
/// metadata are stored relative to the location, so they all carry over.
pub fn relocate(cat: &Catalog, id: i64, new_path: &Path) -> Result<()> {
    let identity = identify(new_path)?;
    let path = new_path.canonicalize()?;
    check_not_nested(cat, &path, Some(id))?;
    let changed = cat.conn().execute(
        "UPDATE locations SET path = ?2, volume_id = ?3, volume_rel_path = ?4 WHERE id = ?1",
        (
            id,
            path_str(&path)?,
            &identity.volume_id,
            &identity.rel_path,
        ),
    )?;
    if changed == 0 {
        bail!("no location with id {id}");
    }
    Ok(())
}

/// Forgets a location and everything cached under it. Files on disk are never touched.
pub fn remove(cat: &Catalog, id: i64) -> Result<()> {
    cat.conn()
        .execute("DELETE FROM locations WHERE id = ?1", [id])?;
    Ok(())
}

pub fn rename(cat: &Catalog, id: i64, name: &str) -> Result<()> {
    cat.conn().execute(
        "UPDATE locations SET name = ?2 WHERE id = ?1",
        (id, name.trim()),
    )?;
    Ok(())
}

/// Per-location scan settings, stored in `settings_json` next to any other keys.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocationSettings {
    /// Folder names, or paths relative to the location, to skip (pv::scan::l0).
    pub excludes: Vec<String>,
    /// Poll interval for the watcher (pv::scan::watch); `None` = the default.
    pub poll_seconds: Option<u64>,
}

impl LocationSettings {
    pub fn of(location: &Location) -> Self {
        serde_json::from_str(&location.settings_json).unwrap_or_default()
    }
}

pub fn update_settings(cat: &Catalog, id: i64, settings: &LocationSettings) -> Result<()> {
    let location = get(cat, id)?.ok_or_else(|| anyhow!("no location with id {id}"))?;
    let mut json: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&location.settings_json).unwrap_or_default();
    let excludes: Vec<String> = settings
        .excludes
        .iter()
        .map(|e| e.trim().trim_matches('/').to_string())
        .filter(|e| !e.is_empty())
        .collect();
    json.insert("excludes".into(), serde_json::json!(excludes));
    match settings.poll_seconds {
        Some(s) => json.insert("pollSeconds".into(), serde_json::json!(s.max(5))),
        None => json.remove("pollSeconds"),
    };
    cat.conn().execute(
        "UPDATE locations SET settings_json = ?2 WHERE id = ?1",
        (id, serde_json::Value::Object(json).to_string()),
    )?;
    Ok(())
}

/// Sets the sidebar order to `ids` (first = top).
pub fn reorder(cat: &mut Catalog, ids: &[i64]) -> Result<()> {
    let tx = cat.conn_mut().transaction()?;
    for (order, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE locations SET sort_order = ?2 WHERE id = ?1",
            (id, order as i64),
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn check_not_nested(cat: &Catalog, path: &Path, except: Option<i64>) -> Result<()> {
    for other in list(cat)? {
        if Some(other.id) == except {
            continue;
        }
        if path.starts_with(&other.path) || other.path.starts_with(path) {
            bail!(
                "{} overlaps the location \"{}\" ({})",
                path.display(),
                other.name,
                other.path.display()
            );
        }
    }
    Ok(())
}

fn default_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("{} is not valid UTF-8", path.display()))
}

// ---- Platform --------------------------------------------------------------------------------

/// Builds `smb://server/share` from a mount source like `//user@server/share`.
fn network_volume_id(fs_type: &str, source: &str) -> Option<String> {
    let scheme = match fs_type {
        "smbfs" => "smb",
        "afpfs" => "afp",
        "nfs" => "nfs",
        "webdav" => return Some(source.trim_end_matches('/').to_string()),
        _ => return None,
    };
    let (host, share) = if scheme == "nfs" {
        let (host, export) = source.split_once(':')?;
        (host, export.trim_start_matches('/'))
    } else {
        let rest = source.trim_start_matches('/');
        let (authority, share) = rest.split_once('/')?;
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        (host, share)
    };
    Some(format!(
        "{scheme}://{}/{}",
        host.to_lowercase(),
        share.trim_end_matches('/')
    ))
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Mount, network_volume_id};
    use anyhow::Result;
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    pub fn mount_of(path: &Path) -> Result<Mount> {
        let c_path = CString::new(path.as_os_str().as_bytes())?;
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: c_path is NUL-terminated and st is a valid statfs to write into.
        if unsafe { libc::statfs(c_path.as_ptr(), &mut st) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(to_mount(&st))
    }

    pub fn mounts() -> Vec<Mount> {
        // SAFETY: a null buffer asks getfsstat for the count only.
        let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
        if count <= 0 {
            return Vec::new();
        }
        // Room for a few mounts appearing between the two calls.
        let mut buf: Vec<libc::statfs> = vec![unsafe { std::mem::zeroed() }; count as usize + 8];
        let bytes = (buf.len() * std::mem::size_of::<libc::statfs>()) as libc::c_int;
        // SAFETY: buf holds `bytes` bytes of statfs entries.
        let filled = unsafe { libc::getfsstat(buf.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
        if filled <= 0 {
            return Vec::new();
        }
        buf.truncate(filled as usize);
        buf.iter()
            .filter(|st| !matches!(c_str(&st.f_fstypename).as_str(), "devfs" | "autofs"))
            .map(to_mount)
            .collect()
    }

    fn to_mount(st: &libc::statfs) -> Mount {
        let fs_type = c_str(&st.f_fstypename);
        let source = c_str(&st.f_mntfromname);
        let mount_point = PathBuf::from(c_str(&st.f_mntonname));
        match network_volume_id(&fs_type, &source) {
            Some(volume_id) => Mount {
                mount_point,
                volume_id,
                network: true,
            },
            None => {
                let volume_id = volume_uuid(&mount_point)
                    .map(|u| format!("uuid:{u}"))
                    // FAT/exFAT cards may have no UUID: fall back to the volume name.
                    .unwrap_or_else(|| format!("name:{}", mount_point.display()));
                Mount {
                    mount_point,
                    volume_id,
                    network: false,
                }
            }
        }
    }

    fn c_str(buf: &[c_char]) -> String {
        // SAFETY: statfs strings are NUL-terminated within their fixed-size arrays.
        unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }

    fn volume_uuid(mount_point: &Path) -> Option<String> {
        #[repr(C, packed(4))]
        struct Reply {
            length: u32,
            uuid: [u8; 16],
        }
        let c_path = CString::new(mount_point.as_os_str().as_bytes()).ok()?;
        let mut attrs = libc::attrlist {
            bitmapcount: libc::ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: 0,
            volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
            dirattr: 0,
            fileattr: 0,
            forkattr: 0,
        };
        let mut reply = Reply {
            length: 0,
            uuid: [0; 16],
        };
        // SAFETY: attrs and reply are valid for the sizes passed; getattrlist writes at most
        // size_of::<Reply>() bytes.
        let rc = unsafe {
            libc::getattrlist(
                c_path.as_ptr(),
                &mut attrs as *mut _ as *mut c_void,
                &mut reply as *mut _ as *mut c_void,
                std::mem::size_of::<Reply>(),
                0,
            )
        };
        let uuid = reply.uuid;
        if rc != 0 || uuid == [0; 16] {
            return None;
        }
        let hex: String = uuid.iter().map(|b| format!("{b:02X}")).collect();
        Some(format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        ))
    }
}

// Windows identity (volume GUID / UNC share) comes with the Windows port (PRD §10.1).
#[cfg(not(target_os = "macos"))]
mod imp {
    use super::Mount;
    use anyhow::{Result, anyhow};
    use std::path::{Component, Path, PathBuf};

    pub fn mount_of(path: &Path) -> Result<Mount> {
        let root: PathBuf = path
            .components()
            .take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
            .collect();
        if root.as_os_str().is_empty() {
            return Err(anyhow!("{} is not an absolute path", path.display()));
        }
        Ok(Mount {
            volume_id: format!("root:{}", root.display()),
            mount_point: root,
            network: false,
        })
    }

    pub fn mounts() -> Vec<Mount> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Instant;

    fn location(path: &Path, volume_id: &str, rel: &str) -> Location {
        Location {
            id: 1,
            name: "NAS".into(),
            path: path.to_path_buf(),
            volume_id: Some(volume_id.into()),
            volume_rel_path: Some(rel.into()),
            sort_order: 0,
            settings_json: "{}".into(),
            last_scan_at: None,
        }
    }

    fn share(mount_point: &Path, volume_id: &str) -> Mount {
        Mount {
            mount_point: mount_point.to_path_buf(),
            volume_id: volume_id.into(),
            network: true,
        }
    }

    #[test]
    fn network_ids_ignore_user_and_host_case() {
        let cases = [
            (
                "smbfs",
                "//xpy@DS220._smb._tcp.local/Public",
                "smb://ds220._smb._tcp.local/Public",
            ),
            (
                "smbfs",
                "//GUEST:@192.168.1.10/Public/",
                "smb://192.168.1.10/Public",
            ),
            ("afpfs", "//nas.local/Photos", "afp://nas.local/Photos"),
            ("nfs", "nas:/volume1/photos", "nfs://nas/volume1/photos"),
        ];
        for (fs_type, source, expected) in cases {
            assert_eq!(
                network_volume_id(fs_type, source).as_deref(),
                Some(expected)
            );
        }
        assert_eq!(network_volume_id("apfs", "/dev/disk3s5"), None);
        assert_eq!(connect_url("smb://nas/Public"), Some("smb://nas/Public"));
        assert_eq!(connect_url("uuid:ABC"), None);
    }

    #[test]
    fn same_folder_at_a_new_mount_path_is_matched() {
        // Two roots stand in for /Volumes/Public and /Volumes/Public-1.
        let tmp = tempfile::tempdir().unwrap();
        let old_root = tmp.path().join("Public");
        let new_root = tmp.path().join("Public-1");
        let other_root = tmp.path().join("Other");
        for root in [&new_root, &other_root] {
            fs::create_dir_all(root.join("Photos/A/DCIM")).unwrap();
        }
        let loc = location(
            &old_root.join("Photos/A/DCIM"),
            "smb://nas/Public",
            "Photos/A/DCIM",
        );
        let mounts = [
            share(&other_root, "smb://nas/Other"),
            share(&new_root, "smb://nas/Public"),
        ];

        assert_eq!(status(&loc, &mounts), Status::Missing);
        assert_eq!(
            suggest_relocation(&loc, &mounts),
            vec![Suggestion {
                path: new_root.join("Photos/A/DCIM"),
                exact: true
            }]
        );
    }

    #[test]
    fn same_share_on_another_address_is_a_weaker_suggestion() {
        let tmp = tempfile::tempdir().unwrap();
        let by_name = tmp.path().join("by-name");
        let by_ip = tmp.path().join("by-ip");
        for root in [&by_name, &by_ip] {
            fs::create_dir_all(root.join("DCIM")).unwrap();
        }
        let loc = location(
            &tmp.path().join("gone/DCIM"),
            "smb://ds220.local/Public",
            "DCIM",
        );
        let mounts = [
            share(&by_ip, "smb://192.168.1.10/Public"),
            share(&by_name, "smb://ds220.local/Public"),
        ];
        let found = suggest_relocation(&loc, &mounts);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].exact, found[1].exact), (true, false));
        assert_eq!(found[0].path, by_name.join("DCIM"));
    }

    #[test]
    fn no_suggestion_when_the_folder_is_not_on_the_new_mount() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(&tmp.path().join("old/DCIM"), "smb://nas/Public", "DCIM");
        let mounts = [share(tmp.path(), "smb://nas/Public")];
        assert!(suggest_relocation(&loc, &mounts).is_empty());
    }

    #[test]
    fn status_reports_each_offline_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let here = identify(tmp.path()).unwrap();
        let mounts = mounts();

        let online = location(tmp.path(), &here.volume_id, &here.rel_path);
        assert_eq!(status(&online, &mounts), Status::Online);

        let moved = location(&tmp.path().join("moved"), &here.volume_id, "x");
        assert_eq!(status(&moved, &mounts), Status::Missing);

        let unmounted = location(
            Path::new("/Volumes/PairviewNoSuchShare/DCIM"),
            "smb://nas/x",
            "DCIM",
        );
        assert_eq!(status(&unmounted, &mounts), Status::NotMounted);

        let wrong = location(tmp.path(), "smb://nas/Public", "DCIM");
        assert_eq!(status(&wrong, &mounts), Status::DifferentVolume);
    }

    #[test]
    fn hung_probe_gives_up_within_the_timeout() {
        let start = Instant::now();
        let answer = probe(PROBE_TIMEOUT, || thread::sleep(Duration::from_secs(10)));
        assert_eq!(answer, None);
        assert!(start.elapsed() < PROBE_TIMEOUT + Duration::from_millis(500));
    }

    #[test]
    fn identify_finds_the_volume_of_a_local_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let id = identify(tmp.path()).unwrap();
        assert!(!id.network);
        assert!(!id.rel_path.is_empty() && !id.rel_path.starts_with('/'));
        if cfg!(target_os = "macos") {
            assert!(id.volume_id.starts_with("uuid:"), "{}", id.volume_id);
            assert!(mounts().iter().any(|m| m.volume_id == id.volume_id));
        }
        // Mount point + relative path leads back to the same folder (possibly through an APFS
        // firmlink, so compare contents rather than path strings).
        fs::write(tmp.path().join("marker"), b"").unwrap();
        assert!(id.mount_point.join(&id.rel_path).join("marker").exists());
    }

    /// `PV_FIXTURES=~/Desktop/2026-10-23_10261023 cargo test --lib pv::locations -- --nocapture`
    #[test]
    fn real_fixture_is_online() {
        let Ok(dir) = std::env::var("PV_FIXTURES") else {
            return;
        };
        let cat = Catalog::open_in_memory().unwrap();
        let loc = get(&cat, add(&cat, Path::new(&dir), None).unwrap())
            .unwrap()
            .unwrap();
        println!("{loc:?}");
        assert_eq!(status(&loc, &mounts()), Status::Online);
    }

    #[test]
    fn catalog_add_list_reorder_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let mut cat = Catalog::open_in_memory().unwrap();
        let id_a = add(&cat, &a, None).unwrap();
        let id_b = add(&cat, &b, Some("NAS — Camera")).unwrap();

        let all = list(&cat).unwrap();
        assert_eq!(all.iter().map(|l| l.id).collect::<Vec<_>>(), [id_a, id_b]);
        assert_eq!(all[0].name, "a");
        assert_eq!(all[0].path, a.canonicalize().unwrap());
        assert!(all[0].volume_id.is_some());

        reorder(&mut cat, &[id_b, id_a]).unwrap();
        rename(&cat, id_a, "  Desktop copy ").unwrap();
        let all = list(&cat).unwrap();
        assert_eq!(all.iter().map(|l| l.id).collect::<Vec<_>>(), [id_b, id_a]);
        assert_eq!(all[1].name, "Desktop copy");
    }

    #[test]
    fn settings_round_trip_and_feed_the_scanner_and_watcher() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = Catalog::open_in_memory().unwrap();
        let id = add(&cat, tmp.path(), None).unwrap();
        let settings = LocationSettings {
            excludes: vec![" exports/ ".into(), "".into(), "2026/old".into()],
            poll_seconds: Some(1),
        };
        update_settings(&cat, id, &settings).unwrap();
        let loc = get(&cat, id).unwrap().unwrap();
        assert_eq!(
            LocationSettings::of(&loc),
            LocationSettings {
                excludes: vec!["exports".into(), "2026/old".into()],
                poll_seconds: Some(5),
            }
        );
        let scan = crate::pv::scan::l0::ScanOptions::for_location(&loc);
        assert_eq!(scan.excludes, ["exports", "2026/old"]);
        let watch = crate::pv::scan::watch::WatchOptions::for_location(&loc);
        assert_eq!(watch.poll_interval, Duration::from_secs(5));

        update_settings(&cat, id, &LocationSettings::default()).unwrap();
        let loc = get(&cat, id).unwrap().unwrap();
        assert_eq!(LocationSettings::of(&loc), LocationSettings::default());
    }

    #[test]
    fn catalog_refuses_nested_locations() {
        let tmp = tempfile::tempdir().unwrap();
        let inner = tmp.path().join("DCIM/2026-10-23_10261023");
        fs::create_dir_all(&inner).unwrap();
        let cat = Catalog::open_in_memory().unwrap();
        add(&cat, &tmp.path().join("DCIM"), None).unwrap();
        assert!(add(&cat, &inner, None).is_err());
        assert!(add(&cat, tmp.path(), None).is_err());
    }

    #[test]
    fn relocate_keeps_everything_under_the_location() {
        let tmp = tempfile::tempdir().unwrap();
        let (old, new) = (
            tmp.path().join("Public/DCIM"),
            tmp.path().join("Public-1/DCIM"),
        );
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        let cat = Catalog::open_in_memory().unwrap();
        let id = add(&cat, &old, None).unwrap();
        cat.conn()
            .execute(
                "INSERT INTO folders (location_id, rel_path, name) VALUES (?1, '2026-10-23', '2026-10-23')",
                [id],
            )
            .unwrap();

        relocate(&cat, id, &new).unwrap();
        let loc = get(&cat, id).unwrap().unwrap();
        assert_eq!(loc.path, new.canonicalize().unwrap());
        assert!(loc.volume_rel_path.unwrap().ends_with("Public-1/DCIM"));
        let folders: i64 = cat
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE location_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(folders, 1);
        assert!(relocate(&cat, 999, &new).is_err());
    }
}
