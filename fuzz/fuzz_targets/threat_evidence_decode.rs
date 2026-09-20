//! Fuzzes the threat-intel decoders: attack attestations and stored
//! indicators.
//!
//! An attestation is written by whoever submits it, and its evidence frame by
//! whoever the submitter says signed it. The properties: no input panics, a
//! declared evidence length over the cap is refused before anything is read,
//! and anything that decodes re-encodes to exactly the bytes that produced it —
//! a second encoding would be a second transaction id for the same evidence.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use libfuzzer_sys::fuzz_target;
use maya_threat_intel::{
    AttackAttestation, HEADER_BYTES, MAX_EVIDENCE_DATA_BYTES, ThreatIndicator,
};

fuzz_target!(|data: &[u8]| {
    if let Ok(attestation) = AttackAttestation::decode(data) {
        assert_eq!(attestation.encode(), data);
        assert!(attestation.gossip.data.len() <= MAX_EVIDENCE_DATA_BYTES);
        let _ = attestation.signed_bytes();
    }
    if let Some(header) = data.get(..HEADER_BYTES) {
        let header: &[u8; HEADER_BYTES] = header.try_into().expect("sliced to length");
        if let Ok(len) = AttackAttestation::data_len(header) {
            assert!(len <= MAX_EVIDENCE_DATA_BYTES);
        }
    }
    if let Ok(indicator) = ThreatIndicator::decode(data) {
        assert_eq!(indicator.encode().as_slice(), data);
    }
});
