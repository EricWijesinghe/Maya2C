use std::panic::{AssertUnwindSafe, catch_unwind};

use p3_field::PrimeCharacteristicRing as _;
use p3_matrix::Matrix as _;
use p3_matrix::dense::RowMajorMatrix;

use super::joinsplit::{self, JoinSplitAir, in_reg, input};
use super::note::{Note, SpendingKey, nullifier_right, value_digest};
use super::tree::{CommitmentTree, merkle_path};
use super::wallet::{self, Payment, Spend};
use super::*;
use crate::gadgets::{poseidon, state};
use crate::hash::{Digest, F, WIDTH, compress};

/// A chain's-eye view: the tree, its leaves, and the notes in it.
#[derive(Default)]
struct Chain {
    tree: CommitmentTree,
    leaves: Vec<Digest>,
}

impl Chain {
    fn add(&mut self, public: &JoinSplitPublic) {
        for cm in &public.commitments {
            self.tree.append(*cm).expect("append");
            self.leaves.push(*cm);
        }
    }

    fn spend<'a>(&self, note: Note, key: &'a SpendingKey) -> Spend<'a> {
        let index = self
            .leaves
            .iter()
            .position(|l| *l == note.commitment())
            .expect("note in tree") as u64;
        Spend {
            note,
            key,
            path: merkle_path(&self.leaves, index).expect("path"),
        }
    }
}

#[test]
fn shield_transfer_unshield_round_trip() {
    let (alice, bob) = (
        SpendingKey::random().expect("key"),
        SpendingKey::random().expect("key"),
    );
    let mut chain = Chain::default();

    let built = wallet::shield(1_000, 10, alice.address(), chain.tree.root()).expect("build");
    let (proof, public) = prove(&built.witness).expect("prove shield");
    assert_eq!(verify(&proof, &public), Ok(()));
    chain.add(&public);
    let alice_note = built.created[0];

    let payments = [
        Payment {
            to: bob.address(),
            amount: 600,
        },
        Payment {
            to: alice.address(),
            amount: 385,
        },
    ];
    let built = wallet::transfer(
        vec![chain.spend(alice_note, &alice)],
        payments.to_vec(),
        chain.tree.root(),
        5,
    )
    .expect("build");
    let (proof, public) = prove(&built.witness).expect("prove transfer");
    assert_eq!(verify(&proof, &public), Ok(()));
    chain.add(&public);

    let recipient = [7u8; 32];
    let built = wallet::unshield(
        vec![chain.spend(built.created[0], &bob)],
        vec![],
        590,
        10,
        recipient,
        chain.tree.root(),
    )
    .expect("build");
    let (proof, public) = prove(&built.witness).expect("prove unshield");
    assert_eq!(verify(&proof, &public), Ok(()));

    // The recipient is bound: a miner cannot redirect the withdrawal.
    let mut redirected = public;
    redirected.recipient = [8u8; 32];
    assert!(verify(&proof, &redirected).is_err());
    // Nor inflate it.
    let mut inflated = public;
    inflated.public_out += 1;
    assert!(verify(&proof, &inflated).is_err());
}

#[test]
fn native_checks_refuse_what_the_air_would() {
    let (alice, mallory) = (
        SpendingKey::random().expect("key"),
        SpendingKey::random().expect("key"),
    );
    let mut chain = Chain::default();
    let built = wallet::shield(100, 0, alice.address(), chain.tree.root()).expect("build");
    let (_, public) = prove(&built.witness).expect("prove");
    chain.add(&public);
    let note = built.created[0];

    let not_hers = wallet::unshield(
        vec![chain.spend(note, &mallory)],
        vec![],
        100,
        0,
        [0; 32],
        chain.tree.root(),
    )
    .expect("build");
    assert!(prove(&not_hers.witness).is_err(), "wrong owner");
    let unbalanced = wallet::unshield(
        vec![chain.spend(note, &alice)],
        vec![],
        101,
        0,
        [0; 32],
        chain.tree.root(),
    )
    .expect("build");
    assert!(prove(&unbalanced.witness).is_err(), "value from nothing");
    let stale = wallet::unshield(
        vec![chain.spend(note, &alice)],
        vec![],
        100,
        0,
        [0; 32],
        compress(&note.commitment(), &note.commitment()),
    )
    .expect("build");
    assert!(prove(&stale.witness).is_err(), "not under that anchor");
}

// ------------------------------------------------------ consistent lies

fn refused(trace: RowMajorMatrix<F>, public: &JoinSplitPublic) -> bool {
    let air = JoinSplitAir::default();
    let Ok(values) = public.to_field_elements() else {
        return true;
    };
    let quiet = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = catch_unwind(AssertUnwindSafe(|| crate::prove(&air, trace, &values)));
    std::panic::set_hook(quiet);
    match outcome {
        Err(_) | Ok(Err(_)) => true,
        Ok(Ok(proof)) => crate::verify(&air, &proof, &values).is_err(),
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

/// A funded chain and a one-input unshield witness for `value`.
fn fixture(value: u64, out: u64, fee: u64) -> (JoinSplitWitness, SpendingKey) {
    let alice = SpendingKey::random().expect("key");
    let mut chain = Chain::default();
    let built = wallet::shield(value, 0, alice.address(), chain.tree.root()).expect("build");
    let (_, public) = prove(&built.witness).expect("prove");
    chain.add(&public);
    let spend = chain.spend(built.created[0], &alice);
    let built =
        wallet::unshield(vec![spend], vec![], out, fee, [1; 32], chain.tree.root()).expect("build");
    (built.witness, alice)
}

#[test]
fn the_honest_fixture_is_accepted() {
    let (w, _) = fixture(100, 90, 10);
    assert!(!refused(witness::trace(&w), &witness::public_of(&w)));
}

#[test]
fn conservation_refuses_value_from_nothing() {
    let (w, _) = fixture(100, 100, 10);
    assert!(refused(witness::trace(&w), &witness::public_of(&w)));
}

#[test]
fn the_range_check_refuses_a_field_wrapped_limb() {
    // Output 0's low limb becomes p − 1 ("−1"); its commitment is recomputed
    // over that limb and the fee chosen so the limb equation holds mod p.
    let (mut w, _) = fixture(5, 0, 5);
    w.public_out = 0;
    let mut trace = witness::trace(&w);
    let minus_one = F::ZERO - F::ONE;
    set_register(
        &mut trace,
        joinsplit::out_reg(0) + joinsplit::output::V,
        minus_one,
    );
    let mut limbs_out = value_digest(0);
    limbs_out[0] = minus_one;
    let out = w.outputs[0];
    let row = joinsplit::INPUTS * joinsplit::INPUT_ROWS;
    rehash(&mut trace, row, state(&limbs_out, &out.address.0));
    let layer = compress(&limbs_out, &out.address.0);
    rehash(&mut trace, row + 1, state(&layer, &out.nonce_digest()));
    let mut public = witness::public_of(&w);
    public.commitments[0] = compress(&layer, &out.nonce_digest());
    public.fee = 6;
    assert!(refused(trace, &public));
}

#[test]
fn a_dummy_input_cannot_carry_value() {
    // Mark the real input a dummy (so no Merkle root is checked) and give the
    // *other*, genuine dummy slot a value of 50, recomputing its commitment.
    let (mut w, _) = fixture(100, 100, 0);
    w.inputs[1].note.value = 50;
    w.public_out = 150;
    let trace = witness::trace(&w);
    assert!(refused(trace, &witness::public_of(&w)));
}

#[test]
fn a_note_owned_by_someone_else_cannot_be_spent() {
    // Mallory's key on the key row; the victim's address on the layer row,
    // so the commitment and root are the victim's real ones.
    let (mut w, _) = fixture(30, 30, 0);
    let victim = w.inputs[0].note.address;
    w.inputs[0].sk = SpendingKey::random().expect("key").0.to_field();
    let mut trace = witness::trace(&w);
    rehash(&mut trace, 1, state(&value_digest(30), &victim.0));
    let cm_row = state(
        &compress(&value_digest(30), &victim.0),
        &w.inputs[0].note.nonce_digest(),
    );
    rehash(&mut trace, 2, cm_row);
    assert!(refused(trace, &witness::public_of(&w)));
}

#[test]
fn the_nullifier_must_use_the_note_s_own_rho() {
    let (w, _) = fixture(50, 50, 0);
    let mut trace = witness::trace(&w);
    let other = [F::ONE, F::TWO, F::ONE, F::TWO];
    rehash(
        &mut trace,
        joinsplit::INPUT_ROWS - 1,
        state(&w.inputs[0].sk, &nullifier_right(&other)),
    );
    let mut public = witness::public_of(&w);
    public.nullifiers[0] = compress(&w.inputs[0].sk, &nullifier_right(&other));
    assert!(refused(trace, &public));
}

#[test]
fn a_real_input_marked_dummy_gains_nothing() {
    // Claiming the funded note is a dummy skips its root check but forces
    // its value to zero; with the value kept, the dummy guard refuses.
    let (w, _) = fixture(40, 40, 0);
    let mut trace = witness::trace(&w);
    set_register(&mut trace, in_reg(0) + input::DUMMY, F::ONE);
    assert!(refused(trace, &witness::public_of(&w)));
}

#[test]
fn report_proof_size_and_security() {
    let (w, _) = fixture(10, 10, 0);
    let (proof, public) = prove(&w).expect("prove");
    assert_eq!(verify(&proof, &public), Ok(()));
    let bits = crate::proof::degree_bits(&proof).expect("bits");
    let (conj, proven) = crate::security_bits(&JoinSplitAir::default(), bits);
    println!(
        "zk-stark: joinsplit proof {} B (2^{bits} rows); conjectured {conj} bits, proven {proven} bits",
        proof.as_bytes().len()
    );
    assert!(conj >= 100);
}

#[test]
fn the_circuit_stays_unaudited_until_someone_audits_it() {
    // Six guards refuse a value-bearing chain on this flag. Flipping it is a
    // decision with an audit report behind it, and this test is where that
    // decision has to be written down.
    assert!(!circuit_is_audited());
}
