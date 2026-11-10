//! Fuzzes the chat relay's two byte-level readers: [`Envelope::decode`], the
//! sealed item a relay stores for an offline recipient, and [`receipt::parse`],
//! the delivery receipt a recipient returns.
//!
//! A relay and a peer both hand these bytes over without a trusted party in
//! between. Properties:
//!
//! - **No panic** on any bytes.
//! - **An accepted envelope re-encodes and decodes back to the same value.**
//! - **An accepted receipt re-encodes to its own bytes.** Its id count is
//!   fixed-width and its ids are fixed-width, so there is one spelling.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use libfuzzer_sys::fuzz_target;
use maya_chat::receipt;
use maya_chat::relay::Envelope;

fuzz_target!(|data: &[u8]| {
    if let Ok(envelope) = Envelope::decode(data) {
        let bytes = envelope.encode().expect("a decoded envelope encodes");
        let again = Envelope::decode(&bytes).expect("an encoded envelope decodes");
        assert_eq!(again, envelope, "an envelope changed across a round trip");
        let _ = envelope.id();
    }

    if let Ok((ids, signature)) = receipt::parse(data) {
        assert_eq!(
            receipt::encode(&ids, signature),
            data,
            "an accepted receipt has a second spelling"
        );
    }
});
