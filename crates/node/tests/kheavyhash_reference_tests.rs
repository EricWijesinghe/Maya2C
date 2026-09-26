//! The kHeavyHash reference in `benches/support/kheavyhash.rs`, checked three
//! independent ways before any number it produces is published:
//!
//! 1. Both precomputed sponge states against a cSHAKE256 written here from
//!    NIST SP 800-185, not from Kaspa's code, over many inputs.
//! 2. Upstream's own `heavy_hash` known answer (fixed matrix, input, output).
//! 3. Upstream's own matrix-generation known answer (seed `[42; 32]`).
//!
//! The vectors are rusty-kaspa's, copied by `scripts/gen_kheavyhash.py`; see
//! `tests/fixtures/kaspa_kheavyhash.rs` for the commit.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../benches/support/kheavyhash.rs"]
mod kheavyhash;
#[path = "fixtures/kaspa_kheavyhash.rs"]
mod vectors;

use kheavyhash::{Matrix, PowHasher, State, heavy_hash_outer};

/// cSHAKE256's rate in bytes (1600 - 2·256 bits).
const RATE: usize = 136;

/// SP 800-185 §2.3.1 `left_encode`.
fn left_encode(value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let first = bytes.iter().position(|&b| b != 0).unwrap_or(7);
    let mut out = vec![u8::try_from(8 - first).expect("at most 8")];
    out.extend_from_slice(&bytes[first..]);
    out
}

/// SP 800-185 §2.3.2 `encode_string`.
fn encode_string(bits_of: &[u8]) -> Vec<u8> {
    let mut out = left_encode(u64::try_from(bits_of.len() * 8).expect("small"));
    out.extend_from_slice(bits_of);
    out
}

/// SP 800-185 §3.3: `cSHAKE256(X, 256, "", S)`, as a plain sponge over
/// Keccak-f[1600].
fn cshake256(customization: &[u8], message: &[u8]) -> [u8; 32] {
    let mut input = left_encode(RATE as u64);
    input.extend(encode_string(b""));
    input.extend(encode_string(customization));
    input.resize(input.len().div_ceil(RATE) * RATE, 0);
    input.extend_from_slice(message);
    // cSHAKE's two domain bits (00), then pad10*1.
    input.push(0x04);
    input.resize(input.len().div_ceil(RATE) * RATE, 0);
    *input.last_mut().expect("non-empty") ^= 0x80;

    let mut state = [0u64; 25];
    for block in input.as_chunks::<RATE>().0 {
        for (word, lane) in state.iter_mut().zip(block.as_chunks::<8>().0) {
            *word ^= u64::from_le_bytes(*lane);
        }
        keccak::f1600(&mut state);
    }
    let mut out = [0u8; 32];
    for (chunk, word) in out.as_chunks_mut::<8>().0.iter_mut().zip(state) {
        *chunk = word.to_le_bytes();
    }
    out
}

fn sample(i: u8) -> [u8; 32] {
    core::array::from_fn(|j| {
        i.wrapping_mul(31)
            .wrapping_add(u8::try_from(j).expect("< 32"))
    })
}

#[test]
fn the_sp800_185_cshake_matches_nists_published_sample() {
    // SP 800-185 cSHAKE256 sample #3: X = 00 01 02 03, N = "", S = "Email
    // Signature". Published with L = 512; cSHAKE's output is a prefix-stable
    // XOF, so its first 32 bytes are the L = 256 answer. Reproduced
    // independently with pycryptodome 3.21.0 before being pinned here.
    assert_eq!(
        hex::encode(cshake256(b"Email Signature", &[0, 1, 2, 3])),
        "d008828e2b80ac9d2218ffee1d070c48b8e4c87bff32c9699d5b6896eee0edd1"
    );
    // And with Kaspa's own customization string, the same cross-check.
    let counting: [u8; 32] = core::array::from_fn(|i| u8::try_from(i).expect("< 32"));
    assert_eq!(
        hex::encode(cshake256(b"HeavyHash", &counting)),
        "5f50dcf008bc684b5a5cfeb65e37557aaad130434cc17cd42f759d9065fb2dbc"
    );
    assert_eq!(left_encode(0), vec![1, 0]);
    assert_eq!(left_encode(136), vec![1, 136]);
    assert_eq!(left_encode(256), vec![2, 1, 0]);
}

#[test]
fn the_heavy_hash_state_is_cshake256_heavyhash() {
    for i in 0..64u8 {
        let input = sample(i);
        assert_eq!(
            heavy_hash_outer(&input),
            cshake256(b"HeavyHash", &input),
            "input {i}"
        );
    }
}

#[test]
fn the_pow_state_is_cshake256_proof_of_work_hash() {
    for i in 0..64u8 {
        let pre_pow = sample(i);
        let timestamp = 5_435_345_234 + u64::from(i);
        let nonce = 432_432_432u64.wrapping_mul(u64::from(i) + 1);
        let mut message = pre_pow.to_vec();
        message.extend_from_slice(&timestamp.to_le_bytes());
        message.extend_from_slice(&[0u8; 32]);
        message.extend_from_slice(&nonce.to_le_bytes());
        assert_eq!(
            PowHasher::new(&pre_pow, timestamp).finalize_with_nonce(nonce),
            cshake256(b"ProofOfWorkHash", &message),
            "input {i}"
        );
    }
}

#[test]
fn heavy_hash_matches_kaspas_known_answer() {
    let matrix = Matrix(vectors::HEAVY_HASH_MATRIX);
    assert_eq!(
        matrix.heavy_hash(&vectors::HEAVY_HASH_INPUT),
        vectors::HEAVY_HASH_EXPECTED
    );
}

#[test]
fn matrix_generation_matches_kaspas_known_answer() {
    assert_eq!(
        Matrix::generate(&vectors::GENERATE_SEED),
        Matrix(vectors::GENERATED_MATRIX)
    );
}

#[test]
fn rank_behaves_as_upstreams_test_says() {
    let zero = Matrix([[0; 64]; 64]);
    assert_eq!(zero.rank(), 0);
    let mut full = Matrix(vectors::GENERATED_MATRIX);
    assert_eq!(full.rank(), 64);
    full.0[0] = full.0[1];
    assert_eq!(full.rank(), 63);
}

#[test]
fn a_state_hashes_each_nonce_differently_and_repeatably() {
    let state = State::new(&[7; 32], 1_700_000_000);
    assert_eq!(state.pow(1), state.pow(1));
    assert_ne!(state.pow(1), state.pow(2));
}
