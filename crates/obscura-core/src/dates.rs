//! Date parsing shared by every platform.
//!
//! The accepted formats are the same everywhere, but platforms disagree on
//! what a bare `YYYY-MM-DD` bound means, so the caller states the policy.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateBound {
    Before,
    After,
}

/// How a bare calendar date maps to an instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundStyle {
    /// The bound day is included: `after` starts at 00:00 and `before` ends
    /// at 23:59:59.999 (Discord).
    WholeDay,
    /// The bound day is excluded: `before` is 00:00 and `after` is the end of
    /// the day (Instagram).
    ExcludeDay,
}

impl BoundStyle {
    fn instant(self, date: NaiveDate, bound: DateBound) -> Option<NaiveDateTime> {
        let start_of_day = matches!(
            (self, bound),
            (Self::WholeDay, DateBound::After) | (Self::ExcludeDay, DateBound::Before)
        );
        if start_of_day {
            date.and_hms_opt(0, 0, 0)
        } else {
            date.and_hms_nano_opt(23, 59, 59, 999_999_999)
        }
    }
}

pub fn parse_to_datetime(
    input: &str,
    bound: DateBound,
    style: BoundStyle,
) -> Option<DateTime<Utc>> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(input) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(input, fmt) {
            return Some(Utc.from_utc_datetime(&naive));
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(input, "%Y-%m-%d") {
        let naive = style.instant(date, bound)?;
        return Some(Utc.from_utc_datetime(&naive));
    }
    None
}

pub fn validate_date_input(s: &str, style: BoundStyle) -> Result<Option<DateTime<Utc>>, String> {
    if s.trim().is_empty() {
        return Ok(None);
    }

    parse_to_datetime(s, DateBound::After, style)
        .map(Some)
        .ok_or_else(|| {
            format!(
                "invalid date {:?}; expected RFC3339 (e.g. 2024-12-31T23:59:00Z) or YYYY-MM-DD",
                s
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rfc3339() {
        let dt = parse_to_datetime(
            "2024-06-15T12:00:00Z",
            DateBound::Before,
            BoundStyle::WholeDay,
        )
        .unwrap();
        assert_eq!(dt.format("%Y-%m-%d").to_string(), "2024-06-15");
    }

    #[test]
    fn whole_day_after_is_midnight() {
        let d = parse_to_datetime("2024-01-02", DateBound::After, BoundStyle::WholeDay).unwrap();
        assert_eq!(d.timestamp(), 1_704_153_600);
    }

    #[test]
    fn whole_day_before_is_end_of_day() {
        let d = parse_to_datetime("2024-01-02", DateBound::Before, BoundStyle::WholeDay).unwrap();
        assert_eq!(
            d.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2024-01-02 23:59:59"
        );
        let start =
            parse_to_datetime("2024-01-02", DateBound::After, BoundStyle::WholeDay).unwrap();
        assert!(d > start);
    }

    #[test]
    fn excluded_day_before_is_midnight() {
        let d = parse_to_datetime("2024-01-02", DateBound::Before, BoundStyle::ExcludeDay).unwrap();
        assert_eq!(d.timestamp(), 1_704_153_600);
    }

    #[test]
    fn excluded_day_after_is_end_of_day() {
        let d = parse_to_datetime("2024-01-02", DateBound::After, BoundStyle::ExcludeDay).unwrap();
        assert_eq!(
            d.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2024-01-02 23:59:59"
        );
        let start =
            parse_to_datetime("2024-01-02", DateBound::Before, BoundStyle::ExcludeDay).unwrap();
        assert!(d > start);
    }

    #[test]
    fn validate_empty_is_none() {
        assert_eq!(validate_date_input("", BoundStyle::WholeDay).unwrap(), None);
        assert_eq!(
            validate_date_input("   ", BoundStyle::WholeDay).unwrap(),
            None
        );
    }

    #[test]
    fn validate_rejects_garbage() {
        assert!(validate_date_input("not-a-date", BoundStyle::WholeDay).is_err());
    }
}
