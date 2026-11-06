//! The Looks panel's catalogue (docs/IMPLEMENTATION_PLAN.md M3-07, PRD §6.4).
//!
//! Looks are ordinary LUT files; only their grouping is Pairview's. Bundled film emulations are
//! Film. In the app's `luts` folder, the file name decides: `My Sony` and `Sony …` are My Camera,
//! `Fujifilm …` and `Leica …` are Camera styles. Anything else is the owner's own LUT: it stays in
//! upstream's LUT control, not here.

use serde::Serialize;

use crate::lut_processing::LutEntry;

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
