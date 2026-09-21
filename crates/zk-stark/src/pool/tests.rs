use std::panic::{AssertUnwindSafe, catch_unwind};

use p3_matrix::Matrix as _;
use p3_matrix::dense::RowMajorMatrix;

use super::note::{Note, nullifier_right, value_digest};
use super::spend::{self, Mode, SpendAir};
use super::witness::{self, SpendWitness};
use super::*;
use crate::gadgets::key::public_key;
use crate::gadgets::{poseidon, state};
use crate::hash::{WIDTH, compress};

fn keypair(seed: u32) -> (SecretDigest, Digest) {
    let sk = SecretDigest::from_words(core::array::from_fn(|i| seed * 1_000 + i as u32));
    let pk = public_key(&sk.to_field());
    (sk, pk)
}

/// A pool holding one minted note of `value` for `pk`.
fn funded(value: u64, pk: Digest) -> (ShieldedPool, Note, u64) {
    let mut pool = ShieldedPool::default();
    let note = Note::new(value, pk).expect("note");
    let (proof, cm) = prove_mint(&note).expect("mint proof");
    let index = pool.mint(&proof, &cm, value).expect("mint");
    (pool, note, index)
}

#[test]
fn mint_transfer_unshield_round_trip_conserves_value() {
    let (alice_sk, alice) = keypair(1);
    let (bob_sk, bob) = keypair(2);
    let (mut pool, note, index) = funded(1_000_000, alice);
    assert_eq!(pool.locked(), 1_000_000);

    let path = pool.tree().path(index).expect("path");
    let to_bob = Note::new(999_990, bob).expect("note");
    let (proof, stmt) = prove_transfer(&alice_sk, &note, &path, &to_bob, 10).expect("transfer");
    let bob_index = pool.transfer(&proof, &stmt).expect("accepted");
    assert_eq!(pool.locked(), 999_990, "the fee left the pool");

    // Replaying the same spend is a double spend.
    assert_eq!(pool.transfer(&proof, &stmt), Err(PoolError::DoubleSpend));

    let bob_path = pool.tree().path(bob_index).expect("path");
    let (proof, stmt) = prove_unshield(&bob_sk, &to_bob, &bob_path, 999_980, 10).expect("unshield");
    assert_eq!(pool.unshield(&proof, &stmt), Ok(999_980));
    assert_eq!(pool.locked(), 0);
}

#[test]
fn the_pool_refuses_unknown_roots_wrong_modes_and_wrong_owners() {
    let (alice_sk, alice) = keypair(3);
    let (mallory_sk, _) = keypair(4);
    let (mut pool, note, index) = funded(500, alice);
    let path = pool.tree().path(index).expect("path");

    assert!(
        prove_unshield(&mallory_sk, &note, &path, 500, 0).is_err(),
        "not her note"
    );
    assert!(
        prove_unshield(&alice_sk, &note, &path, 501, 0).is_err(),
        "unbalanced"
    );

    let (proof, mut stmt) = prove_unshield(&alice_sk, &note, &path, 500, 0).expect("unshield");
    assert_eq!(pool.transfer(&proof, &stmt), Err(PoolError::WrongMode));
    let real_root = stmt.root;
    stmt.root = compress(&stmt.root, &stmt.root);
    assert_eq!(pool.unshield(&proof, &stmt), Err(PoolError::UnknownRoot));
    stmt.root = real_root;
    stmt.fee = 1; // the proof was for fee 0
    assert!(matches!(
        pool.unshield(&proof, &stmt),
        Err(PoolError::Proof(_))
    ));
}

#[test]
fn a_mint_proof_is_bound_to_its_value() {
    let (_, alice) = keypair(5);
    let note = Note::new(77, alice).expect("note");
    let (proof, cm) = prove_mint(&note).expect("mint");
    assert_eq!(verify_mint(&proof, &cm, 77), Ok(()));
    assert!(verify_mint(&proof, &cm, 78).is_err());
}

// ------------------------------------------------------ consistent lies

fn refused(mode: Mode, trace: RowMajorMatrix<F>, stmt: &SpendStatement) -> bool {
    let air = SpendAir::new(mode);
    let quiet = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        crate::prove(&air, trace, &stmt.public())
    }));
    std::panic::set_hook(quiet);
    match outcome {
        Err(_) | Ok(Err(_)) => true,
        Ok(Ok(proof)) => crate::verify(&air, &proof, &stmt.public()).is_err(),
    }
}

fn rehash(trace: &mut RowMajorMatrix<F>, row: usize, input: [F; WIDTH]) {
    let perm = poseidon::permutation_rows(&[input], 1).remove(0);
    let width = trace.width();
    trace.values[row * width..row * width + perm.len()].copy_from_slice(&perm);
}

fn set_register(trace: &mut RowMajorMatrix<F>, column: usize, value: F) {
    let width = trace.width();
    for row in 0..trace.height() {
        trace.values[row * width + poseidon::POSEIDON_COLS + column] = value;
    }
}

struct Fixture {
    sk: Digest,
    input: Note,
    path: MerklePath,
    output: Note,
}

fn fixture(vin: u64, vout: u64) -> Fixture {
    let (sk, pk) = keypair(9);
    let (pool, input, index) = funded(vin, pk);
    let path = pool.tree().path(index).expect("path");
    let output = Note::new(vout, keypair(10).1).expect("note");
    Fixture {
        sk: sk.to_field(),
        input,
        path,
        output,
    }
}

fn witness_of(f: &Fixture, vout: u64, fee: u64) -> SpendWitness<'_> {
    SpendWitness {
        mode: Mode::Transfer,
        sk: f.sk,
        input: &f.input,
        path: &f.path,
        output: Some(&f.output),
        vout,
        fee,
    }
}

#[test]
fn an_honest_trace_from_the_fixture_is_accepted() {
    // The control: the same builder the lies start from is accepted as is.
    let f = fixture(100, 90);
    let w = witness_of(&f, 90, 10);
    assert!(!refused(
        Mode::Transfer,
        witness::trace(&w),
        &witness::statement(&w)
    ));
}

#[test]
fn conservation_refuses_value_from_nothing() {
    // Output 100 from input 100 with a fee of 10: every hash honest, the
    // statement exactly what that witness produces. Only conservation fails.
    let f = fixture(100, 100);
    let w = witness_of(&f, 100, 10);
    assert!(refused(
        Mode::Transfer,
        witness::trace(&w),
        &witness::statement(&w)
    ));
}

#[test]
fn the_range_check_refuses_a_field_wrapped_output() {
    // vout_lo = p − 1, i.e. "−1": with fee = vin_lo + 1 the limb equation
    // holds mod p, and the output commitment is recomputed over that limb.
    // Only the bit decomposition can refuse it.
    let f = fixture(5, 0);
    let w = witness_of(&f, 0, 5);
    let mut trace = witness::trace(&w);
    let minus_one = F::ZERO - F::ONE;
    set_register(&mut trace, spend::VOUT, minus_one);
    let mut limbs_out = value_digest(0);
    limbs_out[0] = minus_one;
    let layer_in = state(&limbs_out, &f.output.owner);
    rehash(&mut trace, 20, layer_in);
    let layer = compress(&limbs_out, &f.output.owner);
    rehash(&mut trace, 21, state(&layer, &f.output.nonce_digest()));
    let mut stmt = witness::statement(&w);
    stmt.out = compress(&layer, &f.output.nonce_digest());
    stmt.fee = 6; // 5 + 1·... = −1 + 6 mod p
    assert!(refused(Mode::Transfer, trace, &stmt));
}

#[test]
fn the_nullifier_must_use_the_note_s_own_rho() {
    // A fresh nullifier for the same note (so it could be spent twice), built
    // from another rho; the commitment side is untouched.
    let f = fixture(50, 40);
    let w = witness_of(&f, 40, 10);
    let mut trace = witness::trace(&w);
    let other_rho = [
        F::from_u32(1),
        F::from_u32(2),
        F::from_u32(3),
        F::from_u32(4),
    ];
    rehash(&mut trace, 19, state(&f.sk, &nullifier_right(&other_rho)));
    let mut stmt = witness::statement(&w);
    stmt.nullifier = compress(&f.sk, &nullifier_right(&other_rho));
    assert!(refused(Mode::Transfer, trace, &stmt));
}

#[test]
fn a_note_owned_by_someone_else_cannot_be_spent() {
    // Mallory's key on row 0, the victim's pk on row 1 (so the commitment and
    // root are the victim's real ones). Only the pk link can refuse it.
    let (_, victim) = keypair(11);
    let (pool, input, index) = funded(30, victim);
    let path = pool.tree().path(index).expect("path");
    let output = Note::new(30, keypair(12).1).expect("note");
    let mallory = keypair(13).0.to_field();
    let w = SpendWitness {
        mode: Mode::Transfer,
        sk: mallory,
        input: &input,
        path: &path,
        output: Some(&output),
        vout: 30,
        fee: 0,
    };
    let mut trace = witness::trace(&w);
    rehash(&mut trace, 1, state(&value_digest(30), &victim));
    assert!(refused(Mode::Transfer, trace, &witness::statement(&w)));
}

#[test]
fn an_unshield_pays_exactly_the_proved_amount() {
    let f = fixture(20, 0);
    let w = SpendWitness {
        mode: Mode::Unshield,
        sk: f.sk,
        input: &f.input,
        path: &f.path,
        output: None,
        vout: 15,
        fee: 5,
    };
    let trace = witness::trace(&w);
    let mut stmt = witness::statement(&w);
    stmt.out = value_digest(16);
    assert!(refused(Mode::Unshield, trace, &stmt));
}

#[test]
fn report_proof_sizes_and_security() {
    let (sk, pk) = keypair(20);
    let (pool, note, index) = funded(10, pk);
    let path = pool.tree().path(index).expect("path");
    let (spend_proof, _) = prove_unshield(&sk, &note, &path, 10, 0).expect("unshield");
    let (mint_proof, _) = prove_mint(&note).expect("mint");
    let spend_bits = crate::proof::degree_bits(&spend_proof).expect("bits");
    let (conj, proven) = crate::security_bits(&SpendAir::new(Mode::Transfer), spend_bits);
    println!(
        "zk-stark: spend proof {} B (2^{spend_bits} rows), mint proof {} B; spend security conjectured {conj} bits, proven {proven} bits",
        spend_proof.as_bytes().len(),
        mint_proof.as_bytes().len()
    );
    assert!(conj >= 100, "conjectured security {conj} bits");
}
