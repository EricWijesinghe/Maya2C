//! Fuzzes [`maya_address::decode_any`]: the Bech32m text form of an account
//! id, pasted by people and read out of QR codes.
//!
//! Bech32m carries a checksum and a zero-padded final group, so an accepted
//! string has exactly one spelling. The property is that: an accepted address
//! re-encodes to itself (in lower case, since mixed case is refused), and the
//! re-encoding decodes back to the same network and id. A second spelling of
//! one id would be two strings the wallet treats as different recipients.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use libfuzzer_sys::fuzz_target;
use maya_address::{decode_any, encode};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    if let Ok((network, id)) = decode_any(text) {
        let canonical = encode(network, &id);
        assert_eq!(
            canonical,
            text.to_ascii_lowercase(),
            "an accepted address has a second spelling"
        );
        let again = decode_any(&canonical).expect("an encoded address decodes");
        assert_eq!(again, (network, id), "address changed across a round trip");
    }
});
