//! Fuzzes the explorer's text handling for route and form values: the account
//! check behind `/account?address=`, and the shortening and formatting applied
//! to identifiers before they reach a page.
//!
//! The values are whatever a visitor types or a link carries. Properties:
//!
//! - **No panic** on any text. `short_hash` once sliced by byte, which panics
//!   inside a multi-byte character.
//! - **An accepted account id is exactly 64 hex characters.**
//! - **Shortening keeps the ends and marks the cut**, so a long value is never
//!   shown as a different, shorter, identifier.
//! - **No panic on any float**, including NaN and infinities from a bad
//!   hashrate sample.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use libfuzzer_sys::fuzz_target;
use maya_explorer::server::is_account_hex;
use maya_explorer::ui::{format_hashrate, short_hash};

fuzz_target!(|data: &[u8]| {
    if data.len() >= 8 {
        let rate = f64::from_le_bytes(data[..8].try_into().expect("eight bytes"));
        assert!(!format_hashrate(rate).is_empty());
    }

    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    if is_account_hex(text) {
        assert_eq!(text.len(), 64, "an accepted account id is not 32 bytes");
        assert!(
            text.bytes().all(|b| b.is_ascii_hexdigit()),
            "an accepted account id is not hex"
        );
    }

    let short = short_hash(text);
    let chars = text.chars().count();
    if chars > 16 {
        assert!(short.contains('…'), "a long value was shortened without a marker");
        assert!(short.starts_with(text.chars().take(8).collect::<String>().as_str()));
        assert!(short.ends_with(text.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<String>().as_str()));
    } else {
        assert_eq!(short, text, "a short value changed");
    }
});
