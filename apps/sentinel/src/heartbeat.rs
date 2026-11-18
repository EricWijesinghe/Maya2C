//! The dead-man switch (ADR-101). A sentinel that crashed, hung or lost its
//! machine sends nothing, and silence reads as a healthy chain. So the running
//! sentinel stamps a file after every poll, and a second, scheduled
//! invocation (`--check-heartbeat`) pages when the stamp is missing or old.

use crate::detect::{Finding, Severity};

/// Default age at which a stamp counts as dead. Well above one poll plus the
/// RPC timeouts of a slow poll, so a busy sentinel is not reported as dead.
pub const DEFAULT_MAX_AGE_SECS: u64 = 120;

/// File contents for a stamp taken at `unix_secs`.
#[must_use]
pub fn stamp(unix_secs: u64) -> String {
    format!("{unix_secs}\n")
}

/// The finding for a heartbeat file read at `now_unix`, or `None` when the
/// sentinel is alive. `contents` is `None` when the file could not be read.
/// A stamp from the future (clock step) counts as fresh.
#[must_use]
pub fn check(contents: Option<&str>, now_unix: u64, max_age_secs: u64) -> Option<Finding> {
    let detail = match contents.map(|c| c.trim().parse::<u64>()) {
        None => "no heartbeat file: the sentinel never started or cannot write it".to_owned(),
        Some(Err(_)) => "heartbeat file is not a timestamp".to_owned(),
        Some(Ok(t)) => {
            let age = now_unix.saturating_sub(t);
            if age < max_age_secs {
                return None;
            }
            format!("last poll {age} s ago, limit {max_age_secs} s")
        }
    };
    Some(Finding {
        key: "heartbeat".into(),
        severity: Severity::Critical,
        title: "Maya2C sentinel stopped".into(),
        body: format!("{detail}; nothing is watching the chain"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_stamp_is_alive_and_the_limit_is_inclusive() {
        let s = stamp(1_000);
        assert_eq!(check(Some(&s), 1_119, 120), None);
        assert_eq!(
            check(Some(&s), 1_120, 120).map(|f| f.key),
            Some("heartbeat".into())
        );
    }

    #[test]
    fn missing_garbled_or_future_stamps() {
        assert!(check(None, 5, 120).is_some(), "missing file pages");
        assert!(
            check(Some("not a number"), 5, 120).is_some(),
            "garbled pages"
        );
        assert_eq!(check(Some("900"), 5, 120), None, "future stamp is fresh");
    }
}
