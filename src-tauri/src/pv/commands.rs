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
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::pv::catalog::{self, Catalog};
use crate::pv::locations::{self, Location, Status, Suggestion};
use crate::pv::metadata::{self, Change, Flag, KeepRaw, PhotoMeta};
use crate::pv::queries::{self, DayCount, Filter, FolderNode, PhotoDetail, PhotoItem, Scope, Sort};
use crate::pv::scan::watch::{self, FOLDER_CHANGED_EVENT, LocationWatcher, WatchOptions};

pub struct PvState {
    catalog_path: PathBuf,
    reader: Mutex<Catalog>,
    watchers: Mutex<HashMap<i64, LocationWatcher>>,
}

impl PvState {
    fn writer(&self) -> Result<Catalog> {
        Catalog::open(&self.catalog_path)
    }

    fn read<T>(&self, f: impl FnOnce(&Catalog) -> Result<T>) -> Result<T> {
        let cat = self
            .reader
            .lock()
            .map_err(|_| anyhow!("catalog lock poisoned"))?;
        f(&cat)
    }
}

/// Opens the catalog and starts a watcher per location. Called once from the app's setup.
pub fn setup(app: &AppHandle) -> Result<()> {
    let dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&dir)?;
    let catalog_path = dir.join(catalog::FILE_NAME);
    let reader = Catalog::open(&catalog_path)?;
    let locations = locations::list(&reader)?;
    app.manage(PvState {
        catalog_path,
        reader: Mutex::new(reader),
        watchers: Mutex::new(HashMap::new()),
    });
    let state = app.state::<PvState>();
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
