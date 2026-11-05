//! Shoot intent (docs/IMPLEMENTATION_PLAN.md M2-08, PRD §6.5 Stage 1): Casual · Normal ·
//! Important, per shoot folder. Set by hand now; the AI guess arrives in M4 and never overrides a
//! choice the owner made.
//!
//! The record on disk is `<shoot>/.pairview-shoot.json` (the catalog is a cache, A3); the catalog
//! keeps a copy in `folders.intent` so the RAW-value verdicts don't read the NAS each time. The
//! same file remembers when the shoot's slim review was done (M2-09, `folders.slim_reviewed_at`).

use std::path::PathBuf;

use anyhow::{Result, anyhow, ensure};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::pv::catalog::Catalog;
use crate::pv::locations;
use crate::pv::signals::Intent;

pub const FILE_NAME: &str = ".pairview-shoot.json";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShootFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    intent: Option<Intent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slim_reviewed_at: Option<i64>,
}

fn read_file(dir: &std::path::Path) -> ShootFile {
    std::fs::read_to_string(dir.join(FILE_NAME))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Read-modify-write of the shoot file, refused when the location is offline.
fn update_file(cat: &Catalog, shoot_id: i64, change: impl FnOnce(&mut ShootFile)) -> Result<()> {
    let (location, dir) = folder_dir(cat, shoot_id)?;
    let status = locations::status(&location, &locations::mounts());
    ensure!(
        status.is_online(),
        "{} is offline ({status:?}); nothing was changed",
        location.name
    );
    let mut file = read_file(&dir);
    file.version = 1;
    change(&mut file);
    let path = dir.join(FILE_NAME);
    crate::file_management::write_file_atomic(&path, &serde_json::to_vec_pretty(&file)?)
        .map_err(|e| anyhow!("cannot write {}: {e}", path.display()))
}

fn as_str(intent: Intent) -> &'static str {
    match intent {
        Intent::Casual => "casual",
        Intent::Normal => "normal",
        Intent::Important => "important",
    }
}

fn parse(s: &str) -> Option<Intent> {
    match s {
        "casual" => Some(Intent::Casual),
        "normal" => Some(Intent::Normal),
        "important" => Some(Intent::Important),
        _ => None,
    }
}

/// `(location path, folder rel path)` of a folder.
fn folder_dir(cat: &Catalog, folder_id: i64) -> Result<(locations::Location, PathBuf)> {
    let (location_id, rel): (i64, String) = cat.conn().query_row(
        "SELECT location_id, rel_path FROM folders WHERE id = ?1",
        [folder_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let location =
        locations::get(cat, location_id)?.ok_or_else(|| anyhow!("location {location_id} gone"))?;
    let dir = if rel.is_empty() {
        location.path.clone()
    } else {
        location.path.join(rel)
    };
    Ok((location, dir))
}

/// The shoot's intent: the catalog's copy, else its `.pairview-shoot.json` (read once and
/// remembered), else Normal.
pub fn get(cat: &Catalog, shoot_id: i64) -> Result<Intent> {
    let stored: Option<Option<String>> = cat
        .conn()
        .query_row(
            "SELECT intent FROM folders WHERE id = ?1",
            [shoot_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(stored) = stored else {
        return Ok(Intent::Normal);
    };
    if let Some(intent) = stored.as_deref().and_then(parse) {
        return Ok(intent);
    }
    let (_, dir) = folder_dir(cat, shoot_id)?;
    let file = read_file(&dir);
    let intent = file.intent.unwrap_or_default();
    cat.conn().execute(
        "UPDATE folders SET intent = ?2, slim_reviewed_at = COALESCE(slim_reviewed_at, ?3)
         WHERE id = ?1",
        (shoot_id, as_str(intent), file.slim_reviewed_at),
    )?;
    Ok(intent)
}

/// Sets a shoot's intent: writes `.pairview-shoot.json` in the shoot folder (only that file),
/// then the catalog. Refused when the location is offline.
pub fn set(cat: &Catalog, shoot_id: i64, intent: Intent) -> Result<()> {
    update_file(cat, shoot_id, |f| f.intent = Some(intent))?;
    cat.conn().execute(
        "UPDATE folders SET intent = ?2 WHERE id = ?1",
        (shoot_id, as_str(intent)),
    )?;
    Ok(())
}

/// Records that the owner finished a slim review of the shoot (Ready to slim stops listing it).
pub fn mark_slim_reviewed(cat: &Catalog, shoot_id: i64, at: i64) -> Result<()> {
    // Make sure the intent was read from the file first, so it isn't lost on write.
    get(cat, shoot_id)?;
    update_file(cat, shoot_id, |f| f.slim_reviewed_at = Some(at))?;
    cat.conn().execute(
        "UPDATE folders SET slim_reviewed_at = ?2 WHERE id = ?1",
        (shoot_id, at),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests_support {
    use super::*;
    use crate::pv::scan::l0::{ScanOptions, scan_location};

    pub fn fixture() -> (tempfile::TempDir, Catalog, i64, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap().join("DCIM");
        std::fs::create_dir_all(root.join("2026-10-23_1/arw")).unwrap();
        std::fs::write(root.join("2026-10-23_1/arw/DSC0001.ARW"), b"r").unwrap();
        let mut cat = Catalog::open_in_memory().unwrap();
        let loc = locations::add(&cat, &root, None).unwrap();
        let loc = locations::get(&cat, loc).unwrap().unwrap();
        let scan = scan_location(&mut cat, &loc, &ScanOptions::default()).unwrap();
        crate::pv::assets::refresh(&mut cat, &scan.changed_folders).unwrap();
        let shoot = cat
            .conn()
            .query_row(
                "SELECT id FROM folders WHERE rel_path = '2026-10-23_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        (tmp, cat, shoot, root)
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;
    use crate::pv::scan::l0::{ScanOptions, scan_location};

    #[test]
    fn set_writes_the_shoot_file_and_survives_a_new_catalog() {
        let (_tmp, cat, shoot, root) = fixture();
        assert_eq!(get(&cat, shoot).unwrap(), Intent::Normal);
        set(&cat, shoot, Intent::Casual).unwrap();
        assert_eq!(get(&cat, shoot).unwrap(), Intent::Casual);
        let file = std::fs::read_to_string(root.join("2026-10-23_1").join(FILE_NAME)).unwrap();
        assert!(file.contains("\"casual\""), "{file}");
        // Only that file was written.
        let names: Vec<_> = std::fs::read_dir(root.join("2026-10-23_1"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");

        // A rebuilt catalog reads it back.
        cat.conn()
            .execute("UPDATE folders SET intent = NULL", [])
            .unwrap();
        assert_eq!(get(&cat, shoot).unwrap(), Intent::Casual);
    }

    #[test]
    fn offline_refuses() {
        let (_tmp, cat, shoot, root) = fixture();
        cat.conn()
            .execute(
                "UPDATE locations SET path = ?1",
                [root.parent().unwrap().join("gone").to_str().unwrap()],
            )
            .unwrap();
        assert!(set(&cat, shoot, Intent::Important).is_err());
    }

    #[test]
    fn the_scanner_ignores_the_shoot_file() {
        let (_tmp, mut cat, shoot, _root) = fixture();
        set(&cat, shoot, Intent::Important).unwrap();
        let loc = locations::list(&cat).unwrap().pop().unwrap();
        scan_location(&mut cat, &loc, &ScanOptions::default()).unwrap();
        let files: i64 = cat
            .conn()
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 1);
    }
}

#[cfg(test)]
mod slim_tests {
    use super::tests_support::*;
    use super::*;

    #[test]
    fn slim_review_and_intent_share_the_file() {
        let (_tmp, cat, shoot, root) = fixture();
        set(&cat, shoot, Intent::Important).unwrap();
        mark_slim_reviewed(&cat, shoot, 1234).unwrap();
        let text = std::fs::read_to_string(root.join("2026-10-23_1").join(FILE_NAME)).unwrap();
        assert!(
            text.contains("important") && text.contains("1234"),
            "{text}"
        );
        cat.conn()
            .execute(
                "UPDATE folders SET intent = NULL, slim_reviewed_at = NULL",
                [],
            )
            .unwrap();
        assert_eq!(get(&cat, shoot).unwrap(), Intent::Important);
        let at: Option<i64> = cat
            .conn()
            .query_row(
                "SELECT slim_reviewed_at FROM folders WHERE id = ?1",
                [shoot],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(at, Some(1234));
    }
}
