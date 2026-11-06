//! Which files a Photo-level delete moves (PRD §6.6: Delete photo · Keep HIF only · Keep RAW only).
//!
//! A plan is computed from the catalog and changes nothing, so the delete dialog can show what
//! will go and how much space it frees before the owner confirms. `lifecycle::apply` re-checks
//! every file on disk before moving anything.
//!
//! Pairs are only what the pairing engine made a Photo: a RAW and a display copy split by the
//! timestamp guard are two Photos, so a delete of one never reaches the other.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use serde::Serialize;

use super::manifest::{ManifestPhoto, Reason};
use crate::pv::catalog::Catalog;
use crate::pv::locations::Location;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// Every file of the Photo: RAW, display copies and sidecars.
    Photo,
    /// Keep HIF only: drop the RAW (and its sidecars). `Slimmed` in Recently Deleted.
    KeepDisplay,
    /// Keep RAW only: drop the display copies (and their sidecars).
    KeepRaw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkipReason {
    /// Not in the catalog (any more).
    Gone,
    /// Keep HIF only / Keep RAW only on a Photo without a RAW.
    NoRaw,
    /// Keep HIF only / Keep RAW only on a Photo without a display copy.
    NoDisplay,
    /// Keep HIF only on a Photo with the Keep-RAW lock (`K`).
    Locked,
    /// Keep HIF only on an edited Photo whose recipe is newer than its rendered display copy:
    /// render it first, or the edit is lost (PRD §6.5).
    NeedsRender,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub photo_id: i64,
    pub reason: SkipReason,
}

/// One file to move.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedFile {
    pub file_id: Option<i64>,
    /// Relative to the location root.
    pub rel: String,
    pub size: u64,
    pub role: String,
    /// Index into `OpPlan::photos`.
    pub photo: Option<usize>,
}

/// Everything for one location: one operation, one Recently Deleted folder.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpPlan {
    pub location_id: i64,
    pub kind: &'static str,
    pub reason: Reason,
    pub files: Vec<PlannedFile>,
    pub photos: Vec<ManifestPhoto>,
    /// Photo-level XMPs copied next to the copy that stays, before the folder holding the
    /// original goes (`arw/DSC0001.xmp` → `hif/DSC0001.xmp`), so ratings survive. Relative paths.
    pub xmp_copies: Vec<(String, String)>,
    /// Shoot folders to re-pair afterwards.
    pub shoots: Vec<i64>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub ops: Vec<OpPlan>,
    pub skipped: Vec<Skipped>,
}

impl Plan {
    pub fn bytes(&self) -> u64 {
        self.ops.iter().flat_map(|o| &o.files).map(|f| f.size).sum()
    }

    pub fn photos(&self) -> usize {
        self.ops.iter().map(|o| o.photos.len()).sum()
    }
}

struct Member {
    file_id: i64,
    role: String,
    folder: String,
    name: String,
    size: u64,
    mtime: i64,
}

impl Member {
    fn rel(&self) -> String {
        join(&self.folder, &self.name)
    }

    fn is_primary(&self) -> bool {
        self.role != "sidecar"
    }
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_string()
    } else {
        format!("{folder}/{name}")
    }
}

/// The primary file a sidecar belongs to: `DSC0001.ARW.rrdata` (and virtual copies) and
/// `DSC0001.ARW.xmp` belong to `DSC0001.ARW` in the same folder. A bare-stem `DSC0001.xmp`
/// describes the whole Photo and has no single owner.
fn owner<'a>(sidecar: &Member, primaries: &[&'a Member]) -> Option<&'a Member> {
    let lower = sidecar.name.to_lowercase();
    let target = if lower.ends_with(".rrdata") {
        crate::file_management::parse_sidecar_name(&sidecar.name)
            .map(|(source, _)| source.to_lowercase())
    } else {
        lower.strip_suffix(".xmp").map(str::to_string)
    }?;
    primaries
        .iter()
        .find(|p| p.folder == sidecar.folder && p.name.to_lowercase() == target)
        .copied()
}

fn is_xmp(m: &Member) -> bool {
    m.name.to_lowercase().ends_with(".xmp")
}

/// Brings the catalog up to date with sidecars written since the last scan, for the folders of
/// `photo_ids`: a rating or an edit writes `.xmp`/`.rrdata` straight away, while the catalog hears
/// of them at the next scan. Call right before a real delete, so "rate, then delete" takes them
/// along. One folder listing each; offline locations are skipped (the delete is refused anyway).
pub fn sync_sidecars(cat: &Catalog, photo_ids: &[i64]) -> Result<usize> {
    let mut folders: Vec<(i64, i64, String)> = Vec::new();
    {
        let mut stmt = cat.conn().prepare_cached(
            "SELECT DISTINCT d.id, d.location_id, d.rel_path
             FROM photo_members m JOIN files f ON f.id = m.file_id JOIN folders d ON d.id = f.folder_id
             WHERE m.photo_id = ?1",
        )?;
        for &id in photo_ids {
            for row in stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))? {
                let row: (i64, i64, String) = row?;
                if !folders.contains(&row) {
                    folders.push(row);
                }
            }
        }
    }
    let mounts = crate::pv::locations::mounts();
    let mut online: BTreeMap<i64, Option<PathBuf>> = BTreeMap::new();
    let mut added = 0;
    let mut changed: Vec<i64> = Vec::new();
    for (folder_id, location_id, rel) in folders {
        let root = match online.get(&location_id) {
            Some(root) => root.clone(),
            None => {
                let root = crate::pv::locations::get(cat, location_id)?
                    .filter(|l| crate::pv::locations::status(l, &mounts).is_online())
                    .map(|l| l.path);
                online.insert(location_id, root.clone());
                root
            }
        };
        let Some(root) = root else {
            continue;
        };
        let dir = if rel.is_empty() {
            root.clone()
        } else {
            root.join(&rel)
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let (stem, ext) = crate::pv::scan::l0::split_name(&name);
            if name.starts_with('.') || crate::pv::scan::l0::file_kind(ext) != Some("sidecar") {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let inserted = cat.conn().execute(
                "INSERT OR IGNORE INTO files (folder_id, name, stem_key, ext, kind, size, mtime)
                 VALUES (?1, ?2, ?3, ?4, 'sidecar', ?5, ?6)",
                (
                    folder_id,
                    &name,
                    stem.to_lowercase(),
                    ext.to_ascii_lowercase(),
                    meta.len() as i64,
                    super::mtime_ms(&meta),
                ),
            )?;
            if inserted > 0 {
                added += 1;
                if !changed.contains(&folder_id) {
                    changed.push(folder_id);
                }
            }
        }
    }
    if !changed.is_empty() {
        let tx = cat.conn().unchecked_transaction()?;
        let mut shoots = Vec::new();
        for folder in changed {
            if let Some(shoot) = crate::pv::assets::shoot_of(&tx, folder)?
                && !shoots.contains(&shoot)
            {
                shoots.push(shoot);
            }
        }
        for shoot in shoots {
            crate::pv::assets::rebuild_shoot(&tx, shoot)?;
        }
        tx.commit()?;
    }
    Ok(added)
}

/// Plans a delete of `photo_ids`. Photos the scope doesn't apply to are listed in `skipped`.
pub fn plan_photos(cat: &Catalog, photo_ids: &[i64], scope: Scope) -> Result<Plan> {
    let mut b = Builder::default();
    let mut seen = HashSet::new();
    for &photo_id in photo_ids.iter().filter(|&&id| seen.insert(id)) {
        b.add_photo(cat, photo_id, How::Scope(scope))?;
    }
    Ok(b.finish())
}

/// The delete dialog's plan: `scope` for RAW+display Photos; a Photo the scope can't apply to
/// (no RAW, or no display copy) is deleted whole, since deleting it was asked for. Locked Photos
/// are skipped for Keep HIF only.
pub fn plan_delete(cat: &Catalog, photo_ids: &[i64], scope: Scope) -> Result<Plan> {
    let mut b = Builder::default();
    let mut seen = HashSet::new();
    for &photo_id in photo_ids.iter().filter(|&&id| seen.insert(id)) {
        b.add_photo(cat, photo_id, How::Scope(scope))?;
        if let Some(last) = b.skipped.last()
            && last.photo_id == photo_id
            && matches!(last.reason, SkipReason::NoRaw | SkipReason::NoDisplay)
        {
            b.skipped.pop();
            b.add_photo(cat, photo_id, How::Scope(Scope::Photo))?;
        }
    }
    Ok(b.finish())
}

/// Photo ids for grid paths (representative or display file). `None` when any path isn't a
/// catalogued Photo, so the caller can leave the whole request to upstream.
pub fn photo_ids_for_paths(cat: &Catalog, paths: &[PathBuf]) -> Result<Option<Vec<i64>>> {
    let locations = crate::pv::locations::list(cat)?;
    let mut ids = Vec::new();
    for path in paths {
        let Some((location, rel)) = locate(&locations, path) else {
            return Ok(None);
        };
        match file_row(cat, location.id, &rel)? {
            Some((_, Some(photo))) => ids.push(photo),
            _ => return Ok(None),
        }
    }
    Ok(Some(ids))
}

/// Plans a delete of exact files (absolute paths), as RapidRAW's "delete selected file" asks.
/// Per Photo, the files asked for decide what it means: every main file → Delete photo; its
/// RAW(s) while a display copy stays → Keep HIF only (lock honoured, XMP kept); its display
/// copies while the RAW stays → Keep RAW only; anything else → those files and their own
/// sidecars. Files the catalog doesn't know yet go as they are. Paths outside every location
/// are returned untouched.
pub fn plan_paths(cat: &Catalog, paths: &[PathBuf]) -> Result<(Plan, Vec<PathBuf>)> {
    plan_paths_with(cat, paths, false)
}

/// `plan_paths`, or with `whole_photos` every Photo touched goes whole (RapidRAW's "delete
/// with associated files", which matched by stem in one folder only).
pub fn plan_paths_with(
    cat: &Catalog,
    paths: &[PathBuf],
    whole_photos: bool,
) -> Result<(Plan, Vec<PathBuf>)> {
    let locations = crate::pv::locations::list(cat)?;
    let mut b = Builder::default();
    let mut outside = Vec::new();
    let mut by_photo: BTreeMap<i64, HashSet<i64>> = BTreeMap::new();
    let mut seen = HashSet::new();
    for path in paths.iter().filter(|p| seen.insert((*p).clone())) {
        let Some((location, rel)) = locate(&locations, path) else {
            outside.push(path.clone());
            continue;
        };
        match file_row(cat, location.id, &rel)? {
            Some((file_id, Some(photo_id))) => {
                by_photo.entry(photo_id).or_default().insert(file_id);
            }
            Some((file_id, None)) => b.add_file(location.id, &rel, Some(file_id), path)?,
            None => b.add_file(location.id, &rel, None, path)?,
        }
    }
    // These plans are for real deletes (RapidRAW's commands), not previews.
    sync_sidecars(cat, &by_photo.keys().copied().collect::<Vec<_>>())?;
    for (photo_id, files) in by_photo {
        let how = if whole_photos {
            How::Scope(Scope::Photo)
        } else {
            How::Files(files)
        };
        b.add_photo(cat, photo_id, how)?;
    }
    Ok((b.finish(), outside))
}

/// Plans a delete of a folder inside a location: every file under it, as one operation. The
/// location root and Recently Deleted itself are refused. `None` when `folder` is outside every
/// location.
pub fn plan_folder(cat: &Catalog, folder: &Path) -> Result<Option<Plan>> {
    let locations = crate::pv::locations::list(cat)?;
    let Some((location, rel)) = locate(&locations, folder) else {
        return Ok(None);
    };
    anyhow::ensure!(
        !rel.is_empty(),
        "{} is a whole library location; remove it in Settings instead",
        folder.display()
    );
    anyhow::ensure!(
        rel.split('/').next() != Some(super::TRASH_DIR),
        "use Recently Deleted to empty Recently Deleted"
    );
    let mut b = Builder::default();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).with_context(|| format!("cannot read {}", dir.display()))?;
        for entry in entries {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                let rel = format!(
                    "{rel}/{}",
                    path.strip_prefix(folder)?
                        .to_string_lossy()
                        .replace('\\', "/")
                );
                let file = file_row(cat, location.id, &rel)?;
                b.add_folder_file(cat, location.id, &rel, file, &path)?;
            }
            // Symlinks are left where they are (the folder then stays too).
        }
    }
    Ok(Some(b.finish()))
}

/// The paths that are in no location.
pub fn outside_locations(cat: &Catalog, paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let locations = crate::pv::locations::list(cat)?;
    Ok(paths
        .iter()
        .filter(|p| locate(&locations, p).is_none())
        .cloned()
        .collect())
}

/// The location `path` is in, and its `/`-separated path relative to the location root.
fn locate<'a>(locations: &'a [Location], path: &Path) -> Option<(&'a Location, String)> {
    locations.iter().find_map(|l| {
        let rel = path.strip_prefix(&l.path).ok()?;
        Some((l, rel.to_string_lossy().replace('\\', "/")))
    })
}

/// `(file id, Photo id)` of a catalog file.
fn file_row(cat: &Catalog, location_id: i64, rel: &str) -> Result<Option<(i64, Option<i64>)>> {
    let (folder, name) = rel.rsplit_once('/').unwrap_or(("", rel));
    Ok(cat
        .conn()
        .query_row(
            "SELECT f.id, m.photo_id FROM files f JOIN folders d ON d.id = f.folder_id
             LEFT JOIN photo_members m ON m.file_id = f.id
             WHERE d.location_id = ?1 AND d.rel_path = ?2 AND f.name = ?3",
            (location_id, folder, name),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

enum How {
    Scope(Scope),
    /// Exactly these member files (and the sidecars that belong to them).
    Files(HashSet<i64>),
}

#[derive(Default)]
struct Builder {
    ops: BTreeMap<(i64, &'static str), OpPlan>,
    skipped: Vec<Skipped>,
    /// Photo ids already in an op, so a folder delete lists each Photo once.
    photo_index: BTreeMap<(i64, &'static str, i64), usize>,
}

impl Builder {
    fn op(&mut self, location_id: i64, kind: &'static str) -> &mut OpPlan {
        let reason = if kind == "drop_raw" {
            Reason::Slimmed
        } else {
            Reason::Deleted
        };
        self.ops
            .entry((location_id, kind))
            .or_insert_with(|| OpPlan {
                location_id,
                kind,
                reason,
                files: Vec::new(),
                photos: Vec::new(),
                xmp_copies: Vec::new(),
                shoots: Vec::new(),
            })
    }

    fn finish(self) -> Plan {
        Plan {
            ops: self
                .ops
                .into_values()
                .filter(|o| !o.files.is_empty())
                .collect(),
            skipped: self.skipped,
        }
    }

    /// A file that isn't part of a Photo.
    fn add_file(
        &mut self,
        location_id: i64,
        rel: &str,
        file_id: Option<i64>,
        path: &Path,
    ) -> Result<()> {
        let size = std::fs::symlink_metadata(path)
            .map(|m| m.len())
            .unwrap_or(0);
        self.op(location_id, "delete").files.push(PlannedFile {
            file_id,
            rel: rel.to_string(),
            size,
            role: "file".into(),
            photo: None,
        });
        Ok(())
    }

    /// A file under a deleted folder: everything goes, grouped by Photo for the manifest.
    fn add_folder_file(
        &mut self,
        cat: &Catalog,
        location_id: i64,
        rel: &str,
        file: Option<(i64, Option<i64>)>,
        path: &Path,
    ) -> Result<()> {
        let Some((file_id, Some(photo_id))) = file else {
            return self.add_file(location_id, rel, file.map(|f| f.0), path);
        };
        let Some(info) = photo_info(cat, photo_id)? else {
            return self.add_file(location_id, rel, Some(file_id), path);
        };
        let role: String = cat.conn().query_row(
            "SELECT role FROM photo_members WHERE file_id = ?1",
            [file_id],
            |r| r.get(0),
        )?;
        let key = (location_id, "delete", photo_id);
        let index = match self.photo_index.get(&key) {
            Some(&i) => i,
            None => {
                let op = self.op(location_id, "delete");
                op.photos.push(info.manifest);
                if !op.shoots.contains(&info.shoot_id) {
                    op.shoots.push(info.shoot_id);
                }
                let i = op.photos.len() - 1;
                self.photo_index.insert(key, i);
                i
            }
        };
        let size = std::fs::symlink_metadata(path)
            .map(|m| m.len())
            .unwrap_or(0);
        self.op(location_id, "delete").files.push(PlannedFile {
            file_id: Some(file_id),
            rel: rel.to_string(),
            size,
            role,
            photo: Some(index),
        });
        Ok(())
    }

    fn add_photo(&mut self, cat: &Catalog, photo_id: i64, how: How) -> Result<()> {
        let Some(info) = photo_info(cat, photo_id)? else {
            self.skipped.push(Skipped {
                photo_id,
                reason: SkipReason::Gone,
            });
            return Ok(());
        };
        let members = members(cat, photo_id)?;
        let primaries: Vec<&Member> = members.iter().filter(|m| m.is_primary()).collect();

        // Exact files that amount to one of the three choices are treated as that choice.
        let how = match how {
            How::Files(files) => {
                let asked = |m: &&Member| files.contains(&m.file_id);
                let all = |pred: &dyn Fn(&Member) -> bool| {
                    let group: Vec<_> = primaries.iter().filter(|m| pred(m)).collect();
                    !group.is_empty()
                        && group.iter().all(|m| asked(m))
                        && primaries.iter().filter(|m| !pred(m)).all(|m| !asked(m))
                };
                if !primaries.is_empty() && primaries.iter().all(asked) {
                    How::Scope(Scope::Photo)
                } else if all(&|m| m.role == "raw") {
                    How::Scope(Scope::KeepDisplay)
                } else if all(&|m| m.role != "raw") {
                    How::Scope(Scope::KeepRaw)
                } else {
                    How::Files(files)
                }
            }
            how => how,
        };

        let has_raw = primaries.iter().any(|m| m.role == "raw");
        let has_display = primaries.iter().any(|m| m.role != "raw");
        let refusal = match &how {
            How::Scope(Scope::KeepDisplay) if !has_raw => Some(SkipReason::NoRaw),
            How::Scope(Scope::KeepDisplay) if !has_display => Some(SkipReason::NoDisplay),
            How::Scope(Scope::KeepDisplay) if info.keep_raw == "keep" => Some(SkipReason::Locked),
            How::Scope(Scope::KeepDisplay) if needs_render(&info, &members, &primaries) => {
                Some(SkipReason::NeedsRender)
            }
            How::Scope(Scope::KeepRaw) if !has_raw => Some(SkipReason::NoRaw),
            How::Scope(Scope::KeepRaw) if !has_display => Some(SkipReason::NoDisplay),
            _ => None,
        };
        if let Some(reason) = refusal {
            self.skipped.push(Skipped { photo_id, reason });
            return Ok(());
        }

        // Which main files go.
        let drops = |m: &Member| match &how {
            How::Scope(Scope::Photo) => true,
            How::Scope(Scope::KeepDisplay) => m.role == "raw",
            How::Scope(Scope::KeepRaw) => m.role != "raw",
            How::Files(files) => files.contains(&m.file_id),
        };
        let kept: Vec<&Member> = primaries.iter().copied().filter(|m| !drops(m)).collect();
        let whole = kept.is_empty();
        let kept_folder = |folder: &str| kept.iter().any(|m| m.folder == folder);
        let mut going: Vec<&Member> = Vec::new();
        let mut photo_xmps_going: Vec<&Member> = Vec::new();
        for m in &members {
            let goes = if m.is_primary() {
                drops(m)
            } else if matches!(&how, How::Files(f) if f.contains(&m.file_id)) {
                true // A virtual copy's sidecar, asked for by itself.
            } else {
                match owner(m, &primaries) {
                    Some(o) => drops(o),
                    // A Photo-level XMP stays while a kept copy shares its folder.
                    None if is_xmp(m) => {
                        let goes = !kept_folder(&m.folder);
                        if goes {
                            photo_xmps_going.push(m);
                        }
                        goes
                    }
                    // An unknown sidecar goes only with the whole Photo.
                    None => whole,
                }
            };
            if goes {
                going.push(m);
            }
        }
        if going.is_empty() {
            return Ok(());
        }

        let kind = if matches!(how, How::Scope(Scope::KeepDisplay)) {
            "drop_raw"
        } else {
            "delete"
        };
        let op = self.op(info.location_id, kind);

        // Ratings live in the Photo-level XMP: keep one next to the copy that stays.
        if let (Some(xmp), Some(stay)) = (photo_xmps_going.first(), kept.first()) {
            let stays_has_xmp = members.iter().any(|m| {
                !m.is_primary()
                    && is_xmp(m)
                    && owner(m, &primaries).is_none()
                    && m.folder == stay.folder
            });
            if !stays_has_xmp {
                let (stem, _) = crate::pv::scan::l0::split_name(&stay.name);
                op.xmp_copies
                    .push((xmp.rel(), join(&stay.folder, &format!("{stem}.xmp"))));
            }
        }

        let index = op.photos.len();
        op.photos.push(info.manifest);
        for m in going {
            op.files.push(PlannedFile {
                file_id: Some(m.file_id),
                rel: m.rel(),
                size: m.size,
                role: m.role.clone(),
                photo: Some(index),
            });
        }
        if !op.shoots.contains(&info.shoot_id) {
            op.shoots.push(info.shoot_id);
        }
        Ok(())
    }
}

/// An edited Photo whose RAW recipe (`.rrdata`) changed after its display copy was rendered.
fn needs_render(info: &PhotoInfo, members: &[Member], primaries: &[&Member]) -> bool {
    if info.state != "edited" {
        return false;
    }
    let rendered = primaries
        .iter()
        .filter(|m| m.role == "display")
        .map(|m| m.mtime)
        .max()
        .unwrap_or(i64::MAX);
    members.iter().any(|m| {
        m.name.to_lowercase().ends_with(".rrdata")
            && owner(m, primaries).is_some_and(|o| o.role == "raw")
            && m.mtime > rendered
    })
}

struct PhotoInfo {
    location_id: i64,
    shoot_id: i64,
    keep_raw: String,
    state: String,
    manifest: ManifestPhoto,
}

fn photo_info(cat: &Catalog, photo_id: i64) -> Result<Option<PhotoInfo>> {
    Ok(cat
        .conn()
        .query_row(
            "SELECT d.location_id, p.shoot_folder_id, d.rel_path, p.stem_key, p.captured_at,
                    COALESCE(md.keep_raw, 'auto'), COALESCE(md.rating, 0), p.state
             FROM photos p JOIN folders d ON d.id = p.shoot_folder_id
             LEFT JOIN metadata md ON md.photo_id = p.id
             WHERE p.id = ?1",
            [photo_id],
            |r| {
                Ok(PhotoInfo {
                    location_id: r.get(0)?,
                    shoot_id: r.get(1)?,
                    keep_raw: r.get(5)?,
                    state: r.get(7)?,
                    manifest: ManifestPhoto {
                        shoot: r.get(2)?,
                        stem: r.get(3)?,
                        captured_at: r.get(4)?,
                        rating: r.get(6)?,
                    },
                })
            },
        )
        .optional()?)
}

fn members(cat: &Catalog, photo_id: i64) -> Result<Vec<Member>> {
    let mut stmt = cat.conn().prepare_cached(
        "SELECT f.id, m.role, d.rel_path, f.name, f.size, f.mtime
         FROM photo_members m JOIN files f ON f.id = m.file_id JOIN folders d ON d.id = f.folder_id
         WHERE m.photo_id = ?1
         ORDER BY CASE m.role WHEN 'raw' THEN 0 WHEN 'display' THEN 1 WHEN 'extra' THEN 2 ELSE 3 END,
                  d.rel_path, f.name",
    )?;
    let rows = stmt
        .query_map([photo_id], |r| {
            Ok(Member {
                file_id: r.get(0)?,
                role: r.get(1)?,
                folder: r.get(2)?,
                name: r.get(3)?,
                size: r.get::<_, i64>(4)?.max(0) as u64,
                mtime: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}
