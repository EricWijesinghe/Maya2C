//! The gate.
//!
//! Every test here answers one question: does this crate's ArgonBlake agree
//! with the node's, bit for bit? Nothing else about the miner matters if the
//! answer is no — a fast miner computing the wrong digest is a slow way to
//! produce rejected blocks.
//!
//! The comparison is against `custom_l1_node::crypto::argon_blake::argon_blake_hash`
//! itself, not against a vector copied out of it. A copied vector agrees with
//! whatever it was copied from on the day it was copied; a call agrees with
//! whatever the node does today.
//!
//! These run on the CPU. When the CUDA kernel lands, `fill_lane` is swapped for
//! the device path and the same assertions carry over unchanged — which is the
//! reason for building the split this way round.

use custom_l1_node::core::HEADER_LEN as NODE_HEADER_LEN;
use custom_l1_node::crypto::argon_blake::argon_blake_hash as node_hash;
use maya_cuda_miner::{HEADER_LEN, argon_blake_hash as split_hash, set_nonce};

/// `KAT_DIGEST_HEADER` from `tests/crypto_tests.rs`: ArgonBlake over a
/// full-length 144-byte header. Reproduced so a divergence says *which* of the
/// two implementations moved.
///
/// Not the original 112-byte `KAT_DIGEST`. The split hasher only accepts
/// full-length headers, and the node keeps that older vector to show that the
/// function itself did not change when the header grew.
const KAT_DIGEST: &str = "5b3992c971393e4127ccd78fb00c2d82217ecdc41b3a0c3caab0629531a78e44";

/// A cheap deterministic byte stream. Not cryptographic — it only has to walk
/// the header through values a real chain would produce.
fn pseudo_random_header(seed: u64) -> [u8; HEADER_LEN] {
    let mut header = [0u8; HEADER_LEN];
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for byte in header.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state >> 24) as u8;
    }
    header
}

#[test]
fn the_header_length_agrees_with_the_node() {
    // This crate hardcodes 144 rather than depending on the node at runtime.
    // If the header ever grows, that constant has to move with it, and this is
    // where the miner finds out.
    assert_eq!(HEADER_LEN, NODE_HEADER_LEN);
}

#[test]
fn the_split_reproduces_the_frozen_known_answer() {
    let header = [0x5Au8; HEADER_LEN];
    let digest = split_hash(&header).expect("hash must succeed");

    assert_eq!(
        hex::encode(digest),
        KAT_DIGEST,
        "the host/GPU split does not reproduce ArgonBlake's frozen vector"
    );
}

#[test]
fn the_split_matches_the_node_on_the_known_answer() {
    let header = [0x5Au8; HEADER_LEN];

    assert_eq!(
        split_hash(&header).expect("split hash must succeed"),
        node_hash(&header).expect("node hash must succeed"),
    );
}

#[test]
fn the_split_matches_the_node_across_random_headers() {
    // Twelve headers, not one. The Argon2id addressing rule changes at the
    // slice-1/slice-2 boundary and the reference index is data-dependent from
    // there on, so a bug in the dependent half is reachable from some seeds and
    // not others. One vector can pass while the implementation is wrong.
    for seed in 0..12u64 {
        let header = pseudo_random_header(seed);

        assert_eq!(
            split_hash(&header).expect("split hash must succeed"),
            node_hash(&header).expect("node hash must succeed"),
            "divergence on seed {seed}"
        );
    }
}

#[test]
fn the_split_matches_the_node_across_the_nonce_field() {
    // What the miner actually varies. Includes the boundaries a 32-bit
    // truncation bug would survive and a 64-bit one would not.
    let mut header = [0x11u8; HEADER_LEN];

    for nonce in [0u64, 1, 255, 256, u64::from(u32::MAX), 1 << 32, u64::MAX] {
        set_nonce(&mut header, nonce).expect("header length is correct");

        assert_eq!(
            split_hash(&header).expect("split hash must succeed"),
            node_hash(&header).expect("node hash must succeed"),
            "divergence at nonce {nonce}"
        );
    }
}

#[test]
fn a_single_bit_flip_changes_the_digest() {
    let base = [0u8; HEADER_LEN];
    let mut flipped = base;
    flipped[0] = 1;

    assert_ne!(
        split_hash(&base).expect("hash must succeed"),
        split_hash(&flipped).expect("hash must succeed"),
    );
}

#[test]
fn the_split_is_deterministic() {
    let header = pseudo_random_header(99);

    assert_eq!(
        split_hash(&header).expect("hash must succeed"),
        split_hash(&header).expect("hash must succeed"),
        "proof-of-work is unverifiable unless hashing is deterministic"
    );
}
