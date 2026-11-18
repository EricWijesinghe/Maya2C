//! Report stamps: the date and time strings xtask writes into report names and
//! bodies. They name files a person reads, never a consensus input, so the
//! wall clock is fine here.

use chrono::{DateTime, FixedOffset, Local, SecondsFormat, Utc};

/// Now, at the local UTC offset.
pub fn local_now() -> DateTime<FixedOffset> {
    Local::now().fixed_offset()
}

/// Now, in UTC.
pub fn utc_now() -> DateTime<FixedOffset> {
    Utc::now().fixed_offset()
}

/// `YYYY-MM-DD` at the instant's own offset.
pub fn date(t: DateTime<FixedOffset>) -> String {
    t.date_naive().format("%Y-%m-%d").to_string()
}

/// `YYYY-MM-DD-HHMMSS` at the instant's own offset: a file-name stamp.
pub fn file_stamp(t: DateTime<FixedOffset>) -> String {
    t.format("%Y-%m-%d-%H%M%S").to_string()
}

/// RFC 3339 in UTC, whole seconds, `+00:00` rather than `Z`.
pub fn rfc3339_utc(t: DateTime<FixedOffset>) -> String {
    t.with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Secs, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2023-11-14T22:13:20Z, plus `nanos`, viewed at `offset_secs` east of UTC.
    fn at(nanos: u32, offset_secs: i32) -> DateTime<FixedOffset> {
        let offset = FixedOffset::east_opt(offset_secs).unwrap();
        DateTime::from_timestamp(1_700_000_000, nanos)
            .unwrap()
            .with_timezone(&offset)
    }

    /// 2024-01-02T03:04:05Z: every field single-digit, so padding shows.
    fn padded() -> DateTime<FixedOffset> {
        DateTime::from_timestamp(1_704_164_645, 0)
            .unwrap()
            .fixed_offset()
    }

    #[test]
    fn date_is_the_civil_date_at_the_offset() {
        assert_eq!(date(at(0, 0)), "2023-11-14");
        assert_eq!(date(at(0, 9 * 3600)), "2023-11-15");
        assert_eq!(date(at(0, -23 * 3600)), "2023-11-13");
        assert_eq!(date(padded()), "2024-01-02");
    }

    #[test]
    fn file_stamp_is_date_dash_hhmmss() {
        assert_eq!(file_stamp(at(0, 0)), "2023-11-14-221320");
        assert_eq!(file_stamp(at(999_999_999, 9 * 3600)), "2023-11-15-071320");
        assert_eq!(file_stamp(padded()), "2024-01-02-030405");
    }

    #[test]
    fn rfc3339_is_utc_whole_seconds_with_a_numeric_offset() {
        assert_eq!(rfc3339_utc(at(0, 0)), "2023-11-14T22:13:20+00:00");
        assert_eq!(
            rfc3339_utc(at(500_000_000, 9 * 3600)),
            "2023-11-14T22:13:20+00:00"
        );
        assert_eq!(rfc3339_utc(padded()), "2024-01-02T03:04:05+00:00");
    }

    #[test]
    fn now_has_the_shape_of_a_stamp() {
        assert_eq!(date(utc_now()).len(), 10);
        assert_eq!(file_stamp(local_now()).len(), 17);
    }
}
