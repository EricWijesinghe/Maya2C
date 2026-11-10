//! Fuzzes the status service's untrusted inputs: the JSON of a stopped-chain
//! report read from a file, the hex header a node returns for `get_headers`
//! (the timestamp is read out of it), and the host extracted from a node URL.
//!
//! Properties:
//!
//! - **No panic** on any text.
//! - **An accepted stopped report is marked stopped and not halted**, and
//!   marking it again changes nothing. The flag must not be able to hide a halt.
//! - **An accepted header is long enough to contain its timestamp field.**
//! - **The host is a substring of the URL and contains no path or query.**

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use libfuzzer_sys::fuzz_target;
use maya_status::decentral::host_of;
use maya_status::{mark_stopped, timestamp_from_header};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    if let Ok(report) = mark_stopped(text) {
        assert_eq!(report["stopped"], serde_json::Value::Bool(true));
        assert_eq!(report["halted_now"], serde_json::Value::Bool(false));
        let again = mark_stopped(&report.to_string()).expect("a marked report marks again");
        assert_eq!(again, report, "marking a stopped report is not idempotent");
    }

    if timestamp_from_header(text).is_some() {
        assert!(text.len() >= 144, "a timestamp was read from a short header");
    }

    let host = host_of(text);
    assert!(text.contains(host), "the host is not part of the URL");
    assert!(!host.contains('/') && !host.contains('?'), "the host carries a path or query");
});
