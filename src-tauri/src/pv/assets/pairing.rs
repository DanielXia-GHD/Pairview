//! Groups a shoot's files into Photos by basename (PRD §5.3). Pure: no I/O.

use std::collections::{BTreeMap, BTreeSet};

/// Longest counter a phone/transfer app appends to the camera's name: `DSC07714` → `DSC07714439.jpg`.
const MAX_SUFFIX_DIGITS: usize = 4;

/// The RAW stem a display copy belongs to: its own stem when a RAW has it, else the longest RAW
/// stem it starts with followed only by 1–4 digits (`dsc07714439` → `dsc07714`). The timestamp
/// guard still checks the pair, so a wrong guess is split again once EXIF is read.
fn display_key<'a>(stem: &'a str, raw_stems: &BTreeSet<&'a str>) -> &'a str {
    if raw_stems.contains(stem) {
        return stem;
    }
    (1..=MAX_SUFFIX_DIGITS.min(stem.len().saturating_sub(1)))
        .map(|n| stem.split_at(stem.len() - n))
        .find(|(prefix, suffix)| {
            suffix.bytes().all(|b| b.is_ascii_digit()) && raw_stems.contains(prefix)
        })
        .map_or(stem, |(prefix, _)| prefix)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Raw,
    Display,
    Sidecar,
}

impl Kind {
    pub fn from_catalog(kind: &str) -> Option<Self> {
        match kind {
            "raw" => Some(Self::Raw),
            "display" => Some(Self::Display),
            "sidecar" => Some(Self::Sidecar),
            _ => None, // Videos become their own items in M5.
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileRef {
    pub id: i64,
    pub name: String,
    pub stem_key: String,
    pub ext: String,
    pub kind: Kind,
}

/// The EXIF timestamp guard's answer for a RAW + display pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    Match,
    Mismatch,
    /// EXIF not read yet. The pair is shown as one Photo; destructive actions check again.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedPhoto {
    pub stem_key: String,
    pub raw: Option<i64>,
    pub display: Option<i64>,
    /// More copies of the same frame (a JPG next to the HIF, a second RAW): listed as versions.
    pub extras: Vec<i64>,
    pub sidecars: Vec<i64>,
    pub guard_mismatch: bool,
}

/// Which display copy the viewer shows when there are several: the camera's 10-bit HEIF first.
fn display_rank(ext: &str) -> u8 {
    match ext {
        "hif" | "heif" | "heic" => 0,
        "jpg" | "jpeg" => 1,
        _ => 2,
    }
}

/// Plans the Photos of one shoot. `guard(raw, display)` reports whether a pair's timestamps
/// agree; a mismatch splits the pair into two Photos, both flagged.
pub fn plan(files: &[FileRef], guard: impl Fn(i64, i64) -> Guard) -> Vec<PlannedPhoto> {
    let raw_stems: BTreeSet<&str> = files
        .iter()
        .filter(|f| f.kind == Kind::Raw)
        .map(|f| f.stem_key.as_str())
        .collect();
    let mut by_stem: BTreeMap<&str, (Vec<&FileRef>, Vec<&FileRef>)> = BTreeMap::new();
    for f in files {
        match f.kind {
            Kind::Raw => by_stem.entry(&f.stem_key).or_default().0.push(f),
            Kind::Display => by_stem
                .entry(display_key(&f.stem_key, &raw_stems))
                .or_default()
                .1
                .push(f),
            Kind::Sidecar => {}
        }
    }

    let mut photos = Vec::new();
    for (stem, (mut raws, mut displays)) in by_stem {
        raws.sort_by(|a, b| a.name.cmp(&b.name));
        displays
            .sort_by(|a, b| (display_rank(&a.ext), &a.name).cmp(&(display_rank(&b.ext), &b.name)));
        let raw = raws.first().map(|f| f.id);
        let display = displays.first().map(|f| f.id);
        let extras: Vec<i64> = raws
            .iter()
            .skip(1)
            .chain(displays.iter().skip(1))
            .map(|f| f.id)
            .collect();

        let mismatch =
            matches!((raw, display), (Some(r), Some(d)) if guard(r, d) == Guard::Mismatch);
        if mismatch {
            // Extras stay with the display copy, which is what they're copies of.
            photos.push(PlannedPhoto {
                stem_key: stem.to_string(),
                raw,
                display: None,
                extras: Vec::new(),
                sidecars: Vec::new(),
                guard_mismatch: true,
            });
            photos.push(PlannedPhoto {
                stem_key: stem.to_string(),
                raw: None,
                display,
                extras,
                sidecars: Vec::new(),
                guard_mismatch: true,
            });
        } else {
            photos.push(PlannedPhoto {
                stem_key: stem.to_string(),
                raw,
                display,
                extras,
                sidecars: Vec::new(),
                guard_mismatch: false,
            });
        }
    }

    attach_sidecars(files, &mut photos);
    photos
}

/// `DSC0001.xmp` belongs to the Photo with a file named `DSC0001.*`. `DSC0001.ARW.rrdata` /
/// `DSC0001.ARW.xmp` belong to the Photo that has the file `DSC0001.ARW`.
fn attach_sidecars(files: &[FileRef], photos: &mut [PlannedPhoto]) {
    let name_of = |id: i64| {
        files
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.name.to_lowercase())
    };
    let stem_of = |id: i64| files.iter().find(|f| f.id == id).map(|f| f.stem_key.as_str());
    let members = |p: &PlannedPhoto| {
        [p.raw, p.display]
            .into_iter()
            .chain(p.extras.iter().copied().map(Some))
            .flatten()
            .collect::<Vec<_>>()
    };
    for sidecar in files.iter().filter(|f| f.kind == Kind::Sidecar) {
        // RapidRAW virtual copies: `DSC0001.ARW.0a1b2c.rrdata` belongs to `DSC0001.ARW`.
        let file_key = if sidecar.ext == "rrdata" {
            crate::file_management::parse_sidecar_name(&sidecar.name)
                .map_or(sidecar.stem_key.clone(), |(source, _)| {
                    source.to_lowercase()
                })
        } else {
            sidecar.stem_key.clone()
        };
        let owner = photos
            .iter()
            .position(|p| {
                members(p)
                    .into_iter()
                    .any(|id| name_of(id).as_deref() == Some(file_key.as_str()))
            })
            .or_else(|| {
                // A bare-stem XMP describes the whole Photo; with a guard split it goes with
                // the RAW, where Lightroom/Capture One put it too.
                photos.iter().position(|p| p.stem_key == sidecar.stem_key)
            })
            .or_else(|| {
                // `DSC07714439.xmp` next to a suffixed JPG that paired with `DSC07714.ARW`.
                photos.iter().position(|p| {
                    members(p)
                        .into_iter()
                        .any(|id| stem_of(id) == Some(sidecar.stem_key.as_str()))
                })
            });
        if let Some(i) = owner {
            photos[i].sidecars.push(sidecar.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pv::scan::l0::{file_kind, split_name};

    pub fn files(names: &[&str]) -> Vec<FileRef> {
        names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let (stem, ext) = split_name(name);
                FileRef {
                    id: i as i64 + 1,
                    name: name.to_string(),
                    stem_key: stem.to_lowercase(),
                    ext: ext.to_lowercase(),
                    kind: Kind::from_catalog(file_kind(ext).unwrap()).unwrap(),
                }
            })
            .collect()
    }

    fn summary(photos: &[PlannedPhoto], files: &[FileRef]) -> Vec<String> {
        let name = |id: Option<i64>| {
            id.and_then(|id| files.iter().find(|f| f.id == id))
                .map_or("-".to_string(), |f| f.name.clone())
        };
        photos
            .iter()
            .map(|p| {
                let mut s = format!("{} {}", name(p.raw), name(p.display));
                for id in p.extras.iter().chain(&p.sidecars) {
                    s += &format!(" +{}", name(Some(*id)));
                }
                if p.guard_mismatch {
                    s += " !";
                }
                s
            })
            .collect()
    }

    fn run(names: &[&str]) -> Vec<String> {
        let f = files(names);
        summary(&plan(&f, |_, _| Guard::Unknown), &f)
    }

    #[test]
    fn pairs_by_basename_ignoring_case() {
        assert_eq!(
            run(&["DSC0001.ARW", "dsc0001.hif", "DSC0002.arw", "DSC0002.JPG"]),
            ["DSC0001.ARW dsc0001.hif", "DSC0002.arw DSC0002.JPG"]
        );
    }

    #[test]
    fn singles_stay_single() {
        assert_eq!(
            run(&["DSC0001.ARW", "DSC0002.HIF"]),
            ["DSC0001.ARW -", "- DSC0002.HIF"]
        );
    }

    #[test]
    fn hif_is_the_display_copy_and_jpg_an_extra() {
        assert_eq!(
            run(&["DSC0001.JPG", "DSC0001.ARW", "DSC0001.HIF"]),
            ["DSC0001.ARW DSC0001.HIF +DSC0001.JPG"]
        );
    }

    #[test]
    fn sidecars_follow_their_file() {
        assert_eq!(
            run(&[
                "DSC0001.ARW",
                "DSC0001.HIF",
                "DSC0001.ARW.rrdata",
                "DSC0001.HIF.rrdata",
                "DSC0001.ARW.0a1b2c.rrdata",
                "DSC0001.xmp",
                "DSC0009.xmp",
            ]),
            [
                "DSC0001.ARW DSC0001.HIF +DSC0001.ARW.rrdata +DSC0001.HIF.rrdata +DSC0001.ARW.0a1b2c.rrdata +DSC0001.xmp"
            ]
        );
    }

    #[test]
    fn display_copy_with_a_transfer_counter_pairs_with_its_raw() {
        // Older shoots on the NAS: JPGs got a counter appended after the camera number.
        assert_eq!(
            run(&[
                "DSC07714.ARW",
                "DSC07714.xmp",
                "DSC07714439.jpg",
                "DSC037651136.jpg",
                "DSC03765.ARW",
                "DSC037651136.xmp",
            ]),
            [
                "DSC03765.ARW DSC037651136.jpg +DSC037651136.xmp",
                "DSC07714.ARW DSC07714439.jpg +DSC07714.xmp"
            ]
        );
    }

    #[test]
    fn exact_names_win_and_only_digits_count_as_a_counter() {
        assert_eq!(
            run(&[
                "DSC0001.ARW",
                "DSC00012.ARW",
                "DSC00012.JPG",   // exact match, not DSC0001 + "2"
                "DSC0001-1.JPG",  // not digits only
                "DSC0001x12.JPG", // not digits only
                "DSC0001234567.JPG", // 5+ digits after DSC00012 / DSC0001
                "IMG1234.JPG",
            ]),
            [
                "DSC0001.ARW -",
                "- DSC0001-1.JPG",
                "DSC00012.ARW DSC00012.JPG",
                "- DSC0001234567.JPG",
                "- DSC0001x12.JPG",
                "- IMG1234.JPG"
            ]
        );
    }

    #[test]
    fn suffixed_pair_is_still_guarded() {
        let f = files(&["DSC07714.ARW", "DSC07714439.jpg"]);
        let photos = plan(&f, |_, _| Guard::Mismatch);
        assert_eq!(
            summary(&photos, &f),
            ["DSC07714.ARW - !", "- DSC07714439.jpg !"]
        );
    }

    #[test]
    fn guard_mismatch_splits_the_pair() {
        let f = files(&["DSC0001.ARW", "DSC0001.HIF", "DSC0001.JPG", "DSC0001.xmp"]);
        let photos = plan(&f, |raw, display| {
            assert_eq!((raw, display), (1, 2));
            Guard::Mismatch
        });
        assert_eq!(
            summary(&photos, &f),
            [
                "DSC0001.ARW - +DSC0001.xmp !",
                "- DSC0001.HIF +DSC0001.JPG !"
            ]
        );
    }
}
