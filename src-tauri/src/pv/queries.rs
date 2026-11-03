//! Read-only catalog queries behind the frontend commands (docs/IMPLEMENTATION_PLAN.md M1-09).

use std::path::PathBuf;

use anyhow::{Result, bail};
use rusqlite::ToSql;
use serde::{Deserialize, Serialize};

use crate::pv::catalog::Catalog;
use crate::pv::metadata::{self, PhotoMeta};

// ---- Folder tree ------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderNode {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub rel_path: String,
    pub path: PathBuf,
    /// `arw/`, `hif/`, …: shown as part of the parent shoot, normally hidden in the tree.
    pub format_subdir: bool,
    pub date_hint: Option<String>,
    /// Photos in this shoot (0 for format sub-folders: their files count for the parent).
    pub photos: i64,
    /// Photos here and in every sub-folder.
    pub photos_recursive: i64,
}

/// Every folder of a location, parents before children.
pub fn folder_tree(cat: &Catalog, location_id: i64) -> Result<Vec<FolderNode>> {
    let mut stmt = cat.conn().prepare_cached(
        "SELECT f.id, f.parent_id, f.name, f.rel_path, f.format_subdir, f.date_hint, l.path,
                (SELECT COUNT(*) FROM photos p WHERE p.shoot_folder_id = f.id)
         FROM folders f JOIN locations l ON l.id = f.location_id
         WHERE f.location_id = ?1
         ORDER BY f.rel_path",
    )?;
    let mut nodes: Vec<FolderNode> = stmt
        .query_map([location_id], |r| {
            let rel: String = r.get(3)?;
            Ok(FolderNode {
                id: r.get(0)?,
                parent_id: r.get(1)?,
                name: r.get(2)?,
                path: join(&r.get::<_, String>(6)?, &rel, ""),
                rel_path: rel,
                format_subdir: r.get(4)?,
                date_hint: r.get(5)?,
                photos: r.get(7)?,
                photos_recursive: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    // Sorted by path, so children come after parents: add up from the end.
    let index: std::collections::HashMap<i64, usize> =
        nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
    for i in (0..nodes.len()).rev() {
        if let Some(parent) = nodes[i].parent_id.and_then(|p| index.get(&p).copied()) {
            nodes[parent].photos_recursive += nodes[i].photos_recursive;
        }
    }
    Ok(nodes)
}

// ---- Photo lists ------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Scope {
    /// A folder's shoot, or with `recursive` everything under it.
    #[serde(rename_all = "camelCase")]
    Folder { folder_id: i64, recursive: bool },
    /// Capture dates `from..=to` (`YYYY-MM-DD`), optionally one location.
    #[serde(rename_all = "camelCase")]
    Dates {
        from: String,
        to: String,
        location_id: Option<i64>,
    },
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    #[default]
    Time,
    Name,
    Rating,
    Size,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Filter {
    pub min_rating: Option<u8>,
    /// `none`, `pick`, `reject`.
    pub flags: Vec<String>,
    pub labels: Vec<String>,
    /// Photo states (`raw_display`, `display_only`, …).
    pub states: Vec<String>,
    pub keep_raw: Vec<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoItem {
    pub id: i64,
    pub state: String,
    pub stem: String,
    /// What the grid and editor use: the RAW if present (A4).
    pub representative_path: Option<PathBuf>,
    /// What the viewer shows (A5).
    pub display_path: Option<PathBuf>,
    /// What the grid thumbnail is made from (M1-07).
    pub thumbnail_path: Option<PathBuf>,
    pub raw_ext: Option<String>,
    pub display_ext: Option<String>,
    pub captured_at: Option<String>,
    pub date_uncertain: bool,
    pub guard_mismatch: bool,
    /// Bytes of all member files.
    pub size: i64,
    /// mtime of the representative file, Unix ms.
    pub modified: i64,
    pub camera: Option<String>,
    pub lens: Option<String>,
    #[serde(flatten)]
    pub meta: PhotoMeta,
}

pub fn list_photos(
    cat: &Catalog,
    scope: &Scope,
    sort: &Sort,
    filter: &Filter,
) -> Result<Vec<PhotoItem>> {
    let mut wheres: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn ToSql>> = Vec::new();
    let param = |v: Box<dyn ToSql>, params: &mut Vec<Box<dyn ToSql>>| {
        params.push(v);
        format!("?{}", params.len())
    };

    match scope {
        Scope::Folder {
            folder_id,
            recursive: false,
        } => {
            let p = param(Box::new(*folder_id), &mut params);
            wheres.push(format!("p.shoot_folder_id = {p}"));
        }
        Scope::Folder {
            folder_id,
            recursive: true,
        } => {
            let (location, rel): (i64, String) = cat.conn().query_row(
                "SELECT location_id, rel_path FROM folders WHERE id = ?1",
                [folder_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let l = param(Box::new(location), &mut params);
            wheres.push(format!("s.location_id = {l}"));
            if !rel.is_empty() {
                let exact = param(Box::new(rel.clone()), &mut params);
                // `GLOB` is case-sensitive and has no `_` wildcard, unlike `LIKE`.
                let prefix = param(Box::new(format!("{}/*", glob_escape(&rel))), &mut params);
                wheres.push(format!(
                    "(s.rel_path = {exact} OR s.rel_path GLOB {prefix})"
                ));
            }
        }
        Scope::Dates {
            from,
            to,
            location_id,
        } => {
            for d in [from, to] {
                if chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_err() {
                    bail!("not a date: {d:?}");
                }
            }
            let f = param(Box::new(from.clone()), &mut params);
            let t = param(Box::new(format!("{to}T99")), &mut params);
            wheres.push(format!("p.captured_at >= {f} AND p.captured_at < {t}"));
            if let Some(location) = location_id {
                let l = param(Box::new(*location), &mut params);
                wheres.push(format!("s.location_id = {l}"));
            }
        }
    }

    if let Some(min) = filter.min_rating {
        let p = param(Box::new(min), &mut params);
        wheres.push(format!("COALESCE(md.rating, 0) >= {p}"));
    }
    let mut any_of = |column: &str, values: &[String], params: &mut Vec<Box<dyn ToSql>>| {
        if values.is_empty() {
            return;
        }
        let list: Vec<String> = values
            .iter()
            .map(|v| param(Box::new(v.clone()), params))
            .collect();
        wheres.push(format!("{column} IN ({})", list.join(", ")));
    };
    any_of("COALESCE(md.flag, 'none')", &filter.flags, &mut params);
    any_of("md.label", &filter.labels, &mut params);
    any_of("p.state", &filter.states, &mut params);
    any_of(
        "COALESCE(md.keep_raw, 'auto')",
        &filter.keep_raw,
        &mut params,
    );
    if let Some(camera) = &filter.camera {
        let p = param(Box::new(camera.clone()), &mut params);
        wheres.push(format!("e.model = {p}"));
    }
    if let Some(lens) = &filter.lens {
        let p = param(Box::new(lens.clone()), &mut params);
        wheres.push(format!("e.lens = {p}"));
    }

    let name = "COALESCE(rf.name, df.name)";
    let time = "COALESCE(p.captured_at, '')";
    let keys = match sort.key {
        SortKey::Time => [time, name],
        SortKey::Name => [name, time],
        SortKey::Rating => ["COALESCE(md.rating, 0)", time],
        SortKey::Size => ["size", name],
    };
    let dir = if sort.descending { "DESC" } else { "ASC" };
    let order: Vec<String> = keys.iter().map(|k| format!("{k} {dir}")).collect();

    let sql = format!(
        "SELECT p.id, p.state, p.stem_key, p.captured_at, p.date_uncertain, p.guard_mismatch,
                l.path, rd.rel_path, rf.name, rf.ext, dd.rel_path, df.name, df.ext,
                COALESCE(md.rating, 0), COALESCE(md.flag, 'none'), md.label,
                COALESCE(md.keep_raw, 'auto'),
                (SELECT COALESCE(SUM(f.size), 0) FROM photo_members m JOIN files f ON f.id = m.file_id
                 WHERE m.photo_id = p.id AND m.role != 'sidecar') AS size,
                e.model, e.lens, COALESCE(rf.mtime, df.mtime, 0)
         FROM photos p
         JOIN folders s ON s.id = p.shoot_folder_id
         JOIN locations l ON l.id = s.location_id
         LEFT JOIN files rf ON rf.id = p.representative_file_id
         LEFT JOIN folders rd ON rd.id = rf.folder_id
         LEFT JOIN files df ON df.id = p.display_file_id
         LEFT JOIN folders dd ON dd.id = df.folder_id
         LEFT JOIN metadata md ON md.photo_id = p.id
         LEFT JOIN exif e ON e.file_id = p.representative_file_id
         WHERE {}
         ORDER BY {}, p.id",
        if wheres.is_empty() { "1".to_string() } else { wheres.join(" AND ") },
        order.join(", "),
    );
    let mut stmt = cat.conn().prepare(&sql)?;
    let refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt.query_map(refs.as_slice(), |r| {
        let location: String = r.get(6)?;
        let rep = r
            .get::<_, Option<String>>(8)?
            .map(|n| join(&location, &r.get::<_, String>(7).unwrap_or_default(), &n));
        let display = r
            .get::<_, Option<String>>(11)?
            .map(|n| join(&location, &r.get::<_, String>(10).unwrap_or_default(), &n));
        let state: String = r.get(1)?;
        let rep_ext: Option<String> = r.get(9)?;
        let display_ext: Option<String> = r.get(12)?;
        let thumbnail = if state == "edited" {
            display.clone().or(rep.clone())
        } else {
            rep.clone().or(display.clone())
        };
        let has_raw = state != "display_only" && rep.is_some() && rep != display;
        Ok(PhotoItem {
            id: r.get(0)?,
            stem: r.get(2)?,
            captured_at: r.get(3)?,
            date_uncertain: r.get(4)?,
            guard_mismatch: r.get(5)?,
            raw_ext: if has_raw { rep_ext } else { None },
            display_ext,
            representative_path: rep,
            display_path: display,
            thumbnail_path: thumbnail,
            meta: PhotoMeta {
                rating: r.get(13)?,
                flag: metadata::Flag::parse(&r.get::<_, String>(14)?).unwrap_or_default(),
                label: r.get(15)?,
                keep_raw: metadata::KeepRaw::parse(&r.get::<_, String>(16)?).unwrap_or_default(),
            },
            size: r.get(17)?,
            modified: r.get(20)?,
            camera: r.get(18)?,
            lens: r.get(19)?,
            state,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---- Paths ------------------------------------------------------------------------------------

/// The catalog folder at an absolute path, if it's inside a location and has been scanned.
/// Returns (folder id, is a format sub-folder).
pub fn folder_at_path(cat: &Catalog, path: &std::path::Path) -> Result<Option<(i64, bool)>> {
    for location in crate::pv::locations::list(cat)? {
        let Ok(rel) = path.strip_prefix(&location.path) else {
            continue;
        };
        let rel: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let found = cat
            .conn()
            .query_row(
                "SELECT id, format_subdir FROM folders WHERE location_id = ?1 AND rel_path = ?2",
                (location.id, rel.join("/")),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(None)
}

/// RapidRAW sidecars of a Photo's representative file, as upstream's listing expects them:
/// `None` for `NAME.rrdata`, `Some(id)` for each virtual copy `NAME.<id>.rrdata`.
pub fn rrdata_copies(cat: &Catalog, photo_id: i64, file_name: &str) -> Result<Vec<Option<String>>> {
    let mut stmt = cat.conn().prepare_cached(
        "SELECT f.name FROM photo_members m JOIN files f ON f.id = m.file_id
         WHERE m.photo_id = ?1 AND m.role = 'sidecar' AND f.ext = 'rrdata'",
    )?;
    let names: Vec<String> = stmt
        .query_map([photo_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut copies: Vec<Option<String>> = names
        .iter()
        .filter_map(|n| crate::file_management::parse_sidecar_name(n))
        .filter(|(source, _)| *source == file_name)
        .map(|(_, id)| id.map(str::to_string))
        .collect();
    copies.sort();
    Ok(copies)
}

// ---- Dates ------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayCount {
    /// `YYYY-MM-DD`; the frontend groups days into months and years.
    pub date: String,
    pub photos: i64,
    /// Photos that day whose date may be wrong (EXIF far from the folder name, or mtime only).
    pub uncertain: i64,
}

pub fn dates(cat: &Catalog, location_id: Option<i64>) -> Result<Vec<DayCount>> {
    let mut stmt = cat.conn().prepare_cached(
        "SELECT substr(p.captured_at, 1, 10) AS day, COUNT(*), SUM(p.date_uncertain)
         FROM photos p JOIN folders s ON s.id = p.shoot_folder_id
         WHERE p.captured_at IS NOT NULL AND (?1 IS NULL OR s.location_id = ?1)
         GROUP BY day ORDER BY day",
    )?;
    let rows = stmt.query_map([location_id], |r| {
        Ok(DayCount {
            date: r.get(0)?,
            photos: r.get(1)?,
            uncertain: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---- Photo detail -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberFile {
    pub path: PathBuf,
    /// `raw`, `display`, `extra` or `sidecar`.
    pub role: String,
    pub size: i64,
    pub mtime: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExifSummary {
    pub date_original: Option<String>,
    pub offset_original: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub iso: Option<i64>,
    pub exposure_s: Option<f64>,
    pub f_number: Option<f64>,
    pub focal_mm: Option<f64>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoDetail {
    pub photo: PhotoItem,
    pub files: Vec<MemberFile>,
    pub exif: Option<ExifSummary>,
}

pub fn photo_detail(cat: &Catalog, photo_id: i64) -> Result<Option<PhotoDetail>> {
    let shoot: Option<i64> = cat
        .conn()
        .query_row(
            "SELECT shoot_folder_id FROM photos WHERE id = ?1",
            [photo_id],
            |r| r.get(0),
        )
        .ok();
    let Some(shoot) = shoot else {
        return Ok(None);
    };
    let scope = Scope::Folder {
        folder_id: shoot,
        recursive: false,
    };
    let Some(photo) = list_photos(cat, &scope, &Sort::default(), &Filter::default())?
        .into_iter()
        .find(|p| p.id == photo_id)
    else {
        return Ok(None);
    };

    let mut stmt = cat.conn().prepare_cached(
        "SELECT m.file_id, m.role, f.size, f.mtime FROM photo_members m
         JOIN files f ON f.id = m.file_id WHERE m.photo_id = ?1
         ORDER BY CASE m.role WHEN 'raw' THEN 0 WHEN 'display' THEN 1 WHEN 'extra' THEN 2 ELSE 3 END,
                  f.name",
    )?;
    let rows: Vec<(i64, String, i64, i64)> = stmt
        .query_map([photo_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut files = Vec::new();
    for (file_id, role, size, mtime) in rows {
        files.push(MemberFile {
            path: crate::pv::assets::file_path(cat, file_id)?,
            role,
            size,
            mtime,
        });
    }

    let exif = cat
        .conn()
        .query_row(
            "SELECT date_original, offset_original, make, model, lens, width, height, iso,
                    exposure_s, f_number, focal_mm, gps_lat, gps_lon
             FROM exif WHERE file_id = (SELECT COALESCE(representative_file_id, display_file_id)
                                        FROM photos WHERE id = ?1)",
            [photo_id],
            |r| {
                Ok(ExifSummary {
                    date_original: r.get(0)?,
                    offset_original: r.get(1)?,
                    make: r.get(2)?,
                    model: r.get(3)?,
                    lens: r.get(4)?,
                    width: r.get(5)?,
                    height: r.get(6)?,
                    iso: r.get(7)?,
                    exposure_s: r.get(8)?,
                    f_number: r.get(9)?,
                    focal_mm: r.get(10)?,
                    gps_lat: r.get(11)?,
                    gps_lon: r.get(12)?,
                })
            },
        )
        .ok();
    Ok(Some(PhotoDetail { photo, files, exif }))
}

fn join(location: &str, rel: &str, name: &str) -> PathBuf {
    let mut path = PathBuf::from(location);
    if !rel.is_empty() {
        path.push(rel);
    }
    if !name.is_empty() {
        path.push(name);
    }
    path
}

fn glob_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '*' | '?' | '[' => format!("[{c}]"),
            c => c.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pv::assets;
    use crate::pv::locations;
    use crate::pv::scan::l0::{ScanOptions, scan_location};
    use std::fs;

    fn fixture() -> (tempfile::TempDir, Catalog, i64) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("DCIM");
        for (f, size) in [
            ("2026-10-24_10361024/arw/DSC03545.ARW", 400),
            ("2026-10-24_10361024/hif/DSC03545.HIF", 100),
            ("2026-10-24_10361024/arw/DSC03546.ARW", 400),
            ("2026-10-24_10361024/hif/DSC03546.HIF", 100),
            ("10561031/DSC03600.HIF", 90),
            ("10561031/DSC03601.ARW", 500),
            ("trips/[2026] kyoto*/DSC00001.JPG", 50),
            ("trips_x/DSC00002.JPG", 50),
        ] {
            let p = root.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, vec![0u8; size]).unwrap();
        }
        let mut cat = Catalog::open_in_memory().unwrap();
        let loc = locations::add(&cat, &root, Some("NAS")).unwrap();
        let loc = locations::get(&cat, loc).unwrap().unwrap();
        let scan = scan_location(&mut cat, &loc, &ScanOptions::default()).unwrap();
        assets::refresh(&mut cat, &scan.changed_folders).unwrap();
        // Dates and ratings as L1 and M1-08 would set them.
        for (stem, at, rating, flag, label) in [
            ("dsc03545", "2026-10-24T09:00:00", 5, "pick", Some("red")),
            ("dsc03546", "2026-10-24T09:01:00", 2, "none", None),
            ("dsc03600", "2026-10-31T18:00:00", 0, "reject", None),
            ("dsc03601", "2026-10-31T18:05:00", 3, "none", Some("green")),
        ] {
            cat.conn()
                .execute(
                    "UPDATE photos SET captured_at = ?2 WHERE stem_key = ?1",
                    (stem, at),
                )
                .unwrap();
            cat.conn()
                .execute(
                    "INSERT INTO metadata (photo_id, rating, flag, label, updated_at)
                     SELECT id, ?2, ?3, ?4, 0 FROM photos WHERE stem_key = ?1",
                    (stem, rating, flag, label),
                )
                .unwrap();
        }
        (tmp, cat, loc.id)
    }

    fn folder(cat: &Catalog, rel: &str) -> i64 {
        cat.conn()
            .query_row("SELECT id FROM folders WHERE rel_path = ?1", [rel], |r| {
                r.get(0)
            })
            .unwrap()
    }

    fn stems(items: &[PhotoItem]) -> Vec<&str> {
        items.iter().map(|p| p.stem.as_str()).collect()
    }

    #[test]
    fn tree_counts_photos_per_shoot_and_recursively() {
        let (_tmp, cat, loc) = fixture();
        let tree = folder_tree(&cat, loc).unwrap();
        let get = |rel: &str| tree.iter().find(|n| n.rel_path == rel).unwrap();
        assert_eq!(get("").photos_recursive, 6);
        assert_eq!(get("2026-10-24_10361024").photos, 2);
        assert_eq!(get("2026-10-24_10361024/arw").photos, 0);
        assert!(get("2026-10-24_10361024/arw").format_subdir);
        assert_eq!(
            get("2026-10-24_10361024").date_hint.as_deref(),
            Some("2026-10-24")
        );
        assert_eq!(get("trips").photos_recursive, 1);
        assert!(get("10561031").path.ends_with("DCIM/10561031"));
    }

    #[test]
    fn folder_scope_plain_and_recursive() {
        let (_tmp, cat, _) = fixture();
        let sort = Sort::default();
        let f = Filter::default();
        let shoot = Scope::Folder {
            folder_id: folder(&cat, "2026-10-24_10361024"),
            recursive: false,
        };
        let items = list_photos(&cat, &shoot, &sort, &f).unwrap();
        assert_eq!(stems(&items), ["dsc03545", "dsc03546"]);
        let p = &items[0];
        assert!(
            p.representative_path
                .as_ref()
                .unwrap()
                .ends_with("arw/DSC03545.ARW")
        );
        assert!(
            p.display_path
                .as_ref()
                .unwrap()
                .ends_with("hif/DSC03545.HIF")
        );
        assert_eq!(p.thumbnail_path, p.representative_path);
        assert_eq!(
            (p.raw_ext.as_deref(), p.display_ext.as_deref()),
            (Some("arw"), Some("hif"))
        );
        assert_eq!(p.size, 500);
        assert_eq!(
            (p.meta.rating, p.meta.flag, p.meta.label.as_deref()),
            (5, metadata::Flag::Pick, Some("red"))
        );

        // `trips` must not pick up `trips_x`, and glob characters in names are literal.
        let trips = Scope::Folder {
            folder_id: folder(&cat, "trips"),
            recursive: true,
        };
        assert_eq!(
            stems(&list_photos(&cat, &trips, &sort, &f).unwrap()),
            ["dsc00001"]
        );
        let all = Scope::Folder {
            folder_id: folder(&cat, ""),
            recursive: true,
        };
        assert_eq!(list_photos(&cat, &all, &sort, &f).unwrap().len(), 6);
    }

    #[test]
    fn sorting_and_filtering() {
        let (_tmp, cat, _) = fixture();
        let all = Scope::Folder {
            folder_id: folder(&cat, ""),
            recursive: true,
        };
        let by = |key, descending, filter: Filter| {
            let items = list_photos(&cat, &all, &Sort { key, descending }, &filter).unwrap();
            items.into_iter().map(|p| p.stem).collect::<Vec<_>>()
        };
        assert_eq!(
            by(
                SortKey::Rating,
                true,
                Filter {
                    min_rating: Some(2),
                    ..Default::default()
                }
            ),
            ["dsc03545", "dsc03601", "dsc03546"]
        );
        assert_eq!(
            by(
                SortKey::Time,
                false,
                Filter {
                    flags: vec!["reject".into()],
                    ..Default::default()
                }
            ),
            ["dsc03600"]
        );
        assert_eq!(
            by(
                SortKey::Name,
                false,
                Filter {
                    states: vec!["display_only".into()],
                    ..Default::default()
                }
            ),
            ["dsc00001", "dsc00002", "dsc03600"]
        );
        assert_eq!(
            by(
                SortKey::Size,
                true,
                Filter {
                    labels: vec!["green".into(), "red".into()],
                    ..Default::default()
                }
            ),
            ["dsc03601", "dsc03545"]
        );
    }

    #[test]
    fn dates_and_date_scope() {
        let (_tmp, cat, loc) = fixture();
        let days = dates(&cat, Some(loc)).unwrap();
        let day_of = |d: &DayCount| (d.date.clone(), d.photos, d.uncertain);
        assert_eq!(day_of(&days[0]), ("2026-10-24".into(), 2, 0));
        assert_eq!(day_of(&days[1]), ("2026-10-31".into(), 2, 0));
        // The two JPGs have no EXIF: dated by mtime (today) and marked uncertain.
        assert_eq!((days[2].photos, days[2].uncertain), (2, 2));
        let day = Scope::Dates {
            from: "2026-10-31".into(),
            to: "2026-10-31".into(),
            location_id: None,
        };
        let items = list_photos(&cat, &day, &Sort::default(), &Filter::default()).unwrap();
        assert_eq!(stems(&items), ["dsc03600", "dsc03601"]);
        let bad = Scope::Dates {
            from: "x".into(),
            to: "2026-10-31".into(),
            location_id: None,
        };
        assert!(list_photos(&cat, &bad, &Sort::default(), &Filter::default()).is_err());
    }

    #[test]
    fn folders_by_path_and_rrdata_copies() {
        let (tmp, cat, _) = fixture();
        let root = tmp.path().join("DCIM").canonicalize().unwrap();
        let shoot = folder_at_path(&cat, &root.join("2026-10-24_10361024")).unwrap();
        assert_eq!(shoot, Some((folder(&cat, "2026-10-24_10361024"), false)));
        assert_eq!(
            folder_at_path(&cat, &root.join("2026-10-24_10361024/arw"))
                .unwrap()
                .map(|f| f.1),
            Some(true)
        );
        assert_eq!(
            folder_at_path(&cat, &root).unwrap().map(|f| f.0),
            Some(folder(&cat, ""))
        );
        assert_eq!(folder_at_path(&cat, &root.join("nope")).unwrap(), None);
        assert_eq!(
            folder_at_path(&cat, std::path::Path::new("/elsewhere")).unwrap(),
            None
        );

        // Sidecars, including a virtual copy, become upstream's copy list.
        let dir = root.join("10561031");
        for name in [
            "DSC03601.ARW.rrdata",
            "DSC03601.ARW.0a1b2c.rrdata",
            "DSC03601.xmp",
        ] {
            fs::write(dir.join(name), b"{}").unwrap();
        }
        let mut cat = cat;
        let loc = locations::list(&cat).unwrap().remove(0);
        let scan = scan_location(
            &mut cat,
            &loc,
            &ScanOptions {
                full: true,
                ..Default::default()
            },
        )
        .unwrap();
        assets::refresh(&mut cat, &scan.changed_folders).unwrap();
        let photo: i64 = cat
            .conn()
            .query_row(
                "SELECT id FROM photos WHERE stem_key = 'dsc03601'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            rrdata_copies(&cat, photo, "DSC03601.ARW").unwrap(),
            [None, Some("0a1b2c".to_string())]
        );
    }

    #[test]
    fn detail_lists_files() {
        let (_tmp, cat, _) = fixture();
        let id: i64 = cat
            .conn()
            .query_row(
                "SELECT id FROM photos WHERE stem_key = 'dsc03545'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let d = photo_detail(&cat, id).unwrap().unwrap();
        let roles: Vec<(&str, i64)> = d.files.iter().map(|f| (f.role.as_str(), f.size)).collect();
        assert_eq!(roles, [("raw", 400), ("display", 100)]);
        assert_eq!(d.photo.id, id);
        assert!(photo_detail(&cat, 999).unwrap().is_none());
    }

    #[test]
    fn scope_and_sort_parse_from_frontend_json() {
        let s: Scope =
            serde_json::from_str(r#"{"kind":"folder","folderId":3,"recursive":true}"#).unwrap();
        assert!(matches!(
            s,
            Scope::Folder {
                folder_id: 3,
                recursive: true
            }
        ));
        let s: Scope =
            serde_json::from_str(r#"{"kind":"dates","from":"2026-10-01","to":"2026-10-31"}"#)
                .unwrap();
        assert!(matches!(
            s,
            Scope::Dates {
                location_id: None,
                ..
            }
        ));
        let sort: Sort = serde_json::from_str(r#"{"key":"rating","descending":true}"#).unwrap();
        assert_eq!((sort.key, sort.descending), (SortKey::Rating, true));
        let f: Filter = serde_json::from_str(r#"{"minRating":3,"keepRaw":["keep"]}"#).unwrap();
        assert_eq!((f.min_rating, f.keep_raw.len()), (Some(3), 1));
        let item = serde_json::to_value(PhotoItem {
            id: 1,
            state: "raw_display".into(),
            stem: "x".into(),
            representative_path: None,
            display_path: None,
            thumbnail_path: None,
            raw_ext: None,
            display_ext: None,
            captured_at: None,
            date_uncertain: false,
            guard_mismatch: false,
            size: 0,
            modified: 0,
            camera: None,
            lens: None,
            meta: PhotoMeta::default(),
        })
        .unwrap();
        // Metadata is flattened into the item for the grid.
        assert_eq!(item["rating"], 0);
        assert_eq!(item["keepRaw"], "auto");
    }
}
