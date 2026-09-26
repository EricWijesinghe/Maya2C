//! The gadgets: completeness, and a consistent lie for every constraint.
//!
//! Each negative test builds a trace in which *everything* is recomputed from
//! the lie — digests, public values — except the one relation the constraint
//! under test guards, so no other constraint can be the one that refuses it
//! (invariant 23's lesson). "Refuses" means: the prover's own constraint check
//! panics (debug builds), or it produces a proof the verifier rejects.

// Traces are built column by column, as the AIR reads them (see lib.rs).
#![allow(clippy::needless_range_loop)]

use std::panic::{AssertUnwindSafe, catch_unwind};

use maya_zk_stark::gadgets::key::{self, KeyAir};
use maya_zk_stark::gadgets::merkle::{self, MerkleAir, MerklePath};
use maya_zk_stark::gadgets::range::{self, RangeAir};
use maya_zk_stark::gadgets::{SecretDigest, poseidon, state};
use maya_zk_stark::hash::{DIGEST, Digest, F, compress};
use maya_zk_stark::proof::StarkAir;
use p3_field::PrimeCharacteristicRing as _;
use p3_matrix::Matrix as _;
use p3_matrix::dense::RowMajorMatrix;

fn d(seed: u32) -> Digest {
    core::array::from_fn(|i| {
        F::from_u32(seed.wrapping_mul(31).wrapping_add(i as u32) % 0x7800_0001)
    })
}

/// True if the statement is refused: by the prover's constraint check or by
/// the verifier.
fn refused<A: StarkAir>(air: &A, trace: RowMajorMatrix<F>, public: &[F]) -> bool {
    let quiet = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        maya_zk_stark::prove(air, trace, public)
    }));
    std::panic::set_hook(quiet);
    match outcome {
        Err(_) | Ok(Err(_)) => true,
        Ok(Ok(proof)) => maya_zk_stark::verify(air, &proof, public).is_err(),
    }
}

fn set(trace: &mut RowMajorMatrix<F>, row: usize, col: usize, value: F) {
    let width = trace.width();
    trace.values[row * width + col] = value;
}

/// Rewrites row `row`'s permutation columns for a new input `state`, keeping
/// the gadget columns — the "recompute everything downstream" half of a lie.
fn rehash(trace: &mut RowMajorMatrix<F>, row: usize, input: [F; 16]) {
    let perm = poseidon::permutation_rows(&[input], 1).remove(0);
    let width = trace.width();
    trace.values[row * width..row * width + perm.len()].copy_from_slice(&perm);
}

#[test]
fn the_air_permutation_is_the_native_permutation() {
    let (l, r) = (d(1), d(2));
    let rows = poseidon::permutation_rows(&[state(&l, &r)], 1);
    assert_eq!(poseidon::digest(&rows[0]), compress(&l, &r));
}

// ------------------------------------------------------------------ range

#[test]
fn a_value_in_range_proves_and_verifies() {
    let blind = d(7);
    let (proof, c) = range::prove(123_456_789, &blind, 32).expect("prove");
    assert_eq!(range::verify(&proof, &c, 32), Ok(()));
    assert!(
        range::verify(&proof, &d(99), 32).is_err(),
        "wrong commitment"
    );
    assert!(
        range::prove(1 << 40, &blind, 32).is_err(),
        "native check refuses"
    );
}

#[test]
fn range_refuses_a_value_above_the_bound() {
    // Honest bits and an honest commitment for 2^40: only the bound fails.
    let (value, blind) = (1u64 << 40, d(3));
    let c = range::commit(value, &blind);
    assert!(refused(
        &RangeAir::new(32).expect("air"),
        range::trace(value, &blind),
        &c
    ));
}

#[test]
fn range_refuses_a_non_boolean_bit() {
    // value 2: bits [0,1] honestly; the lie [2,0] recomposes to the same 2.
    let (value, blind) = (2u64, d(4));
    let c = range::commit(value, &blind);
    let mut trace = range::trace(value, &blind);
    set(&mut trace, 0, poseidon::POSEIDON_COLS, F::TWO);
    set(&mut trace, 0, poseidon::POSEIDON_COLS + 1, F::ZERO);
    assert!(refused(&RangeAir::new(32).expect("air"), trace, &c));
}

#[test]
fn range_refuses_bits_that_do_not_recompose_the_committed_limb() {
    let (value, blind) = (5u64, d(5));
    let c = range::commit(value, &blind);
    let mut trace = range::trace(value, &blind);
    set(&mut trace, 0, poseidon::POSEIDON_COLS + 1, F::ONE); // bits now say 7
    assert!(refused(&RangeAir::new(32).expect("air"), trace, &c));
}

#[test]
fn range_refuses_a_nonzero_padding_input() {
    // Hide extra value in input[2] and recompute the commitment over it.
    let (value, blind) = (9u64, d(6));
    let mut trace = range::trace(value, &blind);
    let mut input = state(
        &[
            F::from_u64(value),
            F::ZERO,
            F::ONE,
            F::ZERO,
            F::ZERO,
            F::ZERO,
            F::ZERO,
            F::ZERO,
        ],
        &blind,
    );
    input[2] = F::ONE;
    rehash(&mut trace, 0, input);
    let c: Digest = poseidon::digest(&poseidon::permutation_rows(&[input], 1)[0]);
    assert!(refused(&RangeAir::new(32).expect("air"), trace, &c));
}

// ----------------------------------------------------------------- merkle

fn path(depth: usize, index: u64) -> MerklePath {
    MerklePath {
        siblings: (0..depth).map(|i| d(100 + i as u32)).collect(),
        index,
    }
}

#[test]
fn a_leaf_proves_membership_and_a_wrong_root_is_rejected() {
    let (leaf, p) = (d(9), path(16, 0b1011_0010_0001_0110));
    let (proof, root) = merkle::prove(&leaf, &p).expect("prove");
    assert_eq!(merkle::verify(&proof, &leaf, &root, 16), Ok(()));
    assert!(merkle::verify(&proof, &leaf, &d(1), 16).is_err());
    assert!(
        merkle::verify(&proof, &d(10), &root, 16).is_err(),
        "another leaf"
    );
}

fn merkle_public(leaf: &Digest, root: &Digest) -> Vec<F> {
    leaf.iter().chain(root).copied().collect()
}

#[test]
fn merkle_refuses_a_non_boolean_path_bit() {
    // Row 0 bit = 2 with the permutation input it implies, and every later
    // level recomputed from that row's digest.
    let (leaf, p) = (d(11), path(4, 0));
    let mut trace = merkle::trace(&leaf, &p);
    let (c, s) = (leaf, p.siblings[0]);
    let two = F::TWO;
    let left: Digest = core::array::from_fn(|j| c[j] + two * (s[j] - c[j]));
    let right: Digest = core::array::from_fn(|j| s[j] + two * (c[j] - s[j]));
    set(&mut trace, 0, poseidon::POSEIDON_COLS, two);
    rehash(&mut trace, 0, state(&left, &right));
    let mut cur = compress(&left, &right);
    for level in 1..4 {
        let base = poseidon::POSEIDON_COLS + 1;
        for j in 0..DIGEST {
            set(&mut trace, level, base + j, cur[j]);
        }
        rehash(&mut trace, level, state(&cur, &p.siblings[level]));
        cur = compress(&cur, &p.siblings[level]);
    }
    assert!(refused(
        &MerkleAir::new(4).expect("air"),
        trace,
        &merkle_public(&leaf, &cur)
    ));
}

#[test]
fn merkle_refuses_a_broken_chain_between_levels() {
    // Level 1 starts from an arbitrary node instead of level 0's digest; the
    // rest, and the root, follow honestly from it.
    let (leaf, p) = (d(12), path(4, 0));
    let mut trace = merkle::trace(&leaf, &p);
    let mut cur = d(555);
    for level in 1..4 {
        for j in 0..DIGEST {
            set(&mut trace, level, poseidon::POSEIDON_COLS + 1 + j, cur[j]);
        }
        rehash(&mut trace, level, state(&cur, &p.siblings[level]));
        cur = compress(&cur, &p.siblings[level]);
    }
    assert!(refused(
        &MerkleAir::new(4).expect("air"),
        trace,
        &merkle_public(&leaf, &cur)
    ));
}

#[test]
fn merkle_refuses_a_leaf_other_than_the_public_one() {
    let (leaf, p) = (d(13), path(4, 5));
    let root = p.root(&leaf);
    assert!(refused(
        &MerkleAir::new(4).expect("air"),
        merkle::trace(&leaf, &p),
        &merkle_public(&d(14), &root)
    ));
}

#[test]
fn merkle_refuses_a_root_other_than_the_path_s() {
    let (leaf, p) = (d(15), path(4, 3));
    assert!(refused(
        &MerkleAir::new(4).expect("air"),
        merkle::trace(&leaf, &p),
        &merkle_public(&leaf, &d(16))
    ));
}

// -------------------------------------------------------------------- key

#[test]
fn key_knowledge_is_bound_to_its_message() {
    let sk = SecretDigest::from_words([1, 2, 3, 4, 5, 6, 7, 8]);
    let (proof, pk) = key::prove(&sk, &d(20)).expect("prove");
    assert_eq!(key::verify(&proof, &pk, &d(20)), Ok(()));
    assert!(key::verify(&proof, &pk, &d(21)).is_err(), "another message");
    assert!(key::verify(&proof, &d(22), &d(20)).is_err(), "another key");
    assert_eq!(pk, key::public_key(&sk.to_field()));
}

#[test]
fn key_refuses_a_hash_under_another_domain() {
    // sk hashed with a non-KEY right half; pk recomputed from exactly that.
    let sk = d(30);
    let mut trace = key::trace(&sk);
    let other = state(&sk, &d(31));
    rehash(&mut trace, 0, other);
    let pk = compress(&sk, &d(31));
    let public: Vec<F> = pk.iter().chain(&d(32)).copied().collect();
    assert!(refused(&KeyAir::default(), trace, &public));
}

#[test]
fn a_proof_does_not_survive_one_flipped_byte() {
    let sk = SecretDigest::from_words([9; 8]);
    let (proof, pk) = key::prove(&sk, &d(40)).expect("prove");
    let mut bytes = proof.as_bytes().to_vec();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 1;
    let tampered = maya_zk_stark::Proof::from_bytes(bytes);
    assert!(key::verify(&tampered, &pk, &d(40)).is_err());
}
