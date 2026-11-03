//! Photo-level ratings, flags, color labels and the Keep-RAW lock (docs/IMPLEMENTATION_PLAN.md
//! A6, M1-08). 🛡 This module writes next to the owner's photos.
//!
//! Write path for one Photo:
//! 1. refuse unless its location is online;
//! 2. read the current values from its sidecars (so edits made in Lightroom or RapidRAW since the
//!    last scan are the starting point, not overwritten);
//! 3. edit its XMP sidecar (only the changed properties; created if missing), atomically;
//! 4. mirror rating, flag and color label into the RapidRAW `.rrdata` of every member file, so
//!    RapidRAW's editor and grid show the same values;
//! 5. store the result in the catalog.
//!
//! Only `.xmp` and `.rrdata` files are ever written; every target path is checked for that.
//! Image files are never opened for writing.
//!
//! The XMP sits next to the RAW (`DSC0001.xmp`, Adobe naming), or next to the display copy when
//! there is no RAW. A display copy split from its RAW by the timestamp guard uses
//! `DSC0001.JPG.xmp`, so the two Photos don't share a sidecar.
//!
//! Read path: XMP wins; without one, the representative's `.rrdata`; else defaults.

// The commands (M1-09) are the first users.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::Serialize;

use crate::exif_processing::{get_primary_sidecar_path, update_sidecar};
use crate::image_processing::{ImageFlag, ImageMetadata};
use crate::pv::catalog::{Catalog, now_ms};
use crate::pv::locations::{self, Location, Status};
use crate::pv::xmp::{self, XmpEdit, XmpFields};
use crate::tagging::COLOR_TAG_PREFIX;

/// Color labels Pairview offers (stored lowercase; XMP gets `Red`, … like Lightroom).
pub const COLOR_LABELS: &[&str] = &["red", "yellow", "green", "blue", "purple"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flag {
    #[default]
    None,
    Pick,
    Reject,
}

impl Flag {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pick => "pick",
            Self::Reject => "reject",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "pick" => Some(Self::Pick),
            "reject" => Some(Self::Reject),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeepRaw {
    #[default]
    Auto,
    Keep,
    Drop,
}

impl KeepRaw {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Keep => "keep",
            Self::Drop => "drop",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "keep" => Some(Self::Keep),
            "drop" => Some(Self::Drop),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoMeta {
    pub rating: u8,
    pub flag: Flag,
    /// A `COLOR_LABELS` entry, or other text another tool wrote (kept as is).
    pub label: Option<String>,
    pub keep_raw: KeepRaw,
}

/// What to change; `None` fields are left alone. `label: Some(None)` clears the label.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    pub rating: Option<u8>,
    pub flag: Option<Flag>,
    pub label: Option<Option<String>>,
    pub keep_raw: Option<KeepRaw>,
}

impl Change {
    fn validate(&self) -> Result<()> {
        if let Some(r) = self.rating {
            ensure!(r <= 5, "rating must be 0–5, got {r}");
        }
        if let Some(Some(label)) = &self.label {
            ensure!(
                COLOR_LABELS.contains(&label.as_str()),
                "unknown color label {label:?}"
            );
        }
        Ok(())
    }
}

impl PhotoMeta {
    /// Applies a change. Like RapidRAW: rating a rejected Photo un-rejects it, and rejecting
    /// clears the stars (XMP stores a reject as rating -1).
    pub fn with(&self, change: &Change) -> Self {
        let mut next = self.clone();
        if let Some(flag) = change.flag {
            next.flag = flag;
            if flag == Flag::Reject {
                next.rating = 0;
            }
        }
        if let Some(rating) = change.rating {
            next.rating = rating;
            if rating > 0 && next.flag == Flag::Reject {
                next.flag = Flag::None;
            }
        }
        if let Some(label) = &change.label {
            next.label = label.clone();
        }
        if let Some(keep_raw) = change.keep_raw {
            next.keep_raw = keep_raw;
        }
        next
    }

    fn from_xmp(fields: &XmpFields) -> Self {
        let rating = fields.rating.unwrap_or(0);
        let flag = fields
            .flag
            .as_deref()
            .and_then(Flag::parse)
            .unwrap_or(if rating < 0 { Flag::Reject } else { Flag::None });
        Self {
            rating: rating.clamp(0, 5) as u8,
            flag,
            label: fields.label.as_deref().map(label_from_xmp),
            keep_raw: fields
                .keep_raw
                .as_deref()
                .and_then(KeepRaw::parse)
                .unwrap_or_default(),
        }
    }

    fn xmp_rating(&self) -> i64 {
        if self.flag == Flag::Reject {
            -1
        } else {
            self.rating as i64
        }
    }

    /// The XMP edit that turns `self` into `next`, touching only what differs.
    fn xmp_edit(&self, next: &Self) -> XmpEdit {
        XmpEdit {
            rating: (self.xmp_rating() != next.xmp_rating()).then(|| Some(next.xmp_rating())),
            label: (self.label != next.label).then(|| next.label.as_deref().map(label_to_xmp)),
            flag: (self.flag != next.flag).then(|| match next.flag {
                Flag::None => None,
                f => Some(f.as_str().to_string()),
            }),
            keep_raw: (self.keep_raw != next.keep_raw).then(|| match next.keep_raw {
                KeepRaw::Auto => None,
                k => Some(k.as_str().to_string()),
            }),
        }
    }
}

fn label_from_xmp(label: &str) -> String {
    COLOR_LABELS
        .iter()
        .find(|c| c.eq_ignore_ascii_case(label))
        .map_or_else(|| label.to_string(), |c| c.to_string())
}

fn label_to_xmp(label: &str) -> String {
    if COLOR_LABELS.contains(&label) {
        let mut c = label.chars();
        c.next()
            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
            .unwrap_or_default()
    } else {
        label.to_string()
    }
}

/// The values the catalog has for a Photo (defaults when it has none).
pub fn get(cat: &Catalog, photo_id: i64) -> Result<PhotoMeta> {
    let row = cat.conn().query_row(
        "SELECT rating, flag, label, keep_raw FROM metadata WHERE photo_id = ?1",
        [photo_id],
        |r| {
            Ok((
                r.get::<_, u8>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
            ))
        },
    );
    match row {
        Ok((rating, flag, label, keep_raw)) => Ok(PhotoMeta {
            rating,
            flag: Flag::parse(&flag).unwrap_or_default(),
            label,
            keep_raw: KeepRaw::parse(&keep_raw).unwrap_or_default(),
        }),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(PhotoMeta::default()),
        Err(e) => Err(e.into()),
    }
}

/// Changes several Photos. Each Photo succeeds or fails on its own; the result lists them in
/// order.
pub fn set(
    cat: &mut Catalog,
    photo_ids: &[i64],
    change: &Change,
) -> Result<Vec<(i64, Result<PhotoMeta>)>> {
    change.validate()?;
    let mut online: HashMap<i64, Status> = HashMap::new();
    let mounts = locations::mounts();
    let mut results = Vec::new();
    for &photo_id in photo_ids {
        let result = (|| {
            let files = photo_files(cat, photo_id)?;
            let status = *online
                .entry(files.location.id)
                .or_insert_with(|| locations::status(&files.location, &mounts));
            ensure!(
                status.is_online(),
                "{} is offline ({status:?}); nothing was changed",
                files.location.name
            );
            set_one(cat, photo_id, &files, change)
        })();
        results.push((photo_id, result));
    }
    Ok(results)
}

fn set_one(cat: &Catalog, photo_id: i64, files: &PhotoFiles, change: &Change) -> Result<PhotoMeta> {
    let (current, existing_xmp) = read_sidecars(files)?;
    let next = current.with(change);
    // A new XMP gets the full state: `current` may have come from `.rrdata`.
    let edit = match existing_xmp {
        Some(_) => current.xmp_edit(&next),
        None => PhotoMeta::default().xmp_edit(&next),
    };

    if !edit.is_empty() {
        let out = xmp::apply(existing_xmp.as_deref(), &edit)
            .with_context(|| format!("{} was left unchanged", files.xmp.display()))?;
        write_sidecar(&files.xmp, "xmp", out.as_bytes())?;
    }
    // Whenever the owner touches what RapidRAW shows, bring every member's `.rrdata` in line,
    // even if the XMP already had these values.
    if change.rating.is_some() || change.flag.is_some() || change.label.is_some() {
        for member in &files.members {
            mirror_to_rrdata(member, &next)?;
        }
    }
    store(cat, photo_id, &next, sidecar_stamp(files))?;
    Ok(next)
}

/// Writes a sidecar atomically after checking that the target really is one.
fn write_sidecar(path: &Path, ext: &str, contents: &[u8]) -> Result<()> {
    ensure_sidecar_path(path, ext)?;
    crate::file_management::write_file_atomic(path, contents)
        .with_context(|| format!("cannot write {}", path.display()))
}

fn ensure_sidecar_path(path: &Path, ext: &str) -> Result<()> {
    let actual = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    ensure!(
        actual.eq_ignore_ascii_case(ext),
        "refusing to write {}: not a .{ext} sidecar",
        path.display()
    );
    Ok(())
}

fn mirror_to_rrdata(member: &Path, meta: &PhotoMeta) -> Result<()> {
    let sidecar = get_primary_sidecar_path(member);
    ensure_sidecar_path(&sidecar, "rrdata")?;
    update_sidecar(&sidecar, member, |m| {
        m.rating = meta.rating;
        m.flag = match meta.flag {
            Flag::None => None,
            Flag::Pick => Some(ImageFlag::Pick),
            Flag::Reject => Some(ImageFlag::Reject),
        };
        // A label RapidRAW can't show (free text from another tool) leaves its tags alone.
        let known = meta
            .label
            .as_deref()
            .is_none_or(|l| COLOR_LABELS.contains(&l));
        if known {
            let mut tags = m.tags.take().unwrap_or_default();
            tags.retain(|t| !t.starts_with(COLOR_TAG_PREFIX));
            if let Some(label) = &meta.label {
                tags.push(format!("{COLOR_TAG_PREFIX}{label}"));
            }
            m.tags = (!tags.is_empty()).then_some(tags);
        }
    })
    .with_context(|| format!("cannot update {}", sidecar.display()))?;
    Ok(())
}

/// Re-reads sidecars for the Photos of `shoots` whose sidecar files changed since the catalog
/// last read them (another app edited them, or they're new). Reads only; never writes files.
pub fn sync_shoots(cat: &mut Catalog, shoots: &[i64]) -> Result<usize> {
    let mut updated = 0;
    for &shoot in shoots {
        let stale: Vec<i64> = {
            let mut stmt = cat.conn().prepare_cached(
                "SELECT p.id
                 FROM photos p
                 LEFT JOIN metadata md ON md.photo_id = p.id
                 LEFT JOIN (SELECT m.photo_id, MAX(f.mtime) AS stamp
                            FROM photo_members m JOIN files f ON f.id = m.file_id
                            WHERE m.role = 'sidecar' GROUP BY m.photo_id) s ON s.photo_id = p.id
                 WHERE p.shoot_folder_id = ?1
                   AND s.stamp IS NOT NULL
                   AND (md.xmp_mtime IS NULL OR md.xmp_mtime != s.stamp)",
            )?;
            stmt.query_map([shoot], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for photo_id in stale {
            // One unreadable sidecar must not stop the rest of the library from syncing.
            let read = photo_files(cat, photo_id).and_then(|files| {
                let (meta, _) = read_sidecars(&files)?;
                Ok((meta, sidecar_stamp(&files)))
            });
            match read {
                Ok((meta, stamp)) => {
                    store(cat, photo_id, &meta, stamp)?;
                    updated += 1;
                }
                Err(e) => log::warn!("Pairview: sidecars of Photo {photo_id}: {e:#}"),
            }
        }
    }
    Ok(updated)
}

fn store(cat: &Catalog, photo_id: i64, meta: &PhotoMeta, stamp: Option<i64>) -> Result<()> {
    cat.conn().execute(
        "INSERT INTO metadata (photo_id, rating, flag, label, keep_raw, xmp_mtime, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (photo_id) DO UPDATE SET
             rating = excluded.rating, flag = excluded.flag, label = excluded.label,
             keep_raw = excluded.keep_raw, xmp_mtime = excluded.xmp_mtime,
             updated_at = excluded.updated_at
         WHERE rating IS NOT excluded.rating OR flag IS NOT excluded.flag
            OR label IS NOT excluded.label OR keep_raw IS NOT excluded.keep_raw
            OR xmp_mtime IS NOT excluded.xmp_mtime",
        rusqlite::params![
            photo_id,
            meta.rating,
            meta.flag.as_str(),
            meta.label,
            meta.keep_raw.as_str(),
            stamp,
            now_ms()
        ],
    )?;
    Ok(())
}

struct PhotoFiles {
    location: Location,
    /// RAW, display and extra copies.
    members: Vec<PathBuf>,
    /// The Photo's XMP sidecar (may not exist yet).
    xmp: PathBuf,
    /// The file whose `.rrdata` is read when there is no XMP.
    representative: PathBuf,
}

fn photo_files(cat: &Catalog, photo_id: i64) -> Result<PhotoFiles> {
    // A display copy split from its RAW by the guard gets its own `NAME.EXT.xmp`.
    let (location_id, raw_split): (i64, bool) = cat.conn().query_row(
        "SELECT d.location_id,
                p.guard_mismatch = 1 AND NOT EXISTS (
                    SELECT 1 FROM photo_members m WHERE m.photo_id = p.id AND m.role = 'raw')
         FROM photos p JOIN folders d ON d.id = p.shoot_folder_id WHERE p.id = ?1",
        [photo_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let location =
        locations::get(cat, location_id)?.ok_or_else(|| anyhow!("location {location_id} gone"))?;

    let mut stmt = cat.conn().prepare_cached(
        "SELECT m.file_id FROM photo_members m
         WHERE m.photo_id = ?1 AND m.role != 'sidecar'
         ORDER BY CASE m.role WHEN 'raw' THEN 0 WHEN 'display' THEN 1 ELSE 2 END, m.file_id",
    )?;
    let file_ids: Vec<i64> = stmt
        .query_map([photo_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut members = Vec::new();
    for file_id in file_ids {
        members.push(crate::pv::assets::file_path(cat, file_id)?);
    }
    let representative = members
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("Photo {photo_id} has no files"))?;

    let xmp = if raw_split {
        let mut name = representative
            .file_name()
            .unwrap_or_default()
            .to_os_string();
        name.push(".xmp");
        representative.with_file_name(name)
    } else {
        crate::file_management::resolve_xmp_path(&representative)
            .unwrap_or_else(|| representative.with_extension("xmp"))
    };
    Ok(PhotoFiles {
        location,
        members,
        xmp,
        representative,
    })
}

/// Current values from the sidecars, plus the XMP text when there is one.
fn read_sidecars(files: &PhotoFiles) -> Result<(PhotoMeta, Option<String>)> {
    match fs::read_to_string(&files.xmp) {
        Ok(text) => Ok((PhotoMeta::from_xmp(&xmp::read(&text)), Some(text))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Parsed here rather than with upstream's `load_sidecar`, which may rename a corrupt
            // sidecar or rewrite a bloated one: reading must not write.
            let rrdata = get_primary_sidecar_path(&files.representative);
            let Ok(json) = fs::read_to_string(&rrdata) else {
                return Ok((PhotoMeta::default(), None));
            };
            let Ok(m) = serde_json::from_str::<ImageMetadata>(&json) else {
                return Ok((PhotoMeta::default(), None));
            };
            let label = m
                .tags
                .iter()
                .flatten()
                .rev()
                .find_map(|t| t.strip_prefix(COLOR_TAG_PREFIX))
                .map(str::to_string);
            Ok((
                PhotoMeta {
                    rating: m.rating.min(5),
                    flag: match m.flag {
                        Some(ImageFlag::Pick) => Flag::Pick,
                        Some(ImageFlag::Reject) => Flag::Reject,
                        None => Flag::None,
                    },
                    label,
                    keep_raw: KeepRaw::Auto,
                },
                None,
            ))
        }
        // An XMP that exists but can't be read (permissions, NAS hiccup) must not be treated as
        // "no XMP": that would create a new one over it.
        Err(e) => bail!("cannot read {}: {e}", files.xmp.display()),
    }
}

/// Newest mtime among the Photo's sidecars, matching what the L0 scanner records.
fn sidecar_stamp(files: &PhotoFiles) -> Option<i64> {
    std::iter::once(files.xmp.clone())
        .chain(files.members.iter().map(|m| get_primary_sidecar_path(m)))
        .filter_map(|p| fs::metadata(p).ok()?.modified().ok())
        .filter_map(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exif_processing::load_sidecar;
    use crate::pv::assets;
    use crate::pv::scan::l0::{ScanOptions, scan_location};
    use std::collections::BTreeMap;

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        cat: Catalog,
        loc: Location,
    }

    impl Fixture {
        /// Image files get distinct pseudo-random bytes so a change would show in the hash.
        fn new(files: &[&str]) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join("DCIM");
            fs::create_dir_all(&root).unwrap();
            for (i, f) in files.iter().enumerate() {
                let p = root.join(f);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                let bytes: Vec<u8> = (0..4096u32)
                    .map(|b| (b * 31 + i as u32 * 7) as u8)
                    .collect();
                fs::write(p, bytes).unwrap();
            }
            let cat = Catalog::open_in_memory().unwrap();
            let id = locations::add(&cat, &root, None).unwrap();
            let loc = locations::get(&cat, id).unwrap().unwrap();
            let mut f = Self {
                _tmp: tmp,
                root,
                cat,
                loc,
            };
            f.sync();
            f
        }

        fn sync(&mut self) -> usize {
            let opts = ScanOptions {
                full: true,
                ..Default::default()
            };
            let scan = scan_location(&mut self.cat, &self.loc, &opts).unwrap();
            assets::refresh(&mut self.cat, &scan.changed_folders).unwrap();
            let shoots: Vec<i64> = self
                .cat
                .conn()
                .prepare("SELECT DISTINCT shoot_folder_id FROM photos")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            sync_shoots(&mut self.cat, &shoots).unwrap()
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

        fn set(&mut self, photo: i64, change: Change) -> Result<PhotoMeta> {
            let mut results = set(&mut self.cat, &[photo], &change).unwrap();
            results.remove(0).1
        }

        /// Relative path → (blake3, mtime) for every file under the root.
        fn snapshot(&self) -> BTreeMap<String, (String, std::time::SystemTime)> {
            walkdir::WalkDir::new(&self.root)
                .into_iter()
                .map(Result::unwrap)
                .filter(|e| e.file_type().is_file())
                .map(|e| {
                    let rel = e
                        .path()
                        .strip_prefix(&self.root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    let hash = blake3::hash(&fs::read(e.path()).unwrap())
                        .to_hex()
                        .to_string();
                    (rel, (hash, e.metadata().unwrap().modified().unwrap()))
                })
                .collect()
        }

        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.root.join(rel)).unwrap()
        }
    }

    fn is_image(rel: &str) -> bool {
        let ext = rel.rsplit('.').next().unwrap().to_ascii_lowercase();
        !["xmp", "rrdata"].contains(&ext.as_str())
    }

    #[test]
    fn images_are_never_written_and_only_sidecars_appear() {
        let mut f = Fixture::new(&["a/DSC00001.ARW", "a/DSC00001.HIF", "a/DSC00001.JPG"]);
        let before = f.snapshot();
        let photo = f.photo("dsc00001");

        let changes = [
            Change {
                rating: Some(4),
                ..Default::default()
            },
            Change {
                label: Some(Some("red".into())),
                ..Default::default()
            },
            Change {
                flag: Some(Flag::Pick),
                keep_raw: Some(KeepRaw::Keep),
                ..Default::default()
            },
            Change {
                flag: Some(Flag::Reject),
                ..Default::default()
            },
            Change {
                rating: Some(2),
                label: Some(None),
                ..Default::default()
            },
        ];
        for change in changes {
            f.set(photo, change).unwrap();
        }

        let after = f.snapshot();
        for (rel, stamp) in &before {
            assert_eq!(after.get(rel), Some(stamp), "{rel} changed");
        }
        let added: Vec<&String> = after.keys().filter(|k| !before.contains_key(*k)).collect();
        assert_eq!(
            added,
            [
                "a/DSC00001.ARW.rrdata",
                "a/DSC00001.HIF.rrdata",
                "a/DSC00001.JPG.rrdata",
                "a/DSC00001.xmp"
            ]
        );
        assert!(added.iter().all(|a| !is_image(a)));

        let expected = PhotoMeta {
            rating: 2,
            flag: Flag::None,
            label: None,
            keep_raw: KeepRaw::Keep,
        };
        assert_eq!(get(&f.cat, photo).unwrap(), expected);
        assert_eq!(
            PhotoMeta::from_xmp(&xmp::read(&f.read("a/DSC00001.xmp"))),
            expected
        );
        // RapidRAW's own sidecars show the same values.
        for member in ["DSC00001.ARW", "DSC00001.HIF", "DSC00001.JPG"] {
            let m = load_sidecar(&f.root.join(format!("a/{member}.rrdata")));
            assert_eq!(
                (m.rating, m.flag.is_none(), m.tags.is_none()),
                (2, true, true),
                "{member}"
            );
        }
    }

    #[test]
    fn rapidraw_sees_flags_and_labels() {
        let mut f = Fixture::new(&["a/DSC00001.ARW", "a/DSC00001.HIF"]);
        let photo = f.photo("dsc00001");
        f.set(
            photo,
            Change {
                rating: Some(5),
                flag: Some(Flag::Pick),
                label: Some(Some("green".into())),
                ..Default::default()
            },
        )
        .unwrap();
        let m = load_sidecar(&f.root.join("a/DSC00001.HIF.rrdata"));
        assert_eq!(m.rating, 5);
        assert!(matches!(m.flag, Some(ImageFlag::Pick)));
        assert_eq!(m.tags, Some(vec!["color:green".to_string()]));
        let x = xmp::read(&f.read("a/DSC00001.xmp"));
        assert_eq!(
            (x.rating, x.label.as_deref(), x.flag.as_deref()),
            (Some(5), Some("Green"), Some("pick"))
        );

        // Reject: XMP rating -1, stars cleared, RapidRAW flag Reject.
        f.set(
            photo,
            Change {
                flag: Some(Flag::Reject),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(xmp::read(&f.read("a/DSC00001.xmp")).rating, Some(-1));
        let m = load_sidecar(&f.root.join("a/DSC00001.ARW.rrdata"));
        assert!(matches!(m.flag, Some(ImageFlag::Reject)));
        assert_eq!(m.rating, 0);
    }

    #[test]
    fn split_layout_puts_the_xmp_next_to_the_raw() {
        let mut f = Fixture::new(&[
            "s/arw/DSC00001.ARW",
            "s/hif/DSC00001.HIF",
            "s/hif/DSC00002.HIF",
        ]);
        f.set(
            f.photo("dsc00001"),
            Change {
                rating: Some(3),
                ..Default::default()
            },
        )
        .unwrap();
        f.set(
            f.photo("dsc00002"),
            Change {
                rating: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        let files: Vec<String> = f.snapshot().into_keys().filter(|k| !is_image(k)).collect();
        assert_eq!(
            files,
            [
                "s/arw/DSC00001.ARW.rrdata",
                "s/arw/DSC00001.xmp",
                "s/hif/DSC00001.HIF.rrdata",
                "s/hif/DSC00002.HIF.rrdata",
                "s/hif/DSC00002.xmp"
            ]
        );
    }

    const LIGHTROOM: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000 1.000000, 0000/00/00-00:00:00        ">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   xmp:Rating="3"
   xmp:Label="Red"
   crs:Exposure2012="+0.35"
   crs:HasSettings="True">
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;

    #[test]
    fn adobe_sidecar_is_imported_and_edited_in_place() {
        let mut f = Fixture::new(&["a/DSC00001.ARW", "a/DSC00001.HIF"]);
        fs::write(f.root.join("a/DSC00001.xmp"), LIGHTROOM).unwrap();
        assert_eq!(f.sync(), 1);
        let photo = f.photo("dsc00001");
        assert_eq!(
            get(&f.cat, photo).unwrap(),
            PhotoMeta {
                rating: 3,
                label: Some("red".into()),
                ..Default::default()
            }
        );

        f.set(
            photo,
            Change {
                rating: Some(5),
                ..Default::default()
            },
        )
        .unwrap();
        let out = f.read("a/DSC00001.xmp");
        let diff: Vec<(&str, &str)> = LIGHTROOM
            .lines()
            .zip(out.lines())
            .filter(|(a, b)| a != b)
            .collect();
        assert_eq!(diff, [("   xmp:Rating=\"3\"", "   xmp:Rating=\"5\"")]);
        assert_eq!(out.lines().count(), LIGHTROOM.lines().count());

        // Our own write doesn't count as an outside change on the next scan.
        assert_eq!(f.sync(), 0);
    }

    #[test]
    fn outside_edits_are_picked_up_and_kept() {
        let mut f = Fixture::new(&["a/DSC00001.ARW", "a/DSC00001.HIF"]);
        let photo = f.photo("dsc00001");
        f.set(
            photo,
            Change {
                rating: Some(2),
                ..Default::default()
            },
        )
        .unwrap();

        // Lightroom adds a label.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let edited = f.read("a/DSC00001.xmp").replace(
            "xmp:Rating=\"2\"",
            "xmp:Rating=\"2\"\n    xmp:Label=\"Purple\"",
        );
        fs::write(f.root.join("a/DSC00001.xmp"), edited).unwrap();
        assert_eq!(f.sync(), 1);
        assert_eq!(get(&f.cat, photo).unwrap().label.as_deref(), Some("purple"));

        // A later change in Pairview keeps it.
        f.set(
            photo,
            Change {
                flag: Some(Flag::Pick),
                ..Default::default()
            },
        )
        .unwrap();
        let x = xmp::read(&f.read("a/DSC00001.xmp"));
        assert_eq!(
            (x.rating, x.label.as_deref(), x.flag.as_deref()),
            (Some(2), Some("Purple"), Some("pick"))
        );
    }

    #[test]
    fn rrdata_ratings_carry_into_a_new_xmp() {
        let mut f = Fixture::new(&["a/DSC00001.ARW"]);
        let raw = f.root.join("a/DSC00001.ARW");
        update_sidecar(&get_primary_sidecar_path(&raw), &raw, |m| m.rating = 3).unwrap();
        f.sync();
        let photo = f.photo("dsc00001");
        assert_eq!(get(&f.cat, photo).unwrap().rating, 3);

        f.set(
            photo,
            Change {
                keep_raw: Some(KeepRaw::Keep),
                ..Default::default()
            },
        )
        .unwrap();
        let x = xmp::read(&f.read("a/DSC00001.xmp"));
        assert_eq!((x.rating, x.keep_raw.as_deref()), (Some(3), Some("keep")));
    }

    #[test]
    fn offline_location_changes_nothing() {
        let mut f = Fixture::new(&["a/DSC00001.ARW", "a/DSC00001.HIF"]);
        let photo = f.photo("dsc00001");
        let before = f.snapshot();
        let gone = f.root.parent().unwrap().join("unplugged");
        f.cat
            .conn()
            .execute("UPDATE locations SET path = ?1", [gone.to_str().unwrap()])
            .unwrap();
        let err = f
            .set(
                photo,
                Change {
                    rating: Some(5),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("offline"), "{err}");
        assert_eq!(f.snapshot(), before);
        assert_eq!(get(&f.cat, photo).unwrap(), PhotoMeta::default());
    }

    #[test]
    fn unreadable_xmp_is_left_alone() {
        let mut f = Fixture::new(&["a/DSC00001.ARW", "a/DSC00001.HIF"]);
        fs::write(f.root.join("a/DSC00001.xmp"), "garbage from some tool").unwrap();
        f.sync();
        let before = f.snapshot();
        let photo = f.photo("dsc00001");
        assert!(
            f.set(
                photo,
                Change {
                    rating: Some(5),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert_eq!(f.snapshot(), before);
    }

    #[test]
    fn guard_split_display_copy_gets_its_own_xmp() {
        let mut f = Fixture::new(&["a/DSC00001.ARW", "a/DSC00001.JPG"]);
        for (name, date) in [
            ("DSC00001.ARW", "2026-10-31T10:00:00"),
            ("DSC00001.JPG", "2026-11-01T10:00:00"),
        ] {
            f.cat
                .conn()
                .execute(
                    "INSERT INTO exif (file_id, file_mtime, date_original, read_at)
                     SELECT id, mtime, ?2, 0 FROM files WHERE name = ?1",
                    (name, date),
                )
                .unwrap();
        }
        let shoot: i64 = f
            .cat
            .conn()
            .query_row("SELECT shoot_folder_id FROM photos", [], |r| r.get(0))
            .unwrap();
        assets::refresh(&mut f.cat, &[shoot]).unwrap();
        let display: i64 = f
            .cat
            .conn()
            .query_row(
                "SELECT id FROM photos WHERE state = 'display_only'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let raw: i64 = f
            .cat
            .conn()
            .query_row("SELECT id FROM photos WHERE state = 'raw_only'", [], |r| {
                r.get(0)
            })
            .unwrap();

        f.set(
            display,
            Change {
                rating: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        f.set(
            raw,
            Change {
                rating: Some(4),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(xmp::read(&f.read("a/DSC00001.JPG.xmp")).rating, Some(1));
        assert_eq!(xmp::read(&f.read("a/DSC00001.xmp")).rating, Some(4));
        assert_eq!(get(&f.cat, display).unwrap().rating, 1);
        assert_eq!(get(&f.cat, raw).unwrap().rating, 4);
    }

    #[test]
    fn bad_values_are_rejected_before_anything_is_written() {
        let mut f = Fixture::new(&["a/DSC00001.ARW"]);
        let before = f.snapshot();
        let photo = f.photo("dsc00001");
        assert!(
            set(
                &mut f.cat,
                &[photo],
                &Change {
                    rating: Some(6),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert!(
            set(
                &mut f.cat,
                &[photo],
                &Change {
                    label: Some(Some("orange".into())),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert_eq!(f.snapshot(), before);
    }

    #[test]
    fn one_unreadable_sidecar_does_not_stop_the_sync() {
        use std::os::unix::fs::PermissionsExt;
        let mut f = Fixture::new(&["a/DSC00001.ARW", "a/DSC00002.ARW"]);
        for (stem, rating) in [("DSC00001", 1), ("DSC00002", 2)] {
            let xmp = LIGHTROOM.replace("xmp:Rating=\"3\"", &format!("xmp:Rating=\"{rating}\""));
            fs::write(f.root.join(format!("a/{stem}.xmp")), xmp).unwrap();
        }
        let locked = f.root.join("a/DSC00001.xmp");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let synced = f.sync();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(synced, 1);
        assert_eq!(get(&f.cat, f.photo("dsc00002")).unwrap().rating, 2);
        // Readable again: picked up on the next sync.
        assert_eq!(f.sync(), 1);
        assert_eq!(get(&f.cat, f.photo("dsc00001")).unwrap().rating, 1);
    }

    #[test]
    fn reading_a_corrupt_rrdata_changes_nothing() {
        let mut f = Fixture::new(&["a/DSC00001.ARW"]);
        fs::write(f.root.join("a/DSC00001.ARW.rrdata"), "{ not json").unwrap();
        let before = f.snapshot();
        f.sync();
        assert_eq!(f.snapshot(), before);
        assert_eq!(
            get(&f.cat, f.photo("dsc00001")).unwrap(),
            PhotoMeta::default()
        );
    }

    #[test]
    fn sidecar_targets_are_checked() {
        assert!(ensure_sidecar_path(Path::new("/x/DSC00001.xmp"), "xmp").is_ok());
        assert!(ensure_sidecar_path(Path::new("/x/DSC00001.ARW.rrdata"), "rrdata").is_ok());
        assert!(ensure_sidecar_path(Path::new("/x/DSC00001.ARW"), "xmp").is_err());
        assert!(ensure_sidecar_path(Path::new("/x/DSC00001.HIF"), "rrdata").is_err());
        assert!(write_sidecar(Path::new("/tmp/pv-never.ARW"), "xmp", b"x").is_err());
    }
}
