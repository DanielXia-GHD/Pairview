//! Photo states and badges (PRD §5.2).

use super::pairing::PlannedPhoto;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// `RAW+HIF` / `RAW+JPG`.
    RawDisplay,
    /// `✎`: the display copy was rendered from the RAW (set by the edit loop, M3).
    Edited,
    /// `HIF` / `JPG` only.
    DisplayOnly,
    /// `RAW` only, as shot (or kept that way on purpose).
    RawOnly,
    /// `⚠ RAW`: the display copy disappeared outside the app (PRD §6.6). Never deleted
    /// automatically; the owner decides in the orphan review (M2-05).
    OrphanRaw,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RawDisplay => "raw_display",
            Self::Edited => "edited",
            Self::DisplayOnly => "display_only",
            Self::RawOnly => "raw_only",
            Self::OrphanRaw => "orphan_raw",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::RawDisplay,
            Self::Edited,
            Self::DisplayOnly,
            Self::RawOnly,
            Self::OrphanRaw,
        ]
        .into_iter()
        .find(|state| state.as_str() == s)
    }
}

/// The state of a planned Photo, given what the catalog said about it before.
pub fn derive(photo: &PlannedPhoto, previous: Option<State>) -> State {
    match (photo.raw, photo.display) {
        (Some(_), Some(_)) if previous == Some(State::Edited) => State::Edited,
        (Some(_), Some(_)) => State::RawDisplay,
        (None, _) => State::DisplayOnly,
        // A RAW that had a display copy and lost it. A guard split isn't a loss.
        (Some(_), None)
            if !photo.guard_mismatch
                && matches!(
                    previous,
                    Some(State::RawDisplay | State::Edited | State::OrphanRaw)
                ) =>
        {
            State::OrphanRaw
        }
        (Some(_), None) => State::RawOnly,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(raw: bool, display: bool, guard_mismatch: bool) -> PlannedPhoto {
        PlannedPhoto {
            stem_key: "dsc0001".into(),
            raw: raw.then_some(1),
            display: display.then_some(2),
            extras: vec![],
            sidecars: vec![],
            guard_mismatch,
        }
    }

    #[test]
    fn states() {
        use State::*;
        let cases = [
            (photo(true, true, false), None, RawDisplay),
            (photo(true, true, false), Some(Edited), Edited),
            (photo(false, true, false), Some(RawDisplay), DisplayOnly),
            (photo(true, false, false), None, RawOnly),
            (photo(true, false, false), Some(RawOnly), RawOnly),
            (photo(true, false, false), Some(RawDisplay), OrphanRaw),
            (photo(true, false, false), Some(Edited), OrphanRaw),
            (photo(true, false, false), Some(OrphanRaw), OrphanRaw),
            (photo(true, false, true), Some(RawDisplay), RawOnly),
        ];
        for (p, previous, expected) in cases {
            assert_eq!(derive(&p, previous), expected, "{p:?} after {previous:?}");
        }
        for s in [RawDisplay, Edited, DisplayOnly, RawOnly, OrphanRaw] {
            assert_eq!(State::parse(s.as_str()), Some(s));
        }
    }
}
