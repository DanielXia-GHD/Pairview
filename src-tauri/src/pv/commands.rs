//! Tauri commands for the Pairview frontend (docs/IMPLEMENTATION_PLAN.md M1-09). Typed bindings:
//! `src/pv/api.ts`.
//!
//! The catalog lives in the app-data dir. Reads share one connection; writes (metadata, location
//! changes) open their own so a slow NAS write never holds up the grid. Each location has a
//! watcher thread that keeps the catalog current and emits `pv://folder-changed`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::pv::catalog::{self, Catalog};
use crate::pv::lifecycle;
use crate::pv::locations::{self, Location, LocationSettings, Status, Suggestion};
use crate::pv::metadata::{self, Change, Flag, KeepRaw, PhotoMeta};
use crate::pv::queries::{self, DayCount, Filter, FolderNode, PhotoDetail, PhotoItem, Scope, Sort};
use crate::pv::scan::watch::{self, FOLDER_CHANGED_EVENT, LocationWatcher, WatchOptions};

pub struct PvState {
    catalog_path: PathBuf,
    reader: Mutex<Catalog>,
    watchers: Mutex<HashMap<i64, LocationWatcher>>,
}

impl PvState {
    pub(crate) fn writer(&self) -> Result<Catalog> {
        Catalog::open(&self.catalog_path)
    }

    pub(crate) fn read<T>(&self, f: impl FnOnce(&Catalog) -> Result<T>) -> Result<T> {
        let cat = self
            .reader
            .lock()
            .map_err(|_| anyhow!("catalog lock poisoned"))?;
        f(&cat)
    }
}

/// Opens the catalog and starts a watcher per location. Called once from the app's setup.
pub fn setup(app: &AppHandle) -> Result<()> {
    crate::pv::looks::remember_look_dirs(app);
    let dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&dir)?;
    let catalog_path = dir.join(catalog::FILE_NAME);
    let mut reader = Catalog::open(&catalog_path)?;
    // Operations a crash left half done are put back before anything else touches files.
    match crate::pv::lifecycle::recover(&mut reader) {
        Ok(r) if r.rolled_back + r.failed + r.waiting > 0 => {
            log::warn!("Pairview: op-log replay: {r:?}")
        }
        Ok(_) => {}
        Err(e) => log::error!("Pairview: op-log replay failed: {e:#}"),
    }
    let locations = locations::list(&reader)?;
    app.manage(PvState {
        catalog_path,
        reader: Mutex::new(reader),
        watchers: Mutex::new(HashMap::new()),
    });
    let state = app.state::<PvState>();
    purge_expired_in_background(state.catalog_path.clone());
    for location in locations {
        if let Err(e) = start_watcher(app, &state, &location) {
            log::warn!("Pairview: cannot watch {}: {e:#}", location.path.display());
        }
    }
    Ok(())
}

fn start_watcher(app: &AppHandle, state: &PvState, location: &Location) -> Result<()> {
    let emitter = app.clone();
    let watcher = watch::watch(
        &state.catalog_path,
        location,
        WatchOptions::for_location(location),
        move |change| {
            let _ = emitter.emit(FOLDER_CHANGED_EVENT, change);
        },
    )?;
    // Replacing an old watcher drops (stops) it.
    state
        .watchers
        .lock()
        .map_err(|_| anyhow!("watcher lock poisoned"))?
        .insert(location.id, watcher);
    Ok(())
}

fn stop_watcher(state: &PvState, id: i64) {
    let watcher = state.watchers.lock().ok().and_then(|mut w| w.remove(&id));
    drop(watcher); // Joins the thread outside the lock.
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

// ---- Locations --------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationInfo {
    pub id: i64,
    pub name: String,
    pub path: PathBuf,
    pub status: Status,
    /// `smb://…` to offer Connect when the share isn't mounted.
    pub connect_url: Option<String>,
    pub photos: i64,
    pub sort_order: i64,
    pub last_scan_at: Option<i64>,
    pub settings: LocationSettings,
}

fn location_info(
    cat: &Catalog,
    location: Location,
    mounts: &[locations::Mount],
) -> Result<LocationInfo> {
    let photos = cat.conn().query_row(
        "SELECT COUNT(*) FROM photos p JOIN folders f ON f.id = p.shoot_folder_id
         WHERE f.location_id = ?1",
        [location.id],
        |r| r.get(0),
    )?;
    Ok(LocationInfo {
        status: locations::status(&location, mounts),
        settings: LocationSettings::of(&location),
        connect_url: location
            .volume_id
            .as_deref()
            .and_then(locations::connect_url)
            .map(str::to_string),
        id: location.id,
        name: location.name,
        path: location.path,
        photos,
        sort_order: location.sort_order,
        last_scan_at: location.last_scan_at,
    })
}

fn list_locations(cat: &Catalog) -> Result<Vec<LocationInfo>> {
    let mounts = locations::mounts();
    locations::list(cat)?
        .into_iter()
        .map(|l| location_info(cat, l, &mounts))
        .collect()
}

#[tauri::command(async)]
pub fn pv_locations_list(state: State<PvState>) -> Result<Vec<LocationInfo>, String> {
    state.read(list_locations).map_err(err)
}

#[tauri::command(async)]
pub fn pv_locations_add(
    app: AppHandle,
    state: State<PvState>,
    path: String,
    name: Option<String>,
) -> Result<LocationInfo, String> {
    (|| {
        let cat = state.writer()?;
        let id = locations::add(&cat, Path::new(&path), name.as_deref())?;
        let location = locations::get(&cat, id)?.context("location vanished")?;
        start_watcher(&app, &state, &location)?;
        location_info(&cat, location, &locations::mounts())
    })()
    .map_err(err)
}

/// Forgets a location and its cached data. Files on disk are not touched.
#[tauri::command(async)]
pub fn pv_locations_remove(state: State<PvState>, id: i64) -> Result<(), String> {
    stop_watcher(&state, id);
    state
        .writer()
        .and_then(|cat| locations::remove(&cat, id))
        .map_err(err)
}

#[tauri::command(async)]
pub fn pv_locations_rename(state: State<PvState>, id: i64, name: String) -> Result<(), String> {
    state
        .writer()
        .and_then(|cat| locations::rename(&cat, id, &name))
        .map_err(err)
}

#[tauri::command(async)]
pub fn pv_locations_reorder(state: State<PvState>, ids: Vec<i64>) -> Result<(), String> {
    state
        .writer()
        .and_then(|mut cat| locations::reorder(&mut cat, &ids))
        .map_err(err)
}

#[tauri::command(async)]
pub fn pv_locations_suggest_relocation(
    state: State<PvState>,
    id: i64,
) -> Result<Vec<Suggestion>, String> {
    state
        .read(|cat| {
            let location = locations::get(cat, id)?.context("no such location")?;
            Ok(locations::suggest_relocation(
                &location,
                &locations::mounts(),
            ))
        })
        .map_err(err)
}

#[tauri::command(async)]
pub fn pv_locations_relocate(
    app: AppHandle,
    state: State<PvState>,
    id: i64,
    path: String,
) -> Result<LocationInfo, String> {
    (|| {
        stop_watcher(&state, id);
        let cat = state.writer()?;
        locations::relocate(&cat, id, Path::new(&path))?;
        let location = locations::get(&cat, id)?.context("location vanished")?;
        start_watcher(&app, &state, &location)?;
        location_info(&cat, location, &locations::mounts())
    })()
    .map_err(err)
}

/// Syncs a location now (restarting its watcher runs a sync immediately).
#[tauri::command(async)]
pub fn pv_locations_rescan(app: AppHandle, state: State<PvState>, id: i64) -> Result<(), String> {
    (|| {
        let location = state
            .read(|cat| locations::get(cat, id))?
            .context("no such location")?;
        stop_watcher(&state, id);
        start_watcher(&app, &state, &location)
    })()
    .map_err(err)
}

#[tauri::command(async)]
pub fn pv_locations_update_settings(
    app: AppHandle,
    state: State<PvState>,
    id: i64,
    settings: LocationSettings,
) -> Result<LocationInfo, String> {
    (|| {
        let cat = state.writer()?;
        locations::update_settings(&cat, id, &settings)?;
        let location = locations::get(&cat, id)?.context("location vanished")?;
        // New excludes / interval take effect with a fresh watcher (and a sync now).
        stop_watcher(&state, id);
        start_watcher(&app, &state, &location)?;
        location_info(&cat, location, &locations::mounts())
    })()
    .map_err(err)
}

/// Asks macOS to mount the location's network share (opens its `smb://` address, which comes
/// from the catalog, never from the frontend).
#[tauri::command(async)]
pub fn pv_locations_connect(state: State<PvState>, id: i64) -> Result<(), String> {
    (|| {
        let location = state
            .read(|cat| locations::get(cat, id))?
            .context("no such location")?;
        let url = location
            .volume_id
            .as_deref()
            .and_then(locations::connect_url)
            .context("this location is not a network share")?;
        open_url(url)
    })()
    .map_err(err)
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) -> Result<()> {
    let status = std::process::Command::new("/usr/bin/open")
        .arg(url)
        .status()?;
    anyhow::ensure!(status.success(), "could not open {url}");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn open_url(url: &str) -> Result<()> {
    anyhow::bail!("connecting to {url} is not supported on this platform yet")
}

// ---- Browsing ---------------------------------------------------------------------------------

#[tauri::command(async)]
pub fn pv_folder_tree(state: State<PvState>, location_id: i64) -> Result<Vec<FolderNode>, String> {
    state
        .read(|cat| queries::folder_tree(cat, location_id))
        .map_err(err)
}

#[tauri::command(async)]
pub fn pv_list_photos(
    state: State<PvState>,
    scope: Scope,
    sort: Option<Sort>,
    filter: Option<Filter>,
) -> Result<Vec<PhotoItem>, String> {
    state
        .read(|cat| {
            queries::list_photos(
                cat,
                &scope,
                &sort.unwrap_or_default(),
                &filter.unwrap_or_default(),
            )
        })
        .map_err(err)
}

#[tauri::command(async)]
pub fn pv_dates(state: State<PvState>, location_id: Option<i64>) -> Result<Vec<DayCount>, String> {
    state
        .read(|cat| queries::dates(cat, location_id))
        .map_err(err)
}

#[tauri::command(async)]
pub fn pv_photo_detail(
    state: State<PvState>,
    photo_id: i64,
) -> Result<Option<PhotoDetail>, String> {
    state
        .read(|cat| queries::photo_detail(cat, photo_id))
        .map_err(err)
}

/// RGB + luma histogram of the Photo as the viewer shows it (info overlay, M1-16).
#[tauri::command(async)]
pub fn pv_photo_histogram(
    state: State<PvState>,
    photo_id: i64,
) -> Result<crate::pv::viewer::Histogram, String> {
    crate::pv::viewer::histogram(&state, photo_id).map_err(err)
}

#[derive(Serialize)]
pub struct FolderListing {
    /// One upstream `ImageFile` per Photo (its representative file), so RapidRAW's grid,
    /// thumbnails and editor work on Photos.
    pub images: Vec<crate::file_management::ImageFile>,
    /// The same Photos with Pairview's details (state, badges, Keep-RAW), for the grid overlay.
    pub photos: Vec<PhotoItem>,
}

/// A folder's Photos for the library grid, or a date range's for a `Dates: …` path. `None` when
/// the folder isn't in a scanned location (or is a format sub-folder): the caller lists the disk
/// as before.
#[tauri::command(async)]
pub fn pv_list_images_for_path(
    app: AppHandle,
    state: State<PvState>,
    path: String,
    recursive: bool,
) -> Result<Option<FolderListing>, String> {
    (|| {
        let listing = state.read(|cat| {
            let scope = if path.starts_with(queries::DATES_PREFIX) {
                match queries::date_scope(&path) {
                    Some(scope) => scope,
                    None => return Ok(Some((Vec::new(), HashMap::new(), Vec::new()))),
                }
            } else {
                let Some(folder_id) = queries::listing_folder(cat, Path::new(&path))? else {
                    return Ok(None);
                };
                Scope::Folder {
                    folder_id,
                    recursive,
                }
            };
            let mut entries = Vec::new();
            let mut sidecars = HashMap::new();
            let photos = queries::list_photos(cat, &scope, &Sort::default(), &Filter::default())?;
            for item in &photos {
                let Some(file) = item
                    .representative_path
                    .clone()
                    .or(item.display_path.clone())
                else {
                    continue;
                };
                let name = file
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let copies = queries::rrdata_copies(cat, item.id, &name)?;
                if !copies.is_empty() {
                    sidecars.insert(file.clone(), copies);
                }
                entries.push((file, (item.modified / 1000).max(0) as u64));
            }
            Ok(Some((entries, sidecars, photos)))
        })?;
        // Reading RapidRAW's sidecars happens outside the catalog lock.
        Ok(listing.map(|(entries, sidecars, photos)| FolderListing {
            images: crate::file_management::image_files_for_paths(entries, sidecars, &app),
            photos,
        }))
    })()
    .map_err(err)
}

// ---- Metadata ---------------------------------------------------------------------------------

/// From the frontend: fields left out are not changed; `label: ""` clears the label.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChangeInput {
    pub rating: Option<u8>,
    pub flag: Option<Flag>,
    pub label: Option<String>,
    pub keep_raw: Option<KeepRaw>,
}

impl From<ChangeInput> for Change {
    fn from(input: ChangeInput) -> Self {
        Change {
            rating: input.rating,
            flag: input.flag,
            label: input.label.map(|l| (!l.is_empty()).then_some(l)),
            keep_raw: input.keep_raw,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetResult {
    pub photo_id: i64,
    pub meta: Option<PhotoMeta>,
    pub error: Option<String>,
}

fn set_metadata(
    cat: &mut Catalog,
    photo_ids: &[i64],
    change: ChangeInput,
) -> Result<Vec<SetResult>> {
    Ok(metadata::set(cat, photo_ids, &change.into())?
        .into_iter()
        .map(|(photo_id, result)| match result {
            Ok(meta) => SetResult {
                photo_id,
                meta: Some(meta),
                error: None,
            },
            Err(e) => SetResult {
                photo_id,
                meta: None,
                error: Some(err(e)),
            },
        })
        .collect())
}

/// Sets rating / flag / label / Keep-RAW on Photos (written to XMP + `.rrdata`, M1-08).
#[tauri::command(async)]
pub fn pv_set_metadata(
    state: State<PvState>,
    photo_ids: Vec<i64>,
    change: ChangeInput,
) -> Result<Vec<SetResult>, String> {
    state
        .writer()
        .and_then(|mut cat| set_metadata(&mut cat, &photo_ids, change))
        .map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pv::assets;
    use crate::pv::scan::l0::{ScanOptions, scan_location};
    use std::fs;

    #[test]
    fn change_input_from_frontend_json() {
        let c: Change = serde_json::from_str::<ChangeInput>(r#"{"rating":4}"#)
            .unwrap()
            .into();
        assert_eq!(
            c,
            Change {
                rating: Some(4),
                ..Default::default()
            }
        );
        let c: Change =
            serde_json::from_str::<ChangeInput>(r#"{"label":"","flag":"pick","keepRaw":"keep"}"#)
                .unwrap()
                .into();
        assert_eq!(
            c,
            Change {
                flag: Some(Flag::Pick),
                label: Some(None),
                keep_raw: Some(KeepRaw::Keep),
                ..Default::default()
            }
        );
        assert!(serde_json::from_str::<ChangeInput>(r#"{"flag":"maybe"}"#).is_err());
    }

    #[test]
    fn locations_and_metadata_through_the_command_layer() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("DCIM");
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(root.join("a/DSC00001.ARW"), b"raw").unwrap();
        fs::write(root.join("a/DSC00001.HIF"), b"hif").unwrap();
        let mut cat = Catalog::open(&tmp.path().join(catalog::FILE_NAME)).unwrap();
        let id = locations::add(&cat, &root, Some("Desk")).unwrap();
        let loc = locations::get(&cat, id).unwrap().unwrap();
        let scan = scan_location(&mut cat, &loc, &ScanOptions::default()).unwrap();
        assets::refresh(&mut cat, &scan.changed_folders).unwrap();

        let list = list_locations(&cat).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(
            (list[0].name.as_str(), list[0].status, list[0].photos),
            ("Desk", Status::Online, 1)
        );
        let json = serde_json::to_value(&list[0]).unwrap();
        assert_eq!(json["status"], "online");
        assert!(json["connectUrl"].is_null());

        let photo: i64 = cat
            .conn()
            .query_row("SELECT id FROM photos", [], |r| r.get(0))
            .unwrap();
        let results = set_metadata(
            &mut cat,
            &[photo, 999],
            ChangeInput {
                rating: Some(3),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results[0].meta.as_ref().map(|m| m.rating), Some(3));
        assert!(results[1].error.is_some());
        assert!(root.join("a/DSC00001.xmp").exists());
    }
}

// ---- Deleting (M2-03) -------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOption {
    pub photos: usize,
    pub files: usize,
    pub bytes: u64,
    /// Photos the Keep-RAW lock keeps out of Keep HIF only.
    pub locked: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletePreview {
    /// Photos with both a RAW and a display copy: the dialog's choices apply to these.
    pub pairs: usize,
    pub photo: DeleteOption,
    pub keep_display: DeleteOption,
    pub keep_raw: DeleteOption,
}

/// What each choice of the delete dialog would move to Recently Deleted. Changes nothing.
#[tauri::command]
pub fn pv_delete_preview(
    state: State<PvState>,
    photo_ids: Vec<i64>,
) -> Result<DeletePreview, String> {
    state
        .read(|cat| {
            let option = |scope| -> Result<DeleteOption> {
                let plan = lifecycle::plan_delete(cat, &photo_ids, scope)?;
                Ok(DeleteOption {
                    photos: plan.photos(),
                    files: plan.ops.iter().map(|o| o.files.len()).sum(),
                    bytes: plan.bytes(),
                    locked: plan
                        .skipped
                        .iter()
                        .filter(|s| s.reason == lifecycle::SkipReason::Locked)
                        .count(),
                })
            };
            let mut pairs = 0;
            for id in &photo_ids {
                let state: Option<String> = cat
                    .conn()
                    .query_row("SELECT state FROM photos WHERE id = ?1", [id], |r| r.get(0))
                    .optional()?;
                if matches!(state.as_deref(), Some("raw_display" | "edited")) {
                    pairs += 1;
                }
            }
            Ok(DeletePreview {
                pairs,
                photo: option(lifecycle::Scope::Photo)?,
                keep_display: option(lifecycle::Scope::KeepDisplay)?,
                keep_raw: option(lifecycle::Scope::KeepRaw)?,
            })
        })
        .map_err(err)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResult {
    pub photos: usize,
    pub files: usize,
    pub bytes: u64,
    /// Photos left alone (Keep-RAW lock).
    pub locked: usize,
}

/// The delete dialog's action on grid paths: moves to Recently Deleted (pv/lifecycle).
#[tauri::command]
pub fn pv_delete_photos(
    app: AppHandle,
    state: State<PvState>,
    paths: Vec<String>,
    scope: lifecycle::Scope,
) -> Result<DeleteResult, String> {
    let targets: Vec<PathBuf> = paths
        .iter()
        .map(|p| PathBuf::from(crate::pv::lifecycle::upstream::base_path(p)))
        .collect();
    let mut cat = state.writer().map_err(err)?;
    let ids = lifecycle::plan::photo_ids_for_paths(&cat, &targets)
        .map_err(err)?
        .ok_or_else(|| "some of these aren't Photos in a library location".to_string())?;
    lifecycle::plan::sync_sidecars(&cat, &ids).map_err(err)?;
    let plan = lifecycle::plan_delete(&cat, &ids, scope).map_err(err)?;
    let result = DeleteResult {
        photos: plan.photos(),
        files: plan.ops.iter().map(|o| o.files.len()).sum(),
        bytes: plan.bytes(),
        locked: plan.skipped.len(),
    };
    lifecycle::upstream::execute(&app, &mut cat, &plan)?;
    let deletions: std::collections::HashSet<String> = paths.into_iter().collect();
    crate::file_management::sync_album_path_changes(&app, None, Some(&deletions), None);
    Ok(result)
}

// ---- Recently Deleted (M2-04) -----------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedPhotoInfo {
    /// Index in the operation's manifest; the preview URL and restore use it.
    pub index: usize,
    pub stem: String,
    pub shoot: String,
    pub captured_at: Option<String>,
    pub rating: i64,
    /// File names still in Recently Deleted.
    pub files: Vec<String>,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedOpInfo {
    pub op_id: String,
    pub reason: lifecycle::Reason,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub bytes: u64,
    pub photos: Vec<DeletedPhotoInfo>,
    /// Files that weren't part of a Photo (folder deletes, unindexed files).
    pub loose_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedLocation {
    pub location_id: i64,
    pub name: String,
    pub status: Status,
    pub ops: Vec<DeletedOpInfo>,
}

fn deleted_op_info(op: lifecycle::DeletedOp) -> DeletedOpInfo {
    let m = op.manifest;
    let mut photos: Vec<DeletedPhotoInfo> = Vec::new();
    let mut loose_files = Vec::new();
    for f in &m.files {
        let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
        match f.photo.and_then(|i| m.photos.get(i).map(|p| (i, p))) {
            Some((index, p)) => match photos.iter_mut().find(|x| x.index == index) {
                Some(x) => {
                    x.files.push(name);
                    x.bytes += f.size;
                }
                None => photos.push(DeletedPhotoInfo {
                    index,
                    stem: p.stem.clone(),
                    shoot: p.shoot.clone(),
                    captured_at: p.captured_at.clone(),
                    rating: p.rating,
                    files: vec![name],
                    bytes: f.size,
                }),
            },
            None => loose_files.push(f.path.clone()),
        }
    }
    DeletedOpInfo {
        op_id: m.op_id.clone(),
        reason: m.reason,
        created_at: m.created_at,
        expires_at: op.expires_at,
        bytes: op.bytes,
        photos,
        loose_files,
    }
}

/// Recently Deleted of every location, newest operation first. Offline locations are listed
/// without operations.
#[tauri::command]
pub fn pv_recently_deleted(state: State<PvState>) -> Result<Vec<DeletedLocation>, String> {
    let mut cat = state.writer().map_err(err)?;
    let mounts = locations::mounts();
    let mut out = Vec::new();
    for location in locations::list(&cat).map_err(err)? {
        let status = locations::status(&location, &mounts);
        let ops = if status.is_online() {
            match lifecycle::list(&mut cat, location.id) {
                Ok(ops) => ops.into_iter().map(deleted_op_info).collect(),
                Err(e) => {
                    log::warn!("Pairview: Recently Deleted of {}: {e:#}", location.name);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        out.push(DeletedLocation {
            location_id: location.id,
            name: location.name,
            status,
            ops,
        });
    }
    Ok(out)
}

/// Puts an operation's files back: all of them, or only those of the Photos `photos` (manifest
/// indexes) and the loose files `files`.
#[tauri::command]
pub fn pv_restore_deleted(
    app: AppHandle,
    state: State<PvState>,
    location_id: i64,
    op_id: String,
    photos: Option<Vec<usize>>,
    files: Option<Vec<String>>,
) -> Result<lifecycle::Restored, String> {
    let mut cat = state.writer().map_err(err)?;
    let only: Option<Vec<String>> = if photos.is_none() && files.is_none() {
        None
    } else {
        let op = lifecycle::list(&mut cat, location_id)
            .map_err(err)?
            .into_iter()
            .find(|o| o.manifest.op_id == op_id)
            .ok_or_else(|| format!("{op_id} is no longer in Recently Deleted"))?;
        let photos = photos.unwrap_or_default();
        let files = files.unwrap_or_default();
        Some(
            op.manifest
                .files
                .iter()
                .filter(|f| f.photo.is_some_and(|i| photos.contains(&i)) || files.contains(&f.path))
                .map(|f| f.path.clone())
                .collect(),
        )
    };
    let restored =
        lifecycle::restore(&mut cat, location_id, &op_id, only.as_deref()).map_err(err)?;
    let _ = app.emit(
        FOLDER_CHANGED_EVENT,
        watch::FolderChanged {
            location_id,
            folders: Vec::new(),
            shoots: Vec::new(),
        },
    );
    Ok(restored)
}

/// Empty now: permanently deletes Recently Deleted of one location (or every online one),
/// limited to `op_ids` when given. Only files the manifests list are deleted.
#[tauri::command]
pub fn pv_empty_deleted(
    state: State<PvState>,
    location_id: Option<i64>,
    op_ids: Option<Vec<String>>,
) -> Result<lifecycle::Purged, String> {
    let mut cat = state.writer().map_err(err)?;
    let mounts = locations::mounts();
    let mut total = lifecycle::Purged::default();
    for location in locations::list(&cat).map_err(err)? {
        if location_id.is_some_and(|id| id != location.id)
            || !locations::status(&location, &mounts).is_online()
        {
            continue;
        }
        let p = lifecycle::purge(&mut cat, location.id, None, op_ids.as_deref()).map_err(err)?;
        total.ops += p.ops;
        total.files += p.files;
        total.bytes += p.bytes;
    }
    Ok(total)
}

/// The 30-day purge of every online location (PRD D12). Runs once at startup, in the background.
fn purge_expired_in_background(catalog_path: PathBuf) {
    std::thread::spawn(move || {
        let run = || -> Result<()> {
            let mut cat = Catalog::open(&catalog_path)?;
            let mounts = locations::mounts();
            for location in locations::list(&cat)? {
                if !locations::status(&location, &mounts).is_online() {
                    continue;
                }
                let p = lifecycle::purge_expired(&mut cat, location.id, catalog::now_ms())?;
                if p.ops > 0 {
                    log::info!(
                        "Pairview: purged {} file(s), {} bytes older than 30 days from {}",
                        p.files,
                        p.bytes,
                        location.name
                    );
                }
            }
            Ok(())
        };
        if let Err(e) = run() {
            log::warn!("Pairview: 30-day purge: {e:#}");
        }
    });
}

// ---- Orphans (M2-05) --------------------------------------------------------------------------

/// `⚠ RAW` Photos everywhere: RAWs whose display copy disappeared outside the app.
#[tauri::command]
pub fn pv_orphans(state: State<PvState>) -> Result<Vec<PhotoItem>, String> {
    state
        .read(|cat| {
            queries::list_photos(
                cat,
                &Scope::All { location_id: None },
                &Sort::default(),
                &Filter {
                    states: vec!["orphan_raw".into()],
                    ..Default::default()
                },
            )
        })
        .map_err(err)
}

/// Orphan review "Keep as RAW-only". Changes no files.
#[tauri::command]
pub fn pv_keep_raw_only(
    app: AppHandle,
    state: State<PvState>,
    photo_ids: Vec<i64>,
) -> Result<usize, String> {
    let cat = state.writer().map_err(err)?;
    let n = crate::pv::assets::mark_raw_only(&cat, &photo_ids).map_err(err)?;
    let mut locations: Vec<i64> = Vec::new();
    for id in &photo_ids {
        if let Ok(l) = cat.conn().query_row(
            "SELECT f.location_id FROM photos p JOIN folders f ON f.id = p.shoot_folder_id WHERE p.id = ?1",
            [id],
            |r| r.get::<_, i64>(0),
        ) && !locations.contains(&l)
        {
            locations.push(l);
        }
    }
    for location_id in locations {
        let _ = app.emit(
            FOLDER_CHANGED_EVENT,
            watch::FolderChanged {
                location_id,
                folders: Vec::new(),
                shoots: Vec::new(),
            },
        );
    }
    Ok(n)
}

// ---- Undo (M2-06) -----------------------------------------------------------------------------

/// Undoes the most recent delete still in Recently Deleted (the whole action). `None` when there
/// is nothing to undo.
#[tauri::command]
pub fn pv_undo(app: AppHandle, state: State<PvState>) -> Result<Option<lifecycle::Undone>, String> {
    let mut cat = state.writer().map_err(err)?;
    let undone = lifecycle::undo_last(&mut cat).map_err(err)?;
    for &location_id in undone.iter().flat_map(|u| &u.locations) {
        let _ = app.emit(
            FOLDER_CHANGED_EVENT,
            watch::FolderChanged {
                location_id,
                folders: Vec::new(),
                shoots: Vec::new(),
            },
        );
    }
    Ok(undone)
}

// ---- RAW value (M2-07) ------------------------------------------------------------------------

/// Keep RAW / Your call / HIF is enough, with reason chips. Computes missing image statistics
/// first (cheap previews, 8 at a time). Recommends only; changes no files.
#[tauri::command]
pub fn pv_raw_value(
    state: State<PvState>,
    photo_ids: Vec<i64>,
) -> Result<Vec<crate::pv::signals::RawValue>, String> {
    let mut cat = state.writer().map_err(err)?;
    crate::pv::signals::ensure_stats(&mut cat, &photo_ids).map_err(err)?;
    crate::pv::signals::raw_values(&cat, &photo_ids, |shoot| {
        crate::pv::intent::get(&cat, shoot).unwrap_or_default()
    })
    .map_err(err)
}

// ---- AI triage (M4) -----------------------------------------------------------------------

/// How many Photos to measure between progress events and catalog writes.
const TRIAGE_BATCH: usize = 16;

/// What's known about a shoot from earlier runs. Reads no image, so it's safe to call on opening
/// a shoot, including on an offline location.
#[tauri::command]
pub fn pv_triage_results(
    state: State<PvState>,
    folder_id: i64,
) -> Result<crate::pv::triage::ShootTriage, String> {
    state
        .read(|cat| crate::pv::triage::results(cat, folder_id))
        .map_err(err)
}

/// Measures a shoot's Photos and returns its suggestions. Reads images only; it writes nothing but
/// the catalog. The catalog isn't held while reading, which on the NAS is the slow part.
#[tauri::command]
pub async fn pv_triage_run(
    app: AppHandle,
    folder_id: i64,
) -> Result<crate::pv::triage::ShootTriage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<PvState>();
        let todo = state
            .read(|cat| {
                let ids = crate::pv::triage::shoot_photos(cat, folder_id)?;
                crate::pv::triage::pending(cat, &ids)
            })
            .map_err(err)?;
        let total = todo.len();
        for (i, batch) in todo.chunks(TRIAGE_BATCH).enumerate() {
            let _ = app.emit(
                "pv-triage-progress",
                serde_json::json!({
                    "folderId": folder_id,
                    "done": i * TRIAGE_BATCH,
                    "total": total,
                }),
            );
            let measured = crate::pv::triage::measure(batch).map_err(err)?;
            let mut writer = state.writer().map_err(err)?;
            crate::pv::triage::store(&mut writer, &measured).map_err(err)?;
        }
        state
            .read(|cat| crate::pv::triage::results(cat, folder_id))
            .map_err(err)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---- Shoot intent (M2-08) ---------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShootIntent {
    pub folder_id: i64,
    pub intent: crate::pv::signals::Intent,
}

/// The intent of the shoot at `path` (a folder of Photos), or `None` for anything else.
#[tauri::command]
pub fn pv_shoot_intent(state: State<PvState>, path: String) -> Result<Option<ShootIntent>, String> {
    state
        .read(|cat| {
            let Some((folder, _)) = queries::folder_at_path(cat, Path::new(&path))? else {
                return Ok(None);
            };
            let tx = cat.conn().unchecked_transaction()?;
            let shoot = crate::pv::assets::shoot_of(&tx, folder)?;
            drop(tx);
            let Some(shoot) = shoot else { return Ok(None) };
            let photos: i64 = cat.conn().query_row(
                "SELECT COUNT(*) FROM photos WHERE shoot_folder_id = ?1",
                [shoot],
                |r| r.get(0),
            )?;
            if photos == 0 {
                return Ok(None);
            }
            Ok(Some(ShootIntent {
                folder_id: shoot,
                intent: crate::pv::intent::get(cat, shoot)?,
            }))
        })
        .map_err(err)
}

#[tauri::command]
pub fn pv_set_shoot_intent(
    state: State<PvState>,
    folder_id: i64,
    intent: crate::pv::signals::Intent,
) -> Result<(), String> {
    let cat = state.writer().map_err(err)?;
    crate::pv::intent::set(&cat, folder_id, intent).map_err(err)
}

// ---- Slimming (M2-09) -------------------------------------------------------------------------

/// Ready to slim: shoots past the 30-day grace period with RAW+display pairs, not yet reviewed.
#[tauri::command]
pub fn pv_slim_ready(state: State<PvState>) -> Result<Vec<crate::pv::slim::ReadyShoot>, String> {
    state
        .read(|cat| crate::pv::slim::ready(cat, chrono::Local::now().date_naive()))
        .map_err(err)
}

/// Review items (verdict, chips, RAW size) for a shoot or for chosen Photos.
#[tauri::command]
pub fn pv_slim_review(
    state: State<PvState>,
    shoot_id: Option<i64>,
    photo_ids: Option<Vec<i64>>,
) -> Result<Vec<crate::pv::slim::SlimItem>, String> {
    let mut cat = state.writer().map_err(err)?;
    let ids = match (shoot_id, photo_ids) {
        (_, Some(ids)) => ids,
        (Some(shoot), None) => crate::pv::slim::shoot_photos(&cat, shoot).map_err(err)?,
        (None, None) => return Ok(Vec::new()),
    };
    crate::pv::slim::review(&mut cat, &ids).map_err(err)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlimResult {
    pub photos: usize,
    pub bytes: u64,
    /// Photos left alone, with why (locked, needs a render, no longer a pair).
    pub skipped: Vec<lifecycle::plan::Skipped>,
}

/// Apply on the review screen: the chosen Photos' RAWs go to Recently Deleted ("Slimmed RAWs").
/// `reviewed_shoots` are marked done so Ready to slim stops listing them.
#[tauri::command]
pub fn pv_slim_apply(
    app: AppHandle,
    state: State<PvState>,
    photo_ids: Vec<i64>,
    reviewed_shoots: Vec<i64>,
) -> Result<SlimResult, String> {
    let mut cat = state.writer().map_err(err)?;
    lifecycle::plan::sync_sidecars(&cat, &photo_ids).map_err(err)?;
    let plan =
        lifecycle::plan_photos(&cat, &photo_ids, lifecycle::Scope::KeepDisplay).map_err(err)?;
    let result = SlimResult {
        photos: plan.photos(),
        bytes: plan.bytes(),
        skipped: plan.skipped.clone(),
    };
    if !plan.ops.is_empty() {
        lifecycle::upstream::execute(&app, &mut cat, &plan)?;
    }
    let now = catalog::now_ms();
    for shoot in reviewed_shoots {
        if let Err(e) = crate::pv::intent::mark_slim_reviewed(&cat, shoot, now) {
            log::warn!("Pairview: shoot {shoot} not marked reviewed: {e:#}");
        }
    }
    Ok(result)
}

/// "Done" on a review without dropping anything: Ready to slim stops listing the shoot.
#[tauri::command]
pub fn pv_slim_mark_reviewed(state: State<PvState>, shoot_id: i64) -> Result<(), String> {
    let cat = state.writer().map_err(err)?;
    crate::pv::intent::mark_slim_reviewed(&cat, shoot_id, catalog::now_ms()).map_err(err)
}

// ---- Done (M3-02) -----------------------------------------------------------------------------

/// Done in the editor: renders the RAW + recipe at full resolution and replaces the Photo's
/// display copy; the old one goes to Recently Deleted. `adjustments` are the editor's current
/// ones, so an edit made a moment ago counts even before its sidecar is saved.
#[tauri::command]
pub async fn pv_edit_done(
    app: AppHandle,
    path: String,
    adjustments: Option<serde_json::Value>,
) -> Result<lifecycle::Replaced, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<PvState>();
        let target = PathBuf::from(lifecycle::upstream::base_path(&path));
        let photo = {
            let cat = state.writer().map_err(err)?;
            let ids = lifecycle::plan::photo_ids_for_paths(&cat, std::slice::from_ref(&target))
                .map_err(err)?
                .ok_or_else(|| "this isn't a Photo in a library location".to_string())?;
            let photo = ids[0];
            // Fail before the expensive render when it can't succeed.
            crate::pv::edit::display_to_replace(&cat, photo).map_err(err)?;
            let location: i64 = cat
                .conn()
                .query_row(
                    "SELECT f.location_id FROM photos p JOIN folders f ON f.id = p.shoot_folder_id
                     WHERE p.id = ?1",
                    [photo],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            let location = locations::get(&cat, location)
                .map_err(err)?
                .ok_or_else(|| "location not found".to_string())?;
            let status = locations::status(&location, &locations::mounts());
            if !status.is_online() {
                return Err(format!(
                    "{} is offline ({status:?}); nothing was changed",
                    location.name
                ));
            }
            photo
        };
        let recipe = adjustments.clone();
        let rendered = crate::export_processing::render_for_pairview(&path, adjustments, &app)?;
        let mut cat = state.writer().map_err(err)?;
        let replaced = crate::pv::edit::finish(&mut cat, photo, &rendered).map_err(err)?;
        // The edit is done whatever happens here: losing a training example costs little.
        if let Some(recipe) = recipe {
            let history = crate::pv::history::file_next_to(&state.catalog_path);
            if let Err(e) = crate::pv::history::record_edit(&history, &cat, photo, &recipe) {
                log::warn!("Pairview: edit history not recorded: {e:#}");
            }
        }
        let _ = app.emit(
            FOLDER_CHANGED_EVENT,
            watch::FolderChanged {
                location_id: replaced.location_id,
                folders: Vec::new(),
                shoots: Vec::new(),
            },
        );
        Ok(replaced)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Whether the editor should offer Revert for this Photo.
#[tauri::command]
pub fn pv_edit_can_revert(state: State<PvState>, path: String) -> Result<bool, String> {
    let target = PathBuf::from(lifecycle::upstream::base_path(&path));
    let mut cat = state.writer().map_err(err)?;
    let Some(ids) =
        lifecycle::plan::photo_ids_for_paths(&cat, std::slice::from_ref(&target)).map_err(err)?
    else {
        return Ok(false);
    };
    lifecycle::can_revert(&mut cat, ids[0]).map_err(err)
}

/// Revert in the editor: the original comes back; the render and recipe go to Recently Deleted.
#[tauri::command]
pub fn pv_edit_revert(
    app: AppHandle,
    state: State<PvState>,
    path: String,
) -> Result<lifecycle::Reverted, String> {
    let target = PathBuf::from(lifecycle::upstream::base_path(&path));
    let mut cat = state.writer().map_err(err)?;
    let ids = lifecycle::plan::photo_ids_for_paths(&cat, std::slice::from_ref(&target))
        .map_err(err)?
        .ok_or_else(|| "this isn't a Photo in a library location".to_string())?;
    let reverted = crate::pv::edit::revert(&mut cat, ids[0]).map_err(err)?;
    let history = crate::pv::history::file_next_to(&state.catalog_path);
    if let Err(e) = crate::pv::history::record_revert(&history, &cat, ids[0]) {
        log::warn!("Pairview: edit history not recorded: {e:#}");
    }
    let _ = app.emit(
        FOLDER_CHANGED_EVENT,
        watch::FolderChanged {
            location_id: reverted.location_id,
            folders: Vec::new(),
            shoots: Vec::new(),
        },
    );
    Ok(reverted)
}

// ---- Base look (M3-07, M3-08) -----------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseLook {
    pub path: String,
    pub name: String,
    pub size: u32,
}

/// The look RAW Photos start from: `My Sony.cube` in the app's LUT folder, if it has been fitted.
#[tauri::command]
pub fn pv_base_look(app: AppHandle) -> Result<Option<BaseLook>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let name = "My Sony.cube";
    let path = crate::lut_processing::get_luts_dir(&dir)
        .map_err(err)?
        .join(name);
    if !path.is_file() {
        return Ok(None);
    }
    let path = path.to_string_lossy().into_owned();
    let lut = crate::lut_processing::parse_lut_file(&path).map_err(err)?;
    Ok(Some(BaseLook {
        path,
        name: name.to_string(),
        size: lut.size,
    }))
}

/// The Looks panel's catalogue.
#[tauri::command]
pub fn pv_looks(app: AppHandle) -> Result<Vec<crate::pv::looks::Look>, String> {
    // The panel's film stocks are Pairview's own calibrated bake (`resources/pv_film_luts`), not
    // upstream's six, which stay in the Effects LUT control.
    let mut entries: Vec<_> = crate::lut_processing::list_luts(app.clone())?
        .into_iter()
        .filter(|e| !e.is_built_in)
        .collect();
    let film = app
        .path()
        .resolve(
            "resources/pv_film_luts",
            tauri::path::BaseDirectory::Resource,
        )
        .map_err(|e| e.to_string())?;
    entries.extend(crate::lut_processing::list_luts_in_dir(&film, true).map_err(err)?);
    Ok(crate::pv::looks::catalogue(entries))
}
