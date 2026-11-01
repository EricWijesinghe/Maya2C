//! Fuzzes the sealed-mempool payload decoders: the transaction-carried
//! envelope and reveal share in `core::sealed_payload`, and the ciphertext and
//! share proof in `maya-mev`.
//!
//! The ciphertext is opaque to every node until its reveal height, so these
//! bytes are parsed while the envelope is still sealed. A decoder that accepted
//! two encodings of one envelope or one proof would give one sealed payload two
//! identifiers. The first byte selects the decoder; the rest is the input.

#![no_main]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::sealed_payload::{RevealShare, SealedEnvelope};
use libfuzzer_sys::fuzz_target;
use maya_mev::{SealedPayload, ShareProof};

#[path = "common/section.rs"]
mod section;

fuzz_target!(|data: &[u8]| {
    let Some((&selector, payload)) = data.split_first() else {
        return;
    };
    match selector % 4 {
        0 => section::check_section(
            payload,
            "sealed envelope",
            SealedEnvelope::decode,
            SealedEnvelope::encode_into,
        ),
        1 => section::check_section(
            payload,
            "reveal share",
            RevealShare::decode,
            RevealShare::encode_into,
        ),
        2 => section::check_record(
            payload,
            "sealed payload",
            SealedPayload::decode,
            SealedPayload::encode,
        ),
        _ => {
            // A share proof is exactly 64 bytes, so the length is the framing.
            if let Ok(bytes) = <&[u8; 64]>::try_from(payload) {
                if let Ok(proof) = ShareProof::decode(bytes) {
                    assert_eq!(&proof.encode(), bytes, "non-canonical share proof accepted");
                }
            }
        }
    }
});
