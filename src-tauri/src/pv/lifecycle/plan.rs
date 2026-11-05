//! Which files a Photo-level delete moves (PRD §6.6: Delete photo · Keep HIF only · Keep RAW only).
//!
//! A plan is computed from the catalog and changes nothing, so the delete dialog can show what
//! will go and how much space it frees before the owner confirms. `lifecycle::apply` re-checks
//! every file on disk before moving anything.
//!
//! Pairs are only what the pairing engine made a Photo: a RAW and a display copy split by the
//! timestamp guard are two Photos, so a delete of one never reaches the other.

use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::OptionalExtension;
use serde::Serialize;

use super::manifest::{ManifestPhoto, Reason};
use crate::pv::catalog::Catalog;

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

/// location, shoot folder, shoot rel path, stem, captured_at, keep_raw, rating.
type PhotoRow = (i64, i64, String, String, Option<String>, String, i64);

struct Member {
    file_id: i64,
    role: String,
    folder: String,
    name: String,
    size: u64,
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

/// Plans a delete of `photo_ids`. Photos the scope doesn't apply to are listed in `skipped`.
pub fn plan_photos(cat: &Catalog, photo_ids: &[i64], scope: Scope) -> Result<Plan> {
    let (kind, reason) = match scope {
        Scope::KeepDisplay => ("drop_raw", Reason::Slimmed),
        Scope::Photo | Scope::KeepRaw => ("delete", Reason::Deleted),
    };
    let mut by_location: BTreeMap<i64, OpPlan> = BTreeMap::new();
    let mut skipped = Vec::new();

    let mut seen = std::collections::HashSet::new();
    for &photo_id in photo_ids.iter().filter(|&&id| seen.insert(id)) {
        let row: Option<PhotoRow> = cat
            .conn()
            .query_row(
                "SELECT d.location_id, p.shoot_folder_id, d.rel_path, p.stem_key, p.captured_at,
                        COALESCE(md.keep_raw, 'auto'), COALESCE(md.rating, 0)
                 FROM photos p JOIN folders d ON d.id = p.shoot_folder_id
                 LEFT JOIN metadata md ON md.photo_id = p.id
                 WHERE p.id = ?1",
                [photo_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((location_id, shoot_id, shoot_rel, stem, captured_at, keep_raw, rating)) = row
        else {
            skipped.push(Skipped {
                photo_id,
                reason: SkipReason::Gone,
            });
            continue;
        };

        let members = members(cat, photo_id)?;
        let primaries: Vec<&Member> = members.iter().filter(|m| m.is_primary()).collect();
        let has = |role: &str| primaries.iter().any(|m| m.role == role);
        let has_display = primaries.iter().any(|m| m.role != "raw");

        // Which primaries go.
        let drops: fn(&Member) -> bool = match scope {
            Scope::Photo => |_| true,
            Scope::KeepDisplay => |m| m.role == "raw",
            Scope::KeepRaw => |m| m.role != "raw",
        };
        let refusal = match scope {
            Scope::Photo => None,
            Scope::KeepDisplay if !has("raw") => Some(SkipReason::NoRaw),
            Scope::KeepDisplay if !has_display => Some(SkipReason::NoDisplay),
            Scope::KeepDisplay if keep_raw == "keep" => Some(SkipReason::Locked),
            Scope::KeepRaw if !has("raw") => Some(SkipReason::NoRaw),
            Scope::KeepRaw if !has_display => Some(SkipReason::NoDisplay),
            _ => None,
        };
        if let Some(reason) = refusal {
            skipped.push(Skipped { photo_id, reason });
            continue;
        }

        let kept: Vec<&Member> = primaries.iter().copied().filter(|m| !drops(m)).collect();
        let kept_folder = |folder: &str| kept.iter().any(|m| m.folder == folder);
        let mut going: Vec<&Member> = Vec::new();
        let mut photo_xmps_going: Vec<&Member> = Vec::new();
        for m in &members {
            let goes = if m.is_primary() {
                drops(m)
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
                    None => scope == Scope::Photo,
                }
            };
            if goes {
                going.push(m);
            }
        }

        let op = by_location.entry(location_id).or_insert_with(|| OpPlan {
            location_id,
            kind,
            reason,
            files: Vec::new(),
            photos: Vec::new(),
            xmp_copies: Vec::new(),
            shoots: Vec::new(),
        });

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
        op.photos.push(ManifestPhoto {
            shoot: shoot_rel,
            stem,
            captured_at,
            rating,
        });
        for m in going {
            op.files.push(PlannedFile {
                file_id: Some(m.file_id),
                rel: m.rel(),
                size: m.size,
                role: m.role.clone(),
                photo: Some(index),
            });
        }
        if !op.shoots.contains(&shoot_id) {
            op.shoots.push(shoot_id);
        }
    }

    Ok(Plan {
        ops: by_location
            .into_values()
            .filter(|o| !o.files.is_empty())
            .collect(),
        skipped,
    })
}

fn members(cat: &Catalog, photo_id: i64) -> Result<Vec<Member>> {
    let mut stmt = cat.conn().prepare_cached(
        "SELECT f.id, m.role, d.rel_path, f.name, f.size
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
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}
