//! Circuit-level tests for the joinsplit statement.
//!
//! These exercise the constraint system directly rather than going through
//! Groth16, which is both far faster and a sharper signal: a satisfied
//! constraint system is what a proof attests to, so an unsatisfiable system is
//! the failure a forged spend should produce.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ark_bls12_381::Fr;
use ark_ff::Zero;
use ark_relations::gr1cs::{ConstraintSynthesizer, ConstraintSystem};

use maya_zk_privacy::circuit::{JoinSplitCircuit, JoinSplitWitness, OutputWitness, SpendWitness};
use maya_zk_privacy::note::{Note, SpendingKey};
use maya_zk_privacy::params::TREE_DEPTH;
use maya_zk_privacy::tree::{CommitmentTree, MerklePath, merkle_path};

/// A spending key derived from a small seed.
fn key(seed: u64) -> SpendingKey {
    SpendingKey(Fr::from(seed + 1))
}

/// A path of the right shape that authenticates nothing, for dummy inputs.
fn null_path() -> MerklePath {
    MerklePath {
        siblings: vec![Fr::zero(); TREE_DEPTH],
        index: 0,
    }
}

/// A dummy input note: zero value, random-ish `rho`, path unchecked.
fn dummy_input(seed: u64) -> SpendWitness {
    SpendWitness {
        note: Note::dummy(Fr::from(seed + 9_000), Fr::from(seed + 9_500)),
        key: key(seed + 77),
        path: null_path(),
        is_dummy: true,
    }
}

/// Builds a tree holding `notes`, returning the tree and their paths.
fn tree_with(notes: &[Note]) -> (CommitmentTree, Vec<MerklePath>) {
    let mut tree = CommitmentTree::new();
    let leaves: Vec<Fr> = notes.iter().map(Note::commitment).collect();
    for leaf in &leaves {
        tree.append(*leaf).expect("append");
    }
    let paths = (0..leaves.len())
        .map(|index| merkle_path(&leaves, index as u64).expect("path"))
        .collect();
    (tree, paths)
}

/// Is the constraint system satisfied by this witness?
fn satisfied(witness: JoinSplitWitness) -> bool {
    let cs = ConstraintSystem::<Fr>::new_ref();
    JoinSplitCircuit::new(witness)
        .generate_constraints(cs.clone())
        .expect("synthesis");
    cs.is_satisfied().expect("satisfiability is decidable")
}

/// A shield: transparent value in, two shielded notes out, no real inputs.
fn shield_witness(amount: u64, fee: u64) -> JoinSplitWitness {
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
fn a_shield_satisfies_the_circuit() {
    assert!(satisfied(shield_witness(1_000, 10)));
}

#[test]
fn a_shielded_transfer_satisfies_the_circuit() {
    let sender = key(1);
    let note = Note::new(1_000, sender.address(), Fr::from(10u64), Fr::from(11u64));
    let (tree, paths) = tree_with(&[note]);
    let recipient = key(2).address();

    let witness = JoinSplitWitness {
        inputs: [
            SpendWitness {
                note,
                key: sender,
                path: paths[0].clone(),
                is_dummy: false,
            },
            dummy_input(5),
        ],
        outputs: [
            OutputWitness {
                note: Note::new(600, recipient, Fr::from(20u64), Fr::from(21u64)),
            },
            OutputWitness {
                // Change back to the sender.
                note: Note::new(395, sender.address(), Fr::from(22u64), Fr::from(23u64)),
            },
        ],
        anchor: tree.root(),
        public_in: 0,
        public_out: 0,
        fee: 5,
        recipient: [0u8; 32],
    };

    assert!(satisfied(witness));
}

#[test]
fn an_unshield_satisfies_the_circuit() {
    let sender = key(1);
    let note = Note::new(500, sender.address(), Fr::from(30u64), Fr::from(31u64));
    let (tree, paths) = tree_with(&[note]);

    let witness = JoinSplitWitness {
        inputs: [
            SpendWitness {
                note,
                key: sender,
                path: paths[0].clone(),
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

    assert!(satisfied(witness));
}

#[test]
fn creating_value_from_nothing_is_unsatisfiable() {
    // Outputs exceed inputs: the conservation constraint must reject it.
    let mut witness = shield_witness(1_000, 10);
    witness.outputs[1] = OutputWitness {
        note: Note::new(1, key(1).address(), Fr::from(102u64), Fr::from(103u64)),
    };
    assert!(!satisfied(witness));
}

#[test]
fn spending_a_note_that_is_not_in_the_tree_is_unsatisfiable() {
    let sender = key(1);
    // The tree holds one note, but we try to spend a different one.
    let in_tree = Note::new(1_000, sender.address(), Fr::from(10u64), Fr::from(11u64));
    let forged = Note::new(9_999, sender.address(), Fr::from(12u64), Fr::from(13u64));
    let (tree, paths) = tree_with(&[in_tree]);

    let witness = JoinSplitWitness {
        inputs: [
            SpendWitness {
                note: forged,
                key: sender,
                path: paths[0].clone(),
                is_dummy: false,
            },
            dummy_input(7),
        ],
        outputs: [
            OutputWitness {
                note: Note::new(9_999, sender.address(), Fr::from(20u64), Fr::from(21u64)),
            },
            OutputWitness {
                note: Note::new(0, sender.address(), Fr::from(22u64), Fr::from(23u64)),
            },
        ],
        anchor: tree.root(),
        public_in: 0,
        public_out: 0,
        fee: 0,
        recipient: [0u8; 32],
    };

    assert!(!satisfied(witness));
}

#[test]
fn spending_someone_elses_note_is_unsatisfiable() {
    // The note belongs to key(1); we try to spend it with key(2). The address
    // is derived from the key inside the circuit, so the commitment will not
    // match what is in the tree.
    let owner = key(1);
    let thief = key(2);
    let note = Note::new(100, owner.address(), Fr::from(50u64), Fr::from(51u64));
    let (tree, paths) = tree_with(&[note]);

    let witness = JoinSplitWitness {
        inputs: [
            SpendWitness {
                note,
                key: thief,
                path: paths[0].clone(),
                is_dummy: false,
            },
            dummy_input(8),
        ],
        outputs: [
            OutputWitness {
                note: Note::new(100, thief.address(), Fr::from(60u64), Fr::from(61u64)),
            },
            OutputWitness {
                note: Note::new(0, thief.address(), Fr::from(62u64), Fr::from(63u64)),
            },
        ],
        anchor: tree.root(),
        public_in: 0,
        public_out: 0,
        fee: 0,
        recipient: [0u8; 32],
    };

    assert!(!satisfied(witness));
}

#[test]
fn a_dummy_input_carrying_value_is_unsatisfiable() {
    // The escape hatch a dummy would otherwise open: unchecked Merkle path
    // plus nonzero value would mint coins freely.
    let mut witness = shield_witness(1_000, 10);
    witness.inputs[0] = SpendWitness {
        note: Note::new(
            500,
            key(3).address(),
            Fr::from(9_000u64),
            Fr::from(9_500u64),
        ),
        key: key(3),
        path: null_path(),
        is_dummy: true,
    };
    // Absorb the extra value into an output so only the dummy rule is broken.
    witness.outputs[1] = OutputWitness {
        note: Note::new(500, key(1).address(), Fr::from(102u64), Fr::from(103u64)),
    };

    assert!(!satisfied(witness));
}

#[test]
fn proving_against_the_wrong_anchor_is_unsatisfiable() {
    let sender = key(1);
    let note = Note::new(100, sender.address(), Fr::from(70u64), Fr::from(71u64));
    let (_, paths) = tree_with(&[note]);

    let mut other = CommitmentTree::new();
    other.append(Fr::from(424_242u64)).expect("append");

    let witness = JoinSplitWitness {
        inputs: [
            SpendWitness {
                note,
                key: sender,
                path: paths[0].clone(),
                is_dummy: false,
            },
            dummy_input(9),
        ],
        outputs: [
            OutputWitness {
                note: Note::new(100, sender.address(), Fr::from(80u64), Fr::from(81u64)),
            },
            OutputWitness {
                note: Note::new(0, sender.address(), Fr::from(82u64), Fr::from(83u64)),
            },
        ],
        // A root the note was never under.
        anchor: other.root(),
        public_in: 0,
        public_out: 0,
        fee: 0,
        recipient: [0u8; 32],
    };

    assert!(!satisfied(witness));
}

#[test]
fn public_inputs_match_the_notes_used() {
    let witness = shield_witness(1_000, 10);
    let public = witness.public();

    assert_eq!(public.commitments[0], witness.outputs[0].note.commitment());
    assert_eq!(public.commitments[1], witness.outputs[1].note.commitment());
    assert_eq!(
        public.nullifiers[0],
        witness.inputs[0].note.nullifier(&witness.inputs[0].key)
    );
    assert_eq!(public.to_field_elements().len(), 10);
}

#[test]
fn value_totals_balance_for_each_operation() {
    let shield = shield_witness(1_000, 10);
    assert_eq!(shield.input_total(), shield.output_total());
}
