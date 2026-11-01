//! Fuzzes the oracle payload decoders: feed observations and submissions, feed
//! creation, authority rotation and randomness beacons.
//!
//! Oracle payloads ask the chain to believe a named party, so every field a
//! decoder accepts has to round-trip to one canonical encoding. The first byte
//! selects the decoder; the rest is the section. Shares the canonicality and
//! round-trip property with `payload_decode`.

#![no_main]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::oracle_payload::{
    BeaconSubmission, FeedCreation, FeedObservation, FeedSubmission, RegistryRotation,
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
            "feed observation",
            FeedObservation::decode,
            FeedObservation::encode_into,
        ),
        1 => section::check_section(
            payload,
            "feed submission",
            FeedSubmission::decode,
            FeedSubmission::encode_into,
        ),
        2 => section::check_section(
            payload,
            "feed creation",
            FeedCreation::decode,
            FeedCreation::encode_into,
        ),
        3 => section::check_section(
            payload,
            "registry rotation",
            RegistryRotation::decode,
            RegistryRotation::encode_into,
        ),
        _ => section::check_section(
            payload,
            "beacon submission",
            BeaconSubmission::decode,
            BeaconSubmission::encode_into,
        ),
    }
});
