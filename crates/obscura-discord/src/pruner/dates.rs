//! Discord date helpers on top of the shared parser.
//!
//! Discord reads a bare date as a whole day: `after` starts at 00:00 and
//! `before` ends at 23:59:59.999.

use chrono::{DateTime, Utc};

pub use obscura_core::dates::{BoundStyle, DateBound};

const STYLE: BoundStyle = BoundStyle::WholeDay;

pub fn parse_to_datetime(input: &str, bound: DateBound) -> Option<DateTime<Utc>> {
    obscura_core::dates::parse_to_datetime(input, bound, STYLE)
}

pub fn validate_date_input(s: &str) -> Result<Option<DateTime<Utc>>, String> {
    obscura_core::dates::validate_date_input(s, STYLE)
}

/// Discord message timestamp (RFC3339 from the API).
pub fn message_timestamp(ts: &str) -> Option<DateTime<Utc>> {
    let s = ts.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    None
}

/// Discord snowflake -> creation time (ms since Discord epoch 1420070400000).
pub const DISCORD_EPOCH: i64 = 1_420_070_400_000;

/// Convert a DateTime<Utc> to a Discord snowflake ID, mirroring
/// discord.py-self's `utils.time_snowflake(dt, high=high)`.
pub fn datetime_to_snowflake(dt: &DateTime<Utc>, high: bool) -> u64 {
    let discord_millis = dt.timestamp_millis() - DISCORD_EPOCH;
    let clamped = discord_millis.max(0) as u64;
    (clamped << 22) + if high { (1 << 22) - 1 } else { 0 }
}

pub fn snowflake_time(id: &str) -> Option<DateTime<Utc>> {
    let n: u64 = id.trim().parse().ok()?;
    let ms = (n >> 22) + DISCORD_EPOCH as u64;
    DateTime::from_timestamp_millis(ms as i64)
}

pub fn message_time(msg: &crate::model::Message) -> Option<DateTime<Utc>> {
    message_timestamp(&msg.timestamp).or_else(|| snowflake_time(&msg.id))
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
    fn bare_date_after_is_midnight() {
        let d = parse_to_datetime("2024-01-02", DateBound::After).unwrap();
        assert_eq!(d.timestamp(), 1_704_153_600);
    }

    #[test]
    fn bare_date_before_is_end_of_day() {
        let d = parse_to_datetime("2024-01-02", DateBound::Before).unwrap();

        assert_eq!(
            d.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2024-01-02 23:59:59"
        );
        let start = parse_to_datetime("2024-01-02", DateBound::After).unwrap();
        assert!(d > start);

        let next = parse_to_datetime("2024-01-03", DateBound::After).unwrap();
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

    #[test]
    fn snowflake_time_known_value() {
        // Discord example snowflake 175928847299117063 -> 2016-04-30.
        let dt = snowflake_time("175928847299117063").unwrap();
        assert_eq!(dt.format("%Y-%m-%d").to_string(), "2016-04-30");
    }

    #[test]
    fn datetime_to_snowflake_roundtrip() {
        let dt = snowflake_time("175928847299117063").unwrap();
        let sf_low = datetime_to_snowflake(&dt, false);
        // The timestamp bits must match
        assert_eq!(sf_low >> 22, 175_928_847_299_117_063 >> 22);
        let sf_high = datetime_to_snowflake(&dt, true);
        assert_eq!(sf_high >> 22, 175_928_847_299_117_063 >> 22);
        assert_eq!(sf_high - sf_low, (1 << 22) - 1);
    }
}
