//! A7 hooks: RapidRAW's delete commands hand Pairview the paths inside its locations
//! (docs/IMPLEMENTATION_PLAN.md M2-02). 🛡
//!
//! Upstream moves files to the OS trash and, when that fails (it does on SMB shares), deletes
//! them permanently, matching pairs by stem in one folder only. Inside a Pairview location the
//! same commands go to Recently Deleted instead, Photo by Photo (a RAW in `arw/` and its HIF in
//! `hif/` together, guard-split pairs apart). Paths outside every location are handed back so
//! upstream treats them exactly as before.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Result;
use tauri::{AppHandle, Emitter, Manager};

use super::{Done, Plan, apply, plan};
use crate::pv::commands::PvState;
use crate::pv::scan::watch::{FOLDER_CHANGED_EVENT, FolderChanged};

/// `delete_files_from_disk` (`associated = false`) and `delete_files_with_associated`. Returns
/// the paths Pairview didn't take.
pub fn take_over_delete(
    app: &AppHandle,
    paths: Vec<String>,
    associated: bool,
) -> Result<Vec<String>, String> {
    let Some(state) = app.try_state::<PvState>() else {
        return Ok(paths);
    };
    let targets: Vec<PathBuf> = paths.iter().map(|p| target(p, associated)).collect();
    let outside: HashSet<PathBuf> = state
        .read(|cat| plan::outside_locations(cat, &targets))
        .map_err(err)?
        .into_iter()
        .collect();
    let (mut taken, mut rest, mut taken_targets) = (Vec::new(), Vec::new(), Vec::new());
    for (path, t) in paths.into_iter().zip(targets) {
        if outside.contains(&t) {
            rest.push(path);
        } else {
            taken.push(path);
            taken_targets.push(t);
        }
    }
    if taken.is_empty() {
        return Ok(rest);
    }
    run(app, &state, |cat| {
        Ok(plan::plan_paths_with(cat, &taken_targets, associated)?.0)
    })?;
    let deletions: HashSet<String> = taken.into_iter().collect();
    crate::file_management::sync_album_path_changes(app, None, Some(&deletions), None);
    Ok(rest)
}

/// `delete_folder`. `true` when the folder was inside a location and went to Recently Deleted.
pub fn take_over_folder(app: &AppHandle, path: &str) -> Result<bool, String> {
    let Some(state) = app.try_state::<PvState>() else {
        return Ok(false);
    };
    let folder = PathBuf::from(path);
    let inside = state
        .read(|cat| plan::outside_locations(cat, std::slice::from_ref(&folder)))
        .map_err(err)?
        .is_empty();
    if !inside {
        return Ok(false);
    }
    run(app, &state, |cat| {
        Ok(plan::plan_folder(cat, &folder)?.unwrap_or_default())
    })?;
    // Folders left empty go too (they come back with a restore).
    remove_empty_dirs(&folder);
    let deletions: HashSet<String> = [path.to_string()].into();
    crate::file_management::sync_album_path_changes(app, None, Some(&deletions), None);
    Ok(true)
}

/// What upstream would delete for one of its paths: the virtual copy's own sidecar for
/// `…?vc=id`, else the image itself.
fn target(path: &str, associated: bool) -> PathBuf {
    let (source, sidecar) = crate::file_management::parse_virtual_path(path);
    if path.contains("?vc=") && !associated {
        sidecar
    } else {
        source
    }
}

fn run(
    app: &AppHandle,
    state: &PvState,
    plan: impl FnOnce(&crate::pv::catalog::Catalog) -> Result<Plan>,
) -> Result<Vec<Done>, String> {
    let mut cat = state.writer().map_err(err)?;
    let plan = plan(&cat).map_err(err)?;
    if let Some(s) = plan.skipped.first() {
        // Only Keep HIF only on a locked Photo can be refused here.
        return Err(format!(
            "{} Photo(s) not deleted ({:?}); nothing was changed",
            plan.skipped.len(),
            s.reason
        ));
    }
    execute(app, &mut cat, &plan)
}

/// Applies a plan and tells the trees and grid which shoots changed. Every location's operation
/// runs; the errors of those that failed are returned together.
pub(crate) fn execute(
    app: &AppHandle,
    cat: &mut crate::pv::catalog::Catalog,
    plan: &Plan,
) -> Result<Vec<Done>, String> {
    let mut done = Vec::new();
    let mut errors = Vec::new();
    for (op, (location_id, result)) in plan.ops.iter().zip(apply(cat, plan)) {
        match result {
            Ok(d) => {
                let _ = app.emit(
                    FOLDER_CHANGED_EVENT,
                    FolderChanged {
                        location_id,
                        folders: Vec::new(),
                        shoots: op.shoots.clone(),
                    },
                );
                done.push(d);
            }
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    if errors.is_empty() {
        Ok(done)
    } else {
        Err(errors.join("\n"))
    }
}

fn remove_empty_dirs(dir: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                remove_empty_dirs(&e.path());
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// A grid path without its virtual-copy suffix.
pub fn base_path(path: &str) -> &str {
    path.split("?vc=").next().unwrap_or(path)
}
