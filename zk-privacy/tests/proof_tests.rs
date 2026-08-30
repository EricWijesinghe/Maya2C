//! End-to-end Groth16 tests.
//!
//! Slower than the constraint-level tests in `circuit_tests.rs` because each
//! one runs a real setup and proof, so this file covers what only a proof can
//! show: that the verifier accepts a valid spend, rejects a tampered one, and
//! that the verifying key is the one consensus pinned.

use ark_bls12_381::Fr;
use ark_ff::Zero;

use maya_zk_privacy::circuit::{JoinSplitWitness, OutputWitness, SpendWitness};
use maya_zk_privacy::note::{Note, SpendingKey};
use maya_zk_privacy::params::TREE_DEPTH;
use maya_zk_privacy::prove::{self, PROOF_BYTES, check_pinned, verifying_key_hash};
use maya_zk_privacy::tree::{CommitmentTree, MerklePath, merkle_path};

fn key(seed: u64) -> SpendingKey {
    SpendingKey(Fr::from(seed + 1))
}

fn dummy_input(seed: u64) -> SpendWitness {
    SpendWitness {
        note: Note::dummy(Fr::from(seed + 9_000), Fr::from(seed + 9_500)),
        key: key(seed + 77),
        path: MerklePath {
            siblings: vec![Fr::zero(); TREE_DEPTH],
            index: 0,
        },
        is_dummy: true,
    }
}

/// Shields `amount`, paying `fee`, into a single note owned by `key(1)`.
fn shield(amount: u64, fee: u64) -> JoinSplitWitness {
    let owner = key(1).address();
    JoinSplitWitness {
        inputs: [dummy_input(0), dummy_input(1)],
        outputs: [
            OutputWitness {
                note: Note::new(amount - fee, owner, Fr::from(100u64), Fr::from(101u64)),
            },
            OutputWitness {
                note: Note::new(0, owner, Fr::from(102u64), Fr::from(103u64)),
            },
        ],
        anchor: CommitmentTree::new().root(),
        public_in: amount,
        public_out: 0,
        fee,
        recipient: [0u8; 32],
    }
}

#[test]
fn the_setup_is_marked_untrusted() {
    // If this ever flips to true, it must be because real ceremony output
    // replaced the seeded setup — not because someone edited the constant.
    assert!(
        !prove::setup_is_trusted(),
        "the deterministic setup must never claim to be trusted"
    );
}

#[test]
fn pinned_verifying_key_hash_is_current() {
    let (_, vk) = prove::setup();
    let actual = verifying_key_hash(&vk);
    // Printed so a deliberate change can be re-pinned from the test output.
    println!("VERIFYING_KEY_HASH={}", hex::encode(actual));
    check_pinned(&vk).expect("the verifying key must match the pinned hash");
}

#[test]
fn the_setup_is_deterministic() {
    // The property the pin depends on: two runs must agree, or every node
    // would compute a different key and reject each other's proofs.
    let (_, first) = prove::setup();
    let (_, second) = prove::setup();
    assert_eq!(verifying_key_hash(&first), verifying_key_hash(&second));
}

#[test]
fn a_valid_shield_proof_verifies() {
    let witness = shield(1_000, 10);
    let public = witness.public();
    let proof = prove::prove(&witness).expect("prove");

    assert_eq!(proof.len(), PROOF_BYTES);
    assert!(prove::verify(&proof, &public).expect("verify"));
}

#[test]
fn a_valid_transfer_proof_verifies() {
    let sender = key(1);
    let note = Note::new(1_000, sender.address(), Fr::from(10u64), Fr::from(11u64));

    let mut tree = CommitmentTree::new();
    let leaves = vec![note.commitment()];
    tree.append(leaves[0]).expect("append");
    let path = merkle_path(&leaves, 0).expect("path");

    let witness = JoinSplitWitness {
        inputs: [
            SpendWitness {
                note,
                key: sender,
                path,
                is_dummy: false,
            },
            dummy_input(5),
        ],
        outputs: [
            OutputWitness {
                note: Note::new(600, key(2).address(), Fr::from(20u64), Fr::from(21u64)),
            },
            OutputWitness {
                note: Note::new(395, sender.address(), Fr::from(22u64), Fr::from(23u64)),
            },
        ],
        anchor: tree.root(),
        public_in: 0,
        public_out: 0,
        fee: 5,
        recipient: [0u8; 32],
    };

    let public = witness.public();
    let proof = prove::prove(&witness).expect("prove");
    assert!(prove::verify(&proof, &public).expect("verify"));
}

#[test]
fn a_tampered_nullifier_does_not_verify() {
    let witness = shield(1_000, 10);
    let proof = prove::prove(&witness).expect("prove");

    let mut public = witness.public();
    public.nullifiers[0] += Fr::from(1u64);

    assert!(!prove::verify(&proof, &public).expect("verify runs"));
}

#[test]
fn a_tampered_commitment_does_not_verify() {
    let witness = shield(1_000, 10);
    let proof = prove::prove(&witness).expect("prove");

    let mut public = witness.public();
    public.commitments[0] += Fr::from(1u64);

    assert!(!prove::verify(&proof, &public).expect("verify runs"));
}

#[test]
fn redirecting_a_withdrawal_does_not_verify() {
    // The reason `recipient` is a public input: a miner who rewrites it must
    // invalidate the proof rather than steal the withdrawal.
    let sender = key(1);
    let note = Note::new(500, sender.address(), Fr::from(30u64), Fr::from(31u64));

    let mut tree = CommitmentTree::new();
    let leaves = vec![note.commitment()];
    tree.append(leaves[0]).expect("append");

    let witness = JoinSplitWitness {
        inputs: [
            SpendWitness {
                note,
                key: sender,
                path: merkle_path(&leaves, 0).expect("path"),
                is_dummy: false,
            },
            dummy_input(6),
        ],
        outputs: [
            OutputWitness {
                note: Note::new(0, sender.address(), Fr::from(40u64), Fr::from(41u64)),
            },
            OutputWitness {
                note: Note::new(0, sender.address(), Fr::from(42u64), Fr::from(43u64)),
            },
        ],
        anchor: tree.root(),
        public_in: 0,
        public_out: 495,
        fee: 5,
        recipient: [7u8; 32],
    };

    let proof = prove::prove(&witness).expect("prove");
    let mut public = witness.public();
    public.recipient = [8u8; 32];

    assert!(!prove::verify(&proof, &public).expect("verify runs"));
}

#[test]
fn a_tampered_proof_does_not_verify() {
    let witness = shield(1_000, 10);
    let public = witness.public();
    let mut proof = prove::prove(&witness).expect("prove");

    // Flip a bit. Either the point fails to decode, or it decodes and fails
    // the pairing check — both are rejections, neither is an acceptance.
    proof[64] ^= 0x01;
    let accepted = prove::verify(&proof, &public).unwrap_or(false);
    assert!(!accepted);
}

#[test]
fn an_unbalanced_joinsplit_is_refused_before_proving() {
    let mut witness = shield(1_000, 10);
    witness.outputs[1] = OutputWitness {
        note: Note::new(500, key(1).address(), Fr::from(102u64), Fr::from(103u64)),
    };

    let error = prove::prove(&witness).expect_err("must not prove");
    assert!(
        matches!(error, maya_zk_privacy::ZkError::ValueImbalance { .. }),
        "expected a value imbalance, got {error:?}"
    );
}

#[test]
fn two_proofs_of_one_statement_differ() {
    // Groth16 proofs are randomized. Identical proofs would let an observer
    // link two spends of the same statement, which is precisely the linkage a
    // shielded pool exists to prevent.
    let witness = shield(1_000, 10);
    let first = prove::prove(&witness).expect("prove");
    let second = prove::prove(&witness).expect("prove");
    assert_ne!(first, second);
}
