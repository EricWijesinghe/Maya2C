//! The property `fuzz/fuzz_targets/sealed_payload_decode.rs` asserts under
//! libFuzzer, run here on a fixed pseudo-random input stream so it executes on
//! every `cargo test`: an accepted ciphertext or share proof re-encodes to the
//! exact bytes it was parsed from.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_mev::cipher::{SealedPayload, ShareProof};

/// Linear congruential generator, so a failure reproduces from the constant
/// alone with no seed file.
fn next_byte(state: &mut u64) -> u8 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (*state >> 56) as u8
}

fn random_bytes(state: &mut u64, len: usize) -> Vec<u8> {
    (0..len).map(|_| next_byte(state)).collect()
}

#[test]
fn accepted_sealed_payloads_re_encode_to_their_bytes() {
    let mut state = 0x5EA1_ED00_u64;
    for len in 0..=256 {
        for _ in 0..64 {
            let bytes = random_bytes(&mut state, len);
            if let Ok(payload) = SealedPayload::decode(&bytes) {
                assert_eq!(payload.encode(), bytes, "non-canonical payload accepted");
                let again = SealedPayload::decode(&payload.encode()).expect("own encoding");
                assert_eq!(again, payload);
            }
        }
    }
}

#[test]
fn accepted_share_proofs_re_encode_to_their_bytes() {
    let mut state = 0x5EA1_ED01_u64;
    let mut accepted = 0_usize;
    for _ in 0..4096 {
        let bytes: [u8; 64] = random_bytes(&mut state, 64).try_into().unwrap();
        if let Ok(proof) = ShareProof::decode(&bytes) {
            accepted += 1;
            assert_eq!(proof.encode(), bytes, "non-canonical proof accepted");
        }
    }
    // Each half is canonical with probability about 1/16, so a fair share of
    // the 4096 draws must reach the accepting branch or the check proves nothing.
    assert!(
        accepted > 0,
        "no proof was accepted; the property was not exercised"
    );
}
