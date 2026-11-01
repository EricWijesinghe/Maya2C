//! Fuzzes the identity payload decoders: DID registration, key rotation,
//! revocation, attestation anchoring and revocation-bit updates.
//!
//! `did_decode` covers the `maya-identity` crate's document and attestation
//! records. These are the transaction-carried forms, which the chain reads
//! before any of that crate's code runs. The first byte selects the decoder;
//! the rest is the section. Shares the canonicality and round-trip property
//! with `payload_decode`.

#![no_main]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::identity_payload::{
    AnchorAttestation, RegisterDid, RevokeDid, RotateDidKey, SetRevocationBit,
};
use libfuzzer_sys::fuzz_target;

#[path = "common/section.rs"]
mod section;

fuzz_target!(|data: &[u8]| {
    let Some((&selector, payload)) = data.split_first() else {
        return;
    };
    match selector % 5 {
        0 => section::check_section(
            payload,
            "register did",
            RegisterDid::decode,
            RegisterDid::encode_into,
        ),
        1 => section::check_section(
            payload,
            "rotate did key",
            RotateDidKey::decode,
            RotateDidKey::encode_into,
        ),
        2 => section::check_section(
            payload,
            "revoke did",
            RevokeDid::decode,
            RevokeDid::encode_into,
        ),
        3 => section::check_section(
            payload,
            "anchor attestation",
            AnchorAttestation::decode,
            AnchorAttestation::encode_into,
        ),
        _ => section::check_section(
            payload,
            "set revocation bit",
            SetRevocationBit::decode,
            SetRevocationBit::encode_into,
        ),
    }
});
