//! 🛡 Every file mutation goes through here (docs/IMPLEMENTATION_PLAN.md A7, A8, M2-01).
//!
//! **Recently Deleted** is `<location root>/.pairview-deleted/<op-id>/<original relative path>`
//! plus a `manifest.json` per operation. Files only ever move by `rename()` on the volume they
//! are on: instant on SMB, nothing copied, nothing lost. Nothing is permanently deleted except
//! by `purge`, and purge removes only files a committed manifest lists.
//!
//! **Write-ahead op-log.** Before the first move an operation is written to `ops_log` as
//! `pending` with every planned move. A crash leaves it pending, and `recover` (at startup and
//! before every operation on that location) moves back whatever had moved, so an operation is
//! either complete or never happened.
//!
//! **Refusals.** Nothing is changed when the location is offline, when a file is on a different
//! volume from the location root (a nested mount), when a path is a symlink or leaves the
//! location, or when an earlier operation on the location can't be recovered.
//!
//! The catalog follows the files: rows of trashed files are removed and their shoots re-paired
//! straight away; restored files are added back (the watcher's next scan would find them too).

// Commands (M2-02..04) are the first users.
#![allow(dead_code)]

pub mod manifest;
pub mod plan;
#[cfg(test)]
mod tests;
pub mod upstream;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail, ensure};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::pv::assets;
use crate::pv::catalog::{Catalog, now_ms};
use crate::pv::locations::{self, Location};
// The commands (M2-02..04) are the first users outside this module.
#[allow(unused_imports)]
pub use manifest::{Manifest, ManifestFile, Reason};
use manifest::{ManifestLocation, Status, safe_join, safe_rel};
#[allow(unused_imports)]
pub use plan::{OpPlan, Plan, Scope, SkipReason, plan_delete, plan_photos};

/// Recently Deleted folder at each location root. The scanner skips dot-folders.
pub const TRASH_DIR: &str = ".pairview-deleted";
pub const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// Retention for everything in Recently Deleted (PRD D12).
pub const RETENTION_MS: i64 = 30 * DAY_MS;

/// The moves of one operation, as stored in `ops_log.payload_json`. Paths are relative to the
/// location root, so a location that was relocated is still recovered correctly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Payload {
    /// `.pairview-deleted/<op-id>`.
    op_dir: String,
    /// `(from, to)` in order.
    moves: Vec<(String, String)>,
    /// XMP copies this operation created (removed again on rollback).
    created: Vec<String>,
}

/// A finished trash operation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Done {
    pub op_id: String,
    pub location_id: i64,
    pub files: usize,
    pub bytes: u64,
}

/// One file operation at a time in this process. Each operation replays unfinished ones on its
/// location first; without this, a second delete could "recover" a first that is still moving.
static EXCLUSIVE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn exclusive() -> std::sync::MutexGuard<'static, ()> {
    // A panic mid-operation leaves the op-log pending, which the next holder replays.
    EXCLUSIVE.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- Trash ------------------------------------------------------------------------------------

/// Moves every planned file to Recently Deleted, one operation per location. An operation that
/// fails leaves its location untouched; the others still happen.
pub fn apply(cat: &mut Catalog, plan: &Plan) -> Vec<(i64, Result<Done>)> {
    let _one_at_a_time = exclusive();
    plan.ops
        .iter()
        .map(|op| (op.location_id, trash(cat, op)))
        .collect()
}

fn trash(cat: &mut Catalog, op: &OpPlan) -> Result<Done> {
    ensure!(!op.files.is_empty(), "nothing to delete");
    let location = online_location(cat, op.location_id)?;
    let root = location.path.clone();
    let root_dev = device(&root)?;

    // Check every file before anything moves.
    let mut files = Vec::with_capacity(op.files.len());
    for f in &op.files {
        let rel = safe_rel(&f.rel)?;
        ensure!(
            !rel.starts_with(TRASH_DIR),
            "{} is already in Recently Deleted",
            f.rel
        );
        let path = safe_join(&root, &f.rel)?;
        let meta = fs::symlink_metadata(&path)
            .with_context(|| format!("{} is gone; nothing was deleted", f.rel))?;
        ensure!(
            meta.is_file(),
            "{} is not a regular file; nothing was deleted",
            f.rel
        );
        ensure!(
            device(&path)? == root_dev,
            "{} is on a different volume from {}; nothing was deleted",
            f.rel,
            location.name
        );
        files.push(ManifestFile {
            path: f.rel.clone(),
            size: meta.len(),
            mtime: mtime_ms(&meta),
            role: f.role.clone(),
            photo: f.photo,
        });
    }
    for (from, to) in &op.xmp_copies {
        ensure_xmp(from)?;
        ensure_xmp(to)?;
    }

    let trash_dir = root.join(TRASH_DIR);
    match fs::symlink_metadata(&trash_dir) {
        Ok(meta) => ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "{} is not a plain folder; nothing was deleted",
            trash_dir.display()
        ),
        Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir(&trash_dir)
            .with_context(|| format!("cannot create {}", trash_dir.display()))?,
        Err(e) => return Err(e.into()),
    }
    ensure!(
        device(&trash_dir)? == root_dev,
        "{} is on a different volume; nothing was deleted",
        trash_dir.display()
    );

    let op_id = new_op_id();
    let op_dir_rel = format!("{TRASH_DIR}/{op_id}");
    let op_dir = root.join(&op_dir_rel);
    let mut payload = Payload {
        op_dir: op_dir_rel.clone(),
        moves: files
            .iter()
            .map(|f| (f.path.clone(), format!("{op_dir_rel}/{}", f.path)))
            .collect(),
        created: Vec::new(),
    };
    let mut manifest = Manifest {
        version: manifest::VERSION,
        op_id: op_id.clone(),
        kind: op.kind.to_string(),
        reason: op.reason,
        status: Status::Pending,
        created_at: now_ms(),
        committed_at: None,
        location: ManifestLocation {
            name: location.name.clone(),
            volume_id: location.volume_id.clone(),
            volume_rel_path: location.volume_rel_path.clone(),
        },
        files,
        photos: op.photos.clone(),
    };

    // Write-ahead: the op-log row exists before anything on disk changes.
    log_begin(cat, &op_id, op.kind, location.id, &payload)?;
    let result = (|| -> Result<()> {
        fs::create_dir(&op_dir).with_context(|| format!("cannot create {}", op_dir.display()))?;
        manifest.write(&op_dir)?;
        for (from, to) in &op.xmp_copies {
            let target = safe_join(&root, to)?;
            if fs::symlink_metadata(&target).is_ok() {
                continue;
            }
            payload.created.push(to.clone());
            log_payload(cat, &op_id, &payload)?;
            let bytes = fs::read(safe_join(&root, from)?)?;
            crate::file_management::write_file_atomic(&target, &bytes)
                .with_context(|| format!("cannot write {to}"))?;
        }
        run_moves(&root, &payload.moves)
    })();
    if let Err(e) = result {
        return Err(abandon(cat, &root, &op_id, op.kind, &payload, e));
    }
    log_finish(cat, &op_id, "committed")?;

    manifest.status = Status::Committed;
    manifest.committed_at = Some(now_ms());
    if let Err(e) = manifest.write(&op_dir) {
        // The files are safely in Recently Deleted; the op-log says committed.
        log::warn!("Pairview: {op_id} committed but its manifest wasn't updated: {e:#}");
    }

    if let Err(e) = forget_files(cat, op) {
        // The files are in Recently Deleted either way; the watcher's next scan catches up.
        log::warn!("Pairview: {op_id}: catalog not updated: {e:#}");
    }
    Ok(Done {
        op_id,
        location_id: location.id,
        files: manifest.files.len(),
        bytes: manifest.bytes(),
    })
}

/// Removes trashed files from the catalog and re-pairs their shoots.
fn forget_files(cat: &mut Catalog, op: &OpPlan) -> Result<()> {
    let tx = cat.conn_mut().transaction()?;
    for f in &op.files {
        if let Some(id) = f.file_id {
            tx.execute("DELETE FROM files WHERE id = ?1", [id])?;
        }
    }
    for &shoot in &op.shoots {
        assets::rebuild_shoot(&tx, shoot)?;
    }
    tx.commit()?;
    Ok(())
}

// ---- Recently Deleted -------------------------------------------------------------------------

/// One operation in Recently Deleted, with only the files still there.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedOp {
    pub location_id: i64,
    pub manifest: Manifest,
    /// Absolute path of the operation folder (thumbnails read files from here).
    pub dir: PathBuf,
    pub bytes: u64,
    /// When `purge_expired` will remove it.
    pub expires_at: Option<i64>,
}

/// Lists Recently Deleted at a location, newest first. Folders without a readable manifest are
/// skipped (and never purged).
pub fn list(cat: &mut Catalog, location_id: i64) -> Result<Vec<DeletedOp>> {
    let _one_at_a_time = exclusive();
    list_unlocked(cat, location_id)
}

fn list_unlocked(cat: &mut Catalog, location_id: i64) -> Result<Vec<DeletedOp>> {
    let location = online_location(cat, location_id)?;
    let trash_dir = location.path.join(TRASH_DIR);
    let entries = match fs::read_dir(&trash_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut ops = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let mut manifest = match Manifest::read(&dir) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("Pairview: skipping {}: {e:#}", dir.display());
                continue;
            }
        };
        manifest
            .files
            .retain(|f| safe_join(&dir, &f.path).is_ok_and(|p| fs::symlink_metadata(p).is_ok()));
        if manifest.files.is_empty() {
            continue;
        }
        let expires_at = manifest
            .committed_at
            .filter(|_| manifest.status == Status::Committed)
            .map(|t| t + RETENTION_MS);
        ops.push(DeletedOp {
            location_id,
            bytes: manifest.bytes(),
            expires_at,
            manifest,
            dir,
        });
    }
    ops.sort_by_key(|o| std::cmp::Reverse(o.manifest.created_at));
    Ok(ops)
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Restored {
    pub files: usize,
    /// Original paths that exist again (a new file took the name); those stay in Recently
    /// Deleted.
    pub conflicts: Vec<String>,
}

/// Puts files of operation `op_id` back where they were: all of them, or only `only` (original
/// relative paths). A file whose original path is taken again is left in Recently Deleted.
pub fn restore(
    cat: &mut Catalog,
    location_id: i64,
    op_id: &str,
    only: Option<&[String]>,
) -> Result<Restored> {
    let _one_at_a_time = exclusive();
    let location = online_location(cat, location_id)?;
    let root = location.path.clone();
    let op_dir_rel = op_dir_rel(op_id)?;
    let op_dir = root.join(&op_dir_rel);
    let mut manifest = Manifest::read(&op_dir)?;
    ensure!(
        manifest.status == Status::Committed,
        "{op_id} didn't finish; it is rolled back when Pairview next checks this location"
    );

    let mut payload = Payload {
        op_dir: op_dir_rel.clone(),
        ..Default::default()
    };
    let mut outcome = Restored::default();
    let mut restored = Vec::new();
    for f in &manifest.files {
        if only.is_some_and(|only| !only.contains(&f.path)) {
            continue;
        }
        let from = safe_join(&op_dir, &f.path)?;
        if fs::symlink_metadata(&from).is_err() {
            continue;
        }
        ensure!(
            !safe_rel(&f.path)?.starts_with(TRASH_DIR),
            "refusing to restore into {TRASH_DIR}"
        );
        let to = safe_join(&root, &f.path)?;
        if fs::symlink_metadata(&to).is_ok() {
            outcome.conflicts.push(f.path.clone());
            continue;
        }
        payload
            .moves
            .push((format!("{op_dir_rel}/{}", f.path), f.path.clone()));
        restored.push(f.path.clone());
    }
    if payload.moves.is_empty() {
        return Ok(outcome);
    }

    let restore_id = new_op_id();
    log_begin(cat, &restore_id, "restore", location.id, &payload)?;
    if let Err(e) = run_moves(&root, &payload.moves) {
        return Err(abandon(cat, &root, &restore_id, "restore", &payload, e));
    }
    log_finish(cat, &restore_id, "committed")?;
    outcome.files = restored.len();

    // What's left in the operation folder.
    manifest.files.retain(|f| !restored.contains(&f.path));
    if manifest.files.is_empty() {
        let _ = fs::remove_file(op_dir.join(manifest::FILE_NAME));
        prune_dirs(&op_dir, &restored);
        let _ = fs::remove_dir(&op_dir);
    } else {
        prune_dirs(&op_dir, &restored);
        if let Err(e) = manifest.write(&op_dir) {
            // `list` skips files that are gone, so a stale manifest only lists too much.
            log::warn!("Pairview: {op_id}: manifest not updated after restore: {e:#}");
        }
    }

    if let Err(e) = remember_files(cat, &location, &restored) {
        log::warn!("Pairview: restore of {op_id}: catalog not updated: {e:#}");
    }
    Ok(outcome)
}

/// Adds restored files back to the catalog, re-pairs their shoots and reads their metadata.
fn remember_files(cat: &mut Catalog, location: &Location, rels: &[String]) -> Result<()> {
    let mut shoots = Vec::new();
    {
        let tx = cat.conn_mut().transaction()?;
        for rel in rels {
            let (folder, name) = rel.rsplit_once('/').unwrap_or(("", rel));
            let (stem, ext) = crate::pv::scan::l0::split_name(name);
            let Some(kind) = crate::pv::scan::l0::file_kind(ext) else {
                continue;
            };
            let folder_id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM folders WHERE location_id = ?1 AND rel_path = ?2",
                    (location.id, folder),
                    |r| r.get(0),
                )
                .optional()?;
            // A folder the catalog doesn't know yet: the watcher's next scan adds it.
            let Some(folder_id) = folder_id else { continue };
            let meta = fs::symlink_metadata(location.path.join(rel))?;
            tx.execute(
                "INSERT OR IGNORE INTO files (folder_id, name, stem_key, ext, kind, size, mtime)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                (
                    folder_id,
                    name,
                    stem.to_lowercase(),
                    ext.to_ascii_lowercase(),
                    kind,
                    meta.len() as i64,
                    mtime_ms(&meta),
                ),
            )?;
            if let Some(shoot) = assets::shoot_of(&tx, folder_id)?
                && !shoots.contains(&shoot)
            {
                shoots.push(shoot);
            }
        }
        for &shoot in &shoots {
            assets::rebuild_shoot(&tx, shoot)?;
        }
        tx.commit()?;
    }
    crate::pv::scan::l1::run(cat, &shoots)?;
    crate::pv::metadata::sync_shoots(cat, &shoots)?;
    Ok(())
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Purged {
    pub ops: usize,
    pub files: usize,
    pub bytes: u64,
}

/// Permanently deletes operations committed at or before `cutoff` (all of them when `None`:
/// "Empty now"), limited to `op_ids` when given. Only files a manifest lists are deleted; an
/// operation folder with anything else in it is left in place.
pub fn purge(
    cat: &mut Catalog,
    location_id: i64,
    cutoff: Option<i64>,
    op_ids: Option<&[String]>,
) -> Result<Purged> {
    let _one_at_a_time = exclusive();
    let ops = list_unlocked(cat, location_id)?;
    let root = online_location(cat, location_id)?.path;
    let mut purged = Purged::default();
    for op in ops {
        let m = &op.manifest;
        let Some(committed_at) = m.committed_at.filter(|_| m.status == Status::Committed) else {
            continue;
        };
        if cutoff.is_some_and(|c| committed_at > c)
            || op_ids.is_some_and(|ids| !ids.contains(&m.op_id))
        {
            continue;
        }
        let payload = Payload {
            op_dir: op_dir_rel(&m.op_id)?,
            moves: m
                .files
                .iter()
                .map(|f| (f.path.clone(), String::new()))
                .collect(),
            created: Vec::new(),
        };
        let purge_id = new_op_id();
        log_begin(cat, &purge_id, "purge", location_id, &payload)?;
        let op_dir = root.join(&payload.op_dir);
        let mut gone = Vec::new();
        for f in &m.files {
            let path = safe_join(&op_dir, &f.path)?;
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_file() || meta.file_type().is_symlink() => {
                    fs::remove_file(&path)
                        .with_context(|| format!("cannot delete {}", path.display()))?;
                    purged.files += 1;
                    purged.bytes += meta.len();
                    gone.push(f.path.clone());
                }
                _ => {}
            }
        }
        let _ = fs::remove_file(op_dir.join(manifest::FILE_NAME));
        prune_dirs(&op_dir, &gone);
        let _ = fs::remove_dir(&op_dir);
        log_finish(cat, &purge_id, "committed")?;
        purged.ops += 1;
    }
    Ok(purged)
}

/// The 30-day purge (PRD D12).
pub fn purge_expired(cat: &mut Catalog, location_id: i64, now: i64) -> Result<Purged> {
    purge(cat, location_id, Some(now - RETENTION_MS), None)
}

/// The file to preview Photo `photo` (manifest index) of operation `op_id` with: its display copy,
/// else its RAW. Reads only.
pub fn preview_file(location_root: &Path, op_id: &str, photo: usize) -> Result<PathBuf> {
    let op_dir = location_root.join(op_dir_rel(op_id)?);
    let manifest = Manifest::read(&op_dir)?;
    let mut files: Vec<&ManifestFile> = manifest
        .files
        .iter()
        .filter(|f| f.photo == Some(photo) && f.role != "sidecar")
        .collect();
    files.sort_by_key(|f| match f.role.as_str() {
        "display" => 0,
        "extra" => 1,
        "raw" => 2,
        _ => 3,
    });
    for f in files {
        let path = safe_join(&op_dir, &f.path)?;
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
            return Ok(path);
        }
    }
    bail!("nothing to preview for Photo {photo} of {op_id}")
}

// ---- Op-log replay ----------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recovered {
    pub rolled_back: usize,
    /// Operations that couldn't be put back cleanly (left marked `failed` with the reason).
    pub failed: usize,
    /// Operations on offline locations, left pending for later.
    pub waiting: usize,
}

/// Rolls back operations a crash left pending, on every online location. Called at startup.
pub fn recover(cat: &mut Catalog) -> Result<Recovered> {
    let _one_at_a_time = exclusive();
    recover_where(cat, None)
}

fn recover_where(cat: &mut Catalog, only_location: Option<i64>) -> Result<Recovered> {
    let pending: Vec<(String, String, Option<i64>, String)> = {
        let mut stmt = cat.conn().prepare(
            "SELECT id, kind, location_id, payload_json FROM ops_log
             WHERE status = 'pending' ORDER BY created_at DESC",
        )?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mounts = locations::mounts();
    let mut out = Recovered::default();
    for (id, kind, location_id, payload) in pending {
        if only_location.is_some_and(|l| location_id != Some(l)) {
            continue;
        }
        let location = match location_id {
            Some(l) => locations::get(cat, l)?,
            None => None,
        };
        let Some(location) = location else {
            log_finish(cat, &id, "failed")?;
            out.failed += 1;
            continue;
        };
        if !locations::status(&location, &mounts).is_online() {
            out.waiting += 1;
            continue;
        }
        let payload: Payload = serde_json::from_str(&payload)
            .with_context(|| format!("op-log entry {id} is unreadable"))?;
        if kind == "purge" {
            // Nothing to put back; the files listed are deleted or still there.
            log_finish(cat, &id, "failed")?;
            out.failed += 1;
            continue;
        }
        match rollback(&location.path, &kind, &payload) {
            Ok(()) => {
                log_finish(cat, &id, "rolled_back")?;
                out.rolled_back += 1;
            }
            Err(e) => {
                log::error!("Pairview: cannot roll back {id}: {e:#}");
                log_finish(cat, &id, "failed")?;
                out.failed += 1;
            }
        }
    }
    Ok(out)
}

/// Undoes whatever part of an operation happened. Safe to run more than once.
fn rollback(root: &Path, kind: &str, payload: &Payload) -> Result<()> {
    let mut problems = Vec::new();
    for (from, to) in payload.moves.iter().rev() {
        let (src, dst) = (safe_join(root, from)?, safe_join(root, to)?);
        let (at_src, at_dst) = (
            fs::symlink_metadata(&src).is_ok(),
            fs::symlink_metadata(&dst).is_ok(),
        );
        match (at_src, at_dst) {
            (true, false) => {}
            (false, true) => {
                if let Err(e) = fs::rename(&dst, &src) {
                    problems.push(format!("{to} → {from}: {e}"));
                }
            }
            (true, true) => problems.push(format!("both {from} and {to} exist")),
            (false, false) => problems.push(format!("{from} is missing")),
        }
    }
    for created in &payload.created {
        ensure_xmp(created)?;
        match fs::remove_file(safe_join(root, created)?) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                problems.push(format!("{created}: {e}"))
            }
            _ => {}
        }
    }
    if kind != "restore" && problems.is_empty() {
        let op_dir = safe_join(root, &payload.op_dir)?;
        let _ = fs::remove_file(op_dir.join(manifest::FILE_NAME));
        let rels: Vec<String> = payload
            .moves
            .iter()
            .filter_map(|(_, to)| to.strip_prefix(&format!("{}/", payload.op_dir)))
            .map(str::to_string)
            .collect();
        prune_dirs(&op_dir, &rels);
        let _ = fs::remove_dir(&op_dir);
    }
    if problems.is_empty() {
        Ok(())
    } else {
        bail!("{}", problems.join("; "))
    }
}

/// Rolls back a failed operation in place and returns the error to report.
fn abandon(
    cat: &Catalog,
    root: &Path,
    op_id: &str,
    kind: &str,
    payload: &Payload,
    error: anyhow::Error,
) -> anyhow::Error {
    if error.is::<Crash>() {
        return error; // Simulated crash (tests): leave it for `recover`.
    }
    let (status, error) = match rollback(root, kind, payload) {
        Ok(()) => ("rolled_back", error.context("nothing was changed")),
        Err(e) => (
            "failed",
            error.context(format!("and putting files back failed too: {e:#}")),
        ),
    };
    if let Err(e) = log_finish(cat, op_id, status) {
        log::error!("Pairview: op-log update for {op_id} failed: {e:#}");
    }
    error
}

// ---- Helpers ----------------------------------------------------------------------------------

/// The location, if it is online and has no operation left half done.
fn online_location(cat: &mut Catalog, location_id: i64) -> Result<Location> {
    let location = locations::get(cat, location_id)?
        .ok_or_else(|| anyhow!("location {location_id} not found"))?;
    let status = locations::status(&location, &locations::mounts());
    ensure!(
        status.is_online(),
        "{} is offline ({status:?}); nothing was changed",
        location.name
    );
    let left = recover_where(cat, Some(location_id))?;
    ensure!(
        left.failed == 0 && left.waiting == 0,
        "an earlier operation on {} couldn't be undone; check the log before changing files there",
        location.name
    );
    let stuck: i64 = cat.conn().query_row(
        "SELECT COUNT(*) FROM ops_log WHERE status = 'pending' AND location_id = ?1",
        [location_id],
        |r| r.get(0),
    )?;
    ensure!(
        stuck == 0,
        "an operation on {} is unfinished",
        location.name
    );
    Ok(location)
}

fn run_moves(root: &Path, moves: &[(String, String)]) -> Result<()> {
    for (i, (from, to)) in moves.iter().enumerate() {
        test_hooks::before_move(i)?;
        let (src, dst) = (safe_join(root, from)?, safe_join(root, to)?);
        ensure!(fs::symlink_metadata(&dst).is_err(), "{to} already exists");
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        // rename() never copies: across volumes it fails (EXDEV) instead.
        fs::rename(&src, &dst).with_context(|| format!("cannot move {from}"))?;
    }
    Ok(())
}

/// Removes now-empty folders left by `rels` inside `base`, deepest first. Folders with anything
/// else in them stay.
fn prune_dirs(base: &Path, rels: &[String]) {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for rel in rels {
        let mut p = Path::new(rel).parent();
        while let Some(d) = p.filter(|d| !d.as_os_str().is_empty()) {
            dirs.push(base.join(d));
            p = d.parent();
        }
    }
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    for d in dirs {
        let _ = fs::remove_dir(d);
    }
}

fn op_dir_rel(op_id: &str) -> Result<String> {
    ensure!(
        !op_id.is_empty() && op_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "not an operation id: {op_id:?}"
    );
    Ok(format!("{TRASH_DIR}/{op_id}"))
}

/// Sortable and readable: `20261104-153012-1a2b3c`.
fn new_op_id() -> String {
    let random = uuid::Uuid::new_v4().simple().to_string();
    format!(
        "{}-{}",
        chrono::Local::now().format("%Y%m%d-%H%M%S"),
        &random[..6]
    )
}

fn ensure_xmp(rel: &str) -> Result<()> {
    ensure!(
        Path::new(rel)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("xmp")),
        "refusing {rel}: not an .xmp sidecar"
    );
    Ok(())
}

fn mtime_ms(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

fn device(path: &Path) -> Result<u64> {
    if let Some(dev) = test_hooks::fake_device(path) {
        return Ok(dev);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(fs::symlink_metadata(path)
            .with_context(|| format!("cannot read {}", path.display()))?
            .dev())
    }
    // Windows (tabled): rename() refuses cross-volume moves itself.
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(0)
    }
}

fn log_begin(
    cat: &Catalog,
    id: &str,
    kind: &str,
    location_id: i64,
    payload: &Payload,
) -> Result<()> {
    cat.conn().execute(
        "INSERT INTO ops_log (id, kind, status, location_id, payload_json, created_at)
         VALUES (?1, ?2, 'pending', ?3, ?4, ?5)",
        (
            id,
            kind,
            location_id,
            serde_json::to_string(payload)?,
            now_ms(),
        ),
    )?;
    Ok(())
}

fn log_payload(cat: &Catalog, id: &str, payload: &Payload) -> Result<()> {
    cat.conn().execute(
        "UPDATE ops_log SET payload_json = ?2 WHERE id = ?1",
        (id, serde_json::to_string(payload)?),
    )?;
    Ok(())
}

fn log_finish(cat: &Catalog, id: &str, status: &str) -> Result<()> {
    cat.conn().execute(
        "UPDATE ops_log SET status = ?2, finished_at = ?3 WHERE id = ?1",
        (id, status, now_ms()),
    )?;
    Ok(())
}

/// A simulated process death (tests only): the operation stops where it is, unrecorded.
#[derive(Debug)]
struct Crash;

impl std::fmt::Display for Crash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("simulated crash")
    }
}

impl std::error::Error for Crash {}

#[cfg(not(test))]
mod test_hooks {
    use std::path::Path;

    pub fn before_move(_: usize) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn fake_device(_: &Path) -> Option<u64> {
        None
    }
}

#[cfg(test)]
pub(crate) mod test_hooks {
    use std::cell::{Cell, RefCell};
    use std::path::{Path, PathBuf};

    #[derive(Clone, Copy)]
    pub enum Interrupt {
        /// The process dies before move N.
        Crash(usize),
        /// Move N fails with an I/O error.
        Fail(usize),
    }

    thread_local! {
        pub static INTERRUPT: Cell<Option<Interrupt>> = const { Cell::new(None) };
        /// Paths under this prefix report this device number.
        pub static FAKE_DEVICE: RefCell<Option<(PathBuf, u64)>> = const { RefCell::new(None) };
    }

    pub fn before_move(i: usize) -> anyhow::Result<()> {
        match INTERRUPT.with(Cell::get) {
            Some(Interrupt::Crash(n)) if n == i => Err(super::Crash.into()),
            Some(Interrupt::Fail(n)) if n == i => {
                Err(std::io::Error::other("simulated I/O error").into())
            }
            _ => Ok(()),
        }
    }

    pub fn fake_device(path: &Path) -> Option<u64> {
        FAKE_DEVICE.with(|f| {
            f.borrow()
                .as_ref()
                .filter(|(prefix, _)| path.starts_with(prefix))
                .map(|(_, dev)| *dev)
        })
    }
}
