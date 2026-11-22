//! Instagram date helpers on top of the shared parser.
//!
//! Instagram excludes the bound day on both sides: `before` is 00:00 and
//! `after` is the end of the day.

use chrono::{DateTime, Utc};

pub use obscura_core::dates::{BoundStyle, DateBound};

const STYLE: BoundStyle = BoundStyle::ExcludeDay;

pub fn parse_to_datetime(input: &str, bound: DateBound) -> Option<DateTime<Utc>> {
    obscura_core::dates::parse_to_datetime(input, bound, STYLE)
}

pub fn validate_date_input(s: &str) -> Result<Option<DateTime<Utc>>, String> {
    obscura_core::dates::validate_date_input(s, STYLE)
}

pub fn media_taken_at(secs: i64) -> Option<DateTime<Utc>> {
    if secs <= 0 {
        return None;
    }
    DateTime::<Utc>::from_timestamp(secs, 0)
}

pub fn dm_timestamp_us(us: i64) -> Option<DateTime<Utc>> {
    if us <= 0 {
        return None;
    }
    let secs = us / 1_000_000;
    let nsecs = ((us % 1_000_000) * 1000) as u32;
    DateTime::<Utc>::from_timestamp(secs, nsecs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rfc3339() {
        let dt = parse_to_datetime("2024-06-15T12:00:00Z", DateBound::Before).unwrap();
        assert_eq!(dt.format("%Y-%m-%d").to_string(), "2024-06-15");
    }

    #[test]
    fn bare_date_before_is_midnight() {
        let d = parse_to_datetime("2024-01-02", DateBound::Before).unwrap();
        assert_eq!(d.timestamp(), 1_704_153_600);
    }

    #[test]
    fn bare_date_after_is_end_of_day() {
        let d = parse_to_datetime("2024-01-02", DateBound::After).unwrap();

        assert_eq!(
            d.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2024-01-02 23:59:59"
        );
        let start = parse_to_datetime("2024-01-02", DateBound::Before).unwrap();
        assert!(d > start);

        let next = parse_to_datetime("2024-01-03", DateBound::Before).unwrap();
        assert!(d < next);
    }

    #[test]
    fn validate_empty_is_none() {
        assert_eq!(validate_date_input("").unwrap(), None);
        assert_eq!(validate_date_input("   ").unwrap(), None);
    }

    #[test]
    fn validate_rejects_garbage() {
        assert!(validate_date_input("not-a-date").is_err());
    }
}
