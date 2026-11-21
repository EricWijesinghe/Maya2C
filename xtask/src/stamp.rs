//! Report stamps: the date and time strings xtask writes into report names and
//! bodies. They name files a person reads, never a consensus input, so the
//! wall clock is fine here.

use time::{OffsetDateTime, UtcOffset};

/// Now, at the local UTC offset, or in UTC when the offset cannot be read.
///
/// `time` refuses to read the local offset on Unix while more than one thread
/// is running (the libc call it needs is not thread-safe), and some platforms
/// cannot report one at all. A UTC stamp is still a correct, sortable name, so
/// the report goes ahead and says which clock it used.
pub fn local_now() -> OffsetDateTime {
    OffsetDateTime::now_local().unwrap_or_else(|e| {
        eprintln!("note: local UTC offset unavailable ({e}); stamping in UTC");
        OffsetDateTime::now_utc()
    })
}

/// Now, in UTC.
pub fn utc_now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// `YYYY-MM-DD` at the instant's own offset.
pub fn date(t: OffsetDateTime) -> String {
    format!("{:04}-{:02}-{:02}", t.year(), u8::from(t.month()), t.day())
}

/// `YYYY-MM-DD-HHMMSS` at the instant's own offset: a file-name stamp.
pub fn file_stamp(t: OffsetDateTime) -> String {
    let (h, m, s) = t.to_hms();
    format!("{}-{h:02}{m:02}{s:02}", date(t))
}

/// RFC 3339 in UTC, whole seconds, `+00:00` rather than `Z`.
pub fn rfc3339_utc(t: OffsetDateTime) -> String {
    let t = t.to_offset(UtcOffset::UTC);
    let (h, m, s) = t.to_hms();
    format!("{}T{h:02}:{m:02}:{s:02}+00:00", date(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2023-11-14T22:13:20Z, plus `nanos`, viewed at `offset_secs` east of UTC.
    fn at(nanos: u32, offset_secs: i32) -> OffsetDateTime {
        let offset = UtcOffset::from_whole_seconds(offset_secs).unwrap();
        (OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
            + time::Duration::nanoseconds(i64::from(nanos)))
        .to_offset(offset)
    }

    /// 2024-01-02T03:04:05Z: every field single-digit, so padding shows.
    fn padded() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_704_164_645).unwrap()
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
