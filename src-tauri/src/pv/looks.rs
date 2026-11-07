//! The Looks panel's catalogue (docs/IMPLEMENTATION_PLAN.md M3-07, PRD §6.4).
//!
//! Looks are ordinary LUT files; only their grouping is Pairview's. Bundled film emulations are
//! Film. In the app's `luts` folder, the file name decides: `My Sony` and `Sony …` are My Camera,
//! `Fujifilm …` and `Leica …` are Camera styles. Anything else is the owner's own LUT: it stays in
//! upstream's LUT control, not here.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::lut_processing::LutEntry;

/// The folders looks live in on this install, found once at startup.
static LOOK_DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// Finds this install's look folders. Their full paths differ between dev and packaged builds and
/// between machines; their own names don't.
pub fn remember_look_dirs(app: &AppHandle) {
    let mut dirs = Vec::new();
    for bundled in ["resources/pv_film_luts", "resources/film_luts"] {
        if let Ok(dir) = app
            .path()
            .resolve(bundled, tauri::path::BaseDirectory::Resource)
        {
            dirs.push(dir);
        }
    }
    if let Ok(data) = app.path().app_data_dir() {
        dirs.push(data.join("luts"));
    }
    let _ = LOOK_DIRS.set(dirs);
}

#[cfg(test)]
pub fn remember_dirs_for_test(dirs: Vec<PathBuf>) {
    let _ = LOOK_DIRS.set(dirs);
}

/// The file a recipe's look path means on this install.
///
/// Recipes store a look's full path, which stops being right when the app moves: a dev build to a
/// packaged one, or another Mac. Recipes aren't rewritten for this. A path that still exists is
/// used as it is, and one that doesn't is looked up here. Anything else comes back unchanged, and
/// the loader's failure renders as no look, as it would have before.
pub fn current_path(stored: &str) -> String {
    if Path::new(stored).exists() {
        return stored.to_string();
    }
    LOOK_DIRS
        .get()
        .and_then(|dirs| moved_to(stored, dirs))
        .map_or_else(|| stored.to_string(), |p| p.to_string_lossy().into_owned())
}

/// The same file in the folder with the same name. The folder is part of the match, because
/// `pv_film_luts` and upstream's `film_luts` hold different LUTs under the same file names.
/// Splits on both separators: a recipe may have been saved on Windows.
fn moved_to(stored: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let mut parts = stored.rsplit(['/', '\\']);
    let (file, folder) = (parts.next()?, parts.next()?);
    if file.is_empty() || file == "." || file == ".." {
        return None;
    }
    let dir = dirs
        .iter()
        .find(|d| d.file_name().is_some_and(|name| name == folder))?;
    let found = dir.join(file);
    found.is_file().then_some(found)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LookGroup {
    MyCamera,
    Film,
    CameraStyle,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Look {
    pub name: String,
    pub path: String,
    pub is_built_in: bool,
    pub group: LookGroup,
}

fn group_of(entry: &LutEntry) -> Option<LookGroup> {
    if entry.is_built_in {
        return Some(LookGroup::Film);
    }
    let name = entry.name.as_str();
    if name == "My Sony" || name.starts_with("Sony ") {
        Some(LookGroup::MyCamera)
    } else if name.starts_with("Fujifilm ") || name.starts_with("Leica ") {
        Some(LookGroup::CameraStyle)
    } else {
        None
    }
}

/// The looks among `entries`, in panel order: My Camera with My Sony first, then Film, then Camera
/// styles, alphabetical within each.
pub fn catalogue(entries: Vec<LutEntry>) -> Vec<Look> {
    let mut looks: Vec<Look> = entries
        .into_iter()
        .filter_map(|e| {
            group_of(&e).map(|group| Look {
                name: e.name,
                path: e.path,
                is_built_in: e.is_built_in,
                group,
            })
        })
        .collect();
    let rank = |g: LookGroup| match g {
        LookGroup::MyCamera => 0,
        LookGroup::Film => 1,
        LookGroup::CameraStyle => 2,
    };
    looks.sort_by_cached_key(|l| (rank(l.group), l.name != "My Sony", l.name.to_lowercase()));
    looks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lut(name: &str, is_built_in: bool) -> LutEntry {
        LutEntry {
            name: name.into(),
            path: format!("/luts/{name}.cube"),
            is_built_in,
        }
    }

    /// `pv_film_luts` and `film_luts` with a same-named file each, and `luts` with one of its own.
    fn install() -> (tempfile::TempDir, Vec<PathBuf>) {
        let root = tempfile::tempdir().unwrap();
        let dirs: Vec<PathBuf> = ["pv_film_luts", "film_luts", "luts"]
            .iter()
            .map(|name| root.path().join(name))
            .collect();
        for dir in &dirs {
            std::fs::create_dir(dir).unwrap();
            std::fs::write(
                dir.join("K-Portra 400.cube"),
                dir.to_string_lossy().as_bytes(),
            )
            .unwrap();
        }
        std::fs::write(dirs[2].join("My Sony.cube"), b"mine").unwrap();
        (root, dirs)
    }

    #[test]
    fn a_moved_look_is_found_in_the_folder_it_came_from() {
        let (_root, dirs) = install();
        let from_dev = "/Users/me/Code/app/src-tauri/target/debug/resources";
        for (dir, stored) in [
            (
                &dirs[0],
                format!("{from_dev}/pv_film_luts/K-Portra 400.cube"),
            ),
            (&dirs[1], format!("{from_dev}/film_luts/K-Portra 400.cube")),
            (
                &dirs[2],
                "/Users/old/Library/x/luts/K-Portra 400.cube".to_string(),
            ),
        ] {
            assert_eq!(
                moved_to(&stored, &dirs),
                Some(dir.join("K-Portra 400.cube")),
                "the same file name in another folder is a different look"
            );
        }
    }

    #[test]
    fn a_recipe_saved_on_windows_still_finds_its_look() {
        let (_root, dirs) = install();
        let stored = r"C:\Users\me\AppData\Roaming\pairview\luts\My Sony.cube";
        assert_eq!(moved_to(stored, &dirs), Some(dirs[2].join("My Sony.cube")));
    }

    #[test]
    fn anything_else_is_not_found() {
        let (_root, dirs) = install();
        for stored in [
            "/nowhere/Gone.cube",
            "/old/pv_film_luts/Gone.cube",
            "/old/some_other_folder/K-Portra 400.cube",
            "K-Portra 400.cube",
            "/old/luts/",
            "/old/luts/..",
            "content://media/luts/1",
        ] {
            assert_eq!(moved_to(stored, &dirs), None, "{stored}");
        }
    }

    #[test]
    fn a_path_that_still_exists_is_left_alone() {
        let (_root, dirs) = install();
        let stored = dirs[0].join("K-Portra 400.cube");
        let stored = stored.to_string_lossy();
        assert_eq!(current_path(&stored), stored);
        assert_eq!(current_path("/nowhere/Gone.cube"), "/nowhere/Gone.cube");
    }

    #[test]
    fn groups_and_order() {
        let looks = catalogue(vec![
            lut("Fujifilm Velvia", false),
            lut("K-Portra 400", true),
            lut("Sony ST", false),
            lut("F-Pro 400H", true),
            lut("My Sony", false),
            lut("Leica Monochrome", false),
            lut("Fujifilm Astia", false),
        ]);
        let names: Vec<_> = looks.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "My Sony",
                "Sony ST",
                "F-Pro 400H",
                "K-Portra 400",
                "Fujifilm Astia",
                "Fujifilm Velvia",
                "Leica Monochrome"
            ]
        );
        assert_eq!(looks[0].group, LookGroup::MyCamera);
        assert_eq!(looks[2].group, LookGroup::Film);
        assert_eq!(looks[6].group, LookGroup::CameraStyle);
    }

    #[test]
    fn an_approximate_style_is_still_a_camera_style() {
        let looks = catalogue(vec![lut("Fujifilm Velvia (approximate)", false)]);
        assert_eq!(looks[0].group, LookGroup::CameraStyle);
        assert_eq!(looks[0].name, "Fujifilm Velvia (approximate)");
    }

    #[test]
    fn the_owners_own_luts_stay_out_of_the_panel() {
        let looks = catalogue(vec![lut("Teal and Orange", false), lut("Sonyish", false)]);
        assert!(looks.is_empty(), "{looks:?}");
    }
}
