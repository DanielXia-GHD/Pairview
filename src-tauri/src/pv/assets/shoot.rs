//! Shoot folders and their date hints (PRD §5.1).

use chrono::{Datelike, NaiveDate};

/// A date read from a folder name. EXIF always wins; this is the fallback and the input for the
/// "date uncertain" check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateHint {
    /// `2026-10-31` or `2026-10-31_10561031` (the owner's rename).
    Full(NaiveDate),
    /// Camera folder `NNNYMMDD`, e.g. `10561031` = counter 105, year …6, Oct 31.
    Camera {
        counter: u16,
        year_digit: u8,
        month: u32,
        day: u32,
    },
}

impl DateHint {
    pub fn parse(name: &str) -> Option<Self> {
        if let Some(date) = name
            .get(..10)
            .filter(|_| name.len() == 10 || name[10..].starts_with(['_', ' ', '-']))
            .and_then(|prefix| NaiveDate::parse_from_str(prefix, "%Y-%m-%d").ok())
        {
            return Some(Self::Full(date));
        }
        if name.len() != 8 || !name.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let counter: u16 = name[..3].parse().ok()?;
        let year_digit = name.as_bytes()[3] - b'0';
        let month: u32 = name[4..6].parse().ok()?;
        let day: u32 = name[6..8].parse().ok()?;
        // Validate against a leap year so 29 Feb is accepted here and checked on resolve.
        let valid =
            (100..=999).contains(&counter) && NaiveDate::from_ymd_opt(2024, month, day).is_some();
        valid.then_some(Self::Camera {
            counter,
            year_digit,
            month,
            day,
        })
    }

    /// The calendar date, taking the decade from `near` (the folder's mtime, or a Photo's EXIF
    /// date): the latest date with a year ending in `year_digit` that isn't after `near` (with
    /// two days of slack for time zones and clocks).
    pub fn resolve(self, near: NaiveDate) -> Option<NaiveDate> {
        match self {
            Self::Full(date) => Some(date),
            Self::Camera {
                year_digit,
                month,
                day,
                ..
            } => {
                let latest = near + chrono::Days::new(2);
                let base = near.year() - near.year().rem_euclid(10) + year_digit as i32;
                [base + 10, base, base - 10, base - 20]
                    .into_iter()
                    .filter_map(|year| NaiveDate::from_ymd_opt(year, month, day))
                    .find(|date| *date <= latest)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn parses_owner_and_camera_names() {
        assert_eq!(
            DateHint::parse("2026-10-31_10561031"),
            Some(DateHint::Full(d(2026, 10, 31)))
        );
        assert_eq!(
            DateHint::parse("2026-10-31"),
            Some(DateHint::Full(d(2026, 10, 31)))
        );
        assert_eq!(
            DateHint::parse("10561031"),
            Some(DateHint::Camera {
                counter: 105,
                year_digit: 6,
                month: 10,
                day: 31
            })
        );
        for not_a_date in [
            "DCIM",
            "arw",
            "2026-13-01_x",
            "2026-10-31x",
            "05561031",
            "10561331",
            "1056103",
            "105610311",
            "10560230",
        ] {
            assert_eq!(DateHint::parse(not_a_date), None, "{not_a_date}");
        }
    }

    #[test]
    fn camera_names_take_the_decade_from_a_nearby_date() {
        let hint = DateHint::parse("10561031").unwrap();
        assert_eq!(hint.resolve(d(2026, 11, 3)), Some(d(2026, 10, 31)));
        // Copied years later: still the most recent year ending in 6.
        assert_eq!(hint.resolve(d(2031, 5, 1)), Some(d(2026, 10, 31)));
        // Folder written at New Year, just before the year digit flips.
        assert_eq!(
            DateHint::parse("10071231")
                .unwrap()
                .resolve(d(2026, 12, 31)),
            Some(d(2017, 12, 31))
        );
        assert_eq!(
            DateHint::parse("10070101")
                .unwrap()
                .resolve(d(2026, 12, 31)),
            Some(d(2027, 1, 1))
        );
        // 29 Feb only in leap years.
        assert_eq!(
            DateHint::parse("10080229").unwrap().resolve(d(2028, 3, 1)),
            Some(d(2028, 2, 29))
        );
        assert_eq!(
            DateHint::parse("10070229").unwrap().resolve(d(2027, 3, 1)),
            None
        );
    }
}
