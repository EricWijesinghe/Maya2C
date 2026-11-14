//! Fuzzes [`PaymentRequest::parse`] and the address normaliser behind it: the
//! `maya:` URI a requester shows as text or a QR code.
//!
//! The URI is untrusted: the requester is anonymous and the wallet parses it
//! before the user has confirmed anything. Two properties:
//!
//! - **No panic** on any text, including amounts past 256 bits and labels with
//!   broken percent-escapes.
//! - **A parsed request renders to a URI that parses back to the same request.**
//!   If it did not, the confirm screen would show one payment and the URI the
//!   wallet re-shares would describe another.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use libfuzzer_sys::fuzz_target;
use maya_wallet_core::payment::{PaymentRequest, normalize_address};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    if let Ok(address) = normalize_address(text) {
        let again = normalize_address(&address).expect("a normalised address re-parses");
        assert_eq!(again, address, "normalisation is not idempotent");
    }

    if let Ok(request) = PaymentRequest::parse(text) {
        let uri = request.to_uri();
        let reparsed = PaymentRequest::parse(&uri)
            .expect("a parsed request renders to a URI that parses");
        assert_eq!(reparsed, request, "a request changed across to_uri and parse");
    }
});
