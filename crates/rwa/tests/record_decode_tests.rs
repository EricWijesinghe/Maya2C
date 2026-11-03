//! The property `fuzz/fuzz_targets/rwa_record_decode.rs` asserts under
//! libFuzzer, run here on a fixed pseudo-random input stream so it executes on
//! every `cargo test`: an accepted record re-encodes to the exact bytes it was
//! parsed from, and decodes back to the same value.
//!
//! Every length from 0 to 512 is tried, so the fixed-size records are reached
//! at their exact size rather than only by chance.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_rwa::cap_table::CapTablePage;
use maya_rwa::token::{Eligibility, LegalAttestation, RevenueDistribution, RwaToken};

const MAX_LEN: usize = 512;
const SAMPLES_PER_LEN: usize = 32;

fn next_byte(state: &mut u64) -> u8 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (*state >> 56) as u8
}

/// Asserts the round-trip property for one decoder and returns how many inputs
/// it accepted, so the caller can require that the accepting branch ran.
fn exercise<T, E>(
    state: &mut u64,
    label: &str,
    decode: impl Fn(&[u8]) -> Result<T, E>,
    encode: impl Fn(&T) -> Vec<u8>,
) -> usize
where
    T: PartialEq + std::fmt::Debug,
    E: std::fmt::Debug,
{
    let mut accepted = 0;
    for len in 0..=MAX_LEN {
        for _ in 0..SAMPLES_PER_LEN {
            let bytes: Vec<u8> = (0..len).map(|_| next_byte(state)).collect();
            if let Ok(value) = decode(&bytes) {
                accepted += 1;
                assert_eq!(
                    encode(&value),
                    bytes,
                    "{label}: non-canonical record accepted"
                );
                let again = decode(&encode(&value)).expect("own encoding must decode");
                assert_eq!(again, value, "{label}: decode is not the inverse of encode");
            }
        }
    }
    accepted
}

#[test]
fn accepted_eligibility_records_re_encode_to_their_bytes() {
    let mut state = 0x0E11_0001_u64;
    let accepted = exercise(
        &mut state,
        "eligibility",
        Eligibility::decode,
        Eligibility::encode,
    );
    assert!(accepted > 0, "no eligibility record was accepted");
}

#[test]
fn accepted_revenue_distributions_re_encode_to_their_bytes() {
    let mut state = 0x0E11_0002_u64;
    let accepted = exercise(
        &mut state,
        "revenue distribution",
        RevenueDistribution::decode,
        RevenueDistribution::encode,
    );
    assert!(accepted > 0, "no revenue distribution was accepted");
}

#[test]
fn rwa_records_never_panic_on_arbitrary_bytes() {
    // Tokens, attestations and cap-table pages need structure (UTF-8 labels,
    // counts that agree with the body), so random bytes are mostly refused.
    // The assertion here is survival, plus the round trip for any accepted.
    let mut state = 0x0E11_0003_u64;
    exercise(&mut state, "rwa token", RwaToken::decode, RwaToken::encode);
    exercise(
        &mut state,
        "legal attestation",
        LegalAttestation::decode,
        LegalAttestation::encode,
    );
    exercise(
        &mut state,
        "cap table page",
        CapTablePage::decode,
        CapTablePage::encode,
    );
}
