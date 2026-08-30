//! Assembling joinsplits.
//!
//! The circuit has fixed arity — always two inputs and two outputs — but real
//! spends rarely do. This module pads the difference with dummies so that a
//! one-note spend and a two-note spend are the same shape on chain, and derives
//! the blinding factors that keep the new notes hiding.
//!
//! All three shielded operations are the same call with different public value
//! terms; [`shield`], [`transfer`], and [`unshield`] are thin wrappers that name
//! them.

use ark_bls12_381::Fr;
use ark_std::rand::RngCore;

use crate::circuit::{JoinSplitWitness, OutputWitness, SpendWitness};
use crate::error::{Result, ZkError};
use crate::note::{Address, Note, SpendingKey};
use crate::params::{JOINSPLIT_INPUTS, JOINSPLIT_OUTPUTS, TREE_DEPTH};
use crate::prove::random_scalar;
use crate::tree::MerklePath;

/// A note the caller can spend.
#[derive(Clone, Debug)]
pub struct Spend {
    /// The note.
    pub note: Note,
    /// Key that owns it.
    pub key: SpendingKey,
    /// Path proving it sits under the anchor.
    pub path: MerklePath,
}

/// A note to create: who gets it and how much.
#[derive(Clone, Copy, Debug)]
pub struct Payment {
    /// Recipient's shielded address.
    pub to: Address,
    /// Value.
    pub amount: u64,
}

/// What a joinsplit produced, alongside the witness that proves it.
#[derive(Clone, Debug)]
pub struct BuiltJoinSplit {
    /// Private input to the prover.
    pub witness: JoinSplitWitness,
    /// The notes created, in output order.
    ///
    /// The caller must keep these: a note that is not recorded is value that
    /// exists on chain but that nobody can produce a spend for.
    pub created: Vec<Note>,
}

/// A zero-value pad on the input side.
///
/// Its Merkle path is never checked — there is no tree entry to check against —
/// so the circuit constrains it to carry no value instead.
fn dummy_spend<R: RngCore>(rng: &mut R) -> SpendWitness {
    SpendWitness {
        note: Note::dummy(random_scalar(rng), random_scalar(rng)),
        key: SpendingKey(random_scalar(rng)),
        path: MerklePath {
            siblings: vec![Fr::from(0u64); TREE_DEPTH],
            index: 0,
        },
        is_dummy: true,
    }
}

/// A zero-value pad on the output side.
///
/// Addressed randomly rather than back to the sender: a padding note that
/// always went to the same place would be a marker saying "this spend had only
/// one real output".
fn dummy_output<R: RngCore>(rng: &mut R) -> Note {
    Note::new(
        0,
        Address(random_scalar(rng)),
        random_scalar(rng),
        random_scalar(rng),
    )
}

/// The transparent side of a joinsplit: value crossing the pool boundary.
///
/// Grouped rather than passed loose because these four move together — which
/// operation a joinsplit *is* is determined entirely by their combination.
#[derive(Clone, Copy, Debug, Default)]
pub struct TransparentEdge {
    /// Value entering the pool, debited from the transaction's signer.
    pub public_in: u64,
    /// Value leaving the pool, credited to `recipient`.
    pub public_out: u64,
    /// Fee, paid in the clear.
    pub fee: u64,
    /// Transparent account receiving `public_out`.
    pub recipient: [u8; 32],
}

/// Assembles a joinsplit, padding both sides to the circuit's fixed arity.
///
/// # Errors
///
/// Returns [`ZkError::ValueImbalance`] if the sums do not balance, or
/// [`ZkError::ValueOutOfRange`] if there are more spends or payments than the
/// circuit accepts.
pub fn build<R: RngCore>(
    spends: Vec<Spend>,
    payments: Vec<Payment>,
    anchor: Fr,
    edge: TransparentEdge,
    rng: &mut R,
) -> Result<BuiltJoinSplit> {
    let TransparentEdge {
        public_in,
        public_out,
        fee,
        recipient,
    } = edge;

    if spends.len() > JOINSPLIT_INPUTS || payments.len() > JOINSPLIT_OUTPUTS {
        return Err(ZkError::ValueOutOfRange {
            value: spends.len().max(payments.len()) as u128,
        });
    }

    let spent: u128 = spends.iter().map(|s| u128::from(s.note.value)).sum();
    let paid: u128 = payments.iter().map(|p| u128::from(p.amount)).sum();
    let inputs = spent + u128::from(public_in);
    let outputs = paid + u128::from(public_out) + u128::from(fee);
    if inputs != outputs {
        return Err(ZkError::ValueImbalance { inputs, outputs });
    }

    let mut input_witnesses: Vec<SpendWitness> = spends
        .into_iter()
        .map(|spend| SpendWitness {
            note: spend.note,
            key: spend.key,
            path: spend.path,
            is_dummy: false,
        })
        .collect();
    while input_witnesses.len() < JOINSPLIT_INPUTS {
        input_witnesses.push(dummy_spend(rng));
    }

    let mut created: Vec<Note> = payments
        .into_iter()
        .map(|payment| {
            Note::new(
                payment.amount,
                payment.to,
                random_scalar(rng),
                random_scalar(rng),
            )
        })
        .collect();
    while created.len() < JOINSPLIT_OUTPUTS {
        created.push(dummy_output(rng));
    }

    let witness = JoinSplitWitness {
        inputs: [input_witnesses[0].clone(), input_witnesses[1].clone()],
        outputs: [
            OutputWitness { note: created[0] },
            OutputWitness { note: created[1] },
        ],
        anchor,
        public_in,
        public_out,
        fee,
        recipient,
    };

    Ok(BuiltJoinSplit { witness, created })
}

/// Moves transparent value into the pool.
///
/// `amount` is debited from the transparent signer; `amount - fee` arrives as a
/// shielded note owned by `to`.
///
/// # Errors
///
/// As [`build`], plus [`ZkError::ValueImbalance`] if `fee` exceeds `amount`.
pub fn shield<R: RngCore>(
    amount: u64,
    fee: u64,
    to: Address,
    anchor: Fr,
    rng: &mut R,
) -> Result<BuiltJoinSplit> {
    let shielded = amount.checked_sub(fee).ok_or(ZkError::ValueImbalance {
        inputs: u128::from(amount),
        outputs: u128::from(fee),
    })?;
    build(
        Vec::new(),
        vec![Payment {
            to,
            amount: shielded,
        }],
        anchor,
        TransparentEdge {
            public_in: amount,
            fee,
            ..TransparentEdge::default()
        },
        rng,
    )
}

/// Moves value between shielded addresses, revealing nothing but the fee.
///
/// # Errors
///
/// As [`build`].
pub fn transfer<R: RngCore>(
    spends: Vec<Spend>,
    payments: Vec<Payment>,
    anchor: Fr,
    fee: u64,
    rng: &mut R,
) -> Result<BuiltJoinSplit> {
    build(
        spends,
        payments,
        anchor,
        TransparentEdge {
            fee,
            ..TransparentEdge::default()
        },
        rng,
    )
}

/// Moves value out of the pool to a transparent account.
///
/// # Errors
///
/// As [`build`].
pub fn unshield<R: RngCore>(
    spends: Vec<Spend>,
    change: Vec<Payment>,
    amount: u64,
    fee: u64,
    recipient: [u8; 32],
    anchor: Fr,
    rng: &mut R,
) -> Result<BuiltJoinSplit> {
    build(
        spends,
        change,
        anchor,
        TransparentEdge {
            public_out: amount,
            fee,
            recipient,
            ..TransparentEdge::default()
        },
        rng,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_std::rand::SeedableRng;
    use ark_std::rand::rngs::StdRng;

    use crate::tree::CommitmentTree;

    fn rng() -> StdRng {
        StdRng::seed_from_u64(99)
    }

    #[test]
    fn a_shield_pads_both_sides() {
        let mut rng = rng();
        let built = shield(
            1_000,
            10,
            SpendingKey(Fr::from(1u64)).address(),
            CommitmentTree::new().root(),
            &mut rng,
        )
        .expect("build");

        assert!(built.witness.inputs.iter().all(|input| input.is_dummy));
        assert_eq!(built.created.len(), 2);
        assert_eq!(built.created[0].value, 990);
        assert_eq!(built.created[1].value, 0);
        assert_eq!(built.witness.input_total(), built.witness.output_total());
    }

    #[test]
    fn padding_notes_differ_between_builds() {
        // Two shields of the same amount must not produce identical padding,
        // or the pads would correlate spends.
        let mut first = StdRng::seed_from_u64(1);
        let mut second = StdRng::seed_from_u64(2);
        let address = SpendingKey(Fr::from(1u64)).address();
        let anchor = CommitmentTree::new().root();

        let a = shield(1_000, 10, address, anchor, &mut first).expect("build");
        let b = shield(1_000, 10, address, anchor, &mut second).expect("build");

        assert_ne!(
            a.created[0].commitment(),
            b.created[0].commitment(),
            "identical shields must not share a commitment"
        );
        assert_ne!(a.created[1].value, 1);
    }

    #[test]
    fn an_unbalanced_build_is_rejected() {
        let mut rng = rng();
        let error = build(
            Vec::new(),
            vec![Payment {
                to: SpendingKey(Fr::from(1u64)).address(),
                amount: 500,
            }],
            CommitmentTree::new().root(),
            TransparentEdge {
                public_in: 100,
                ..TransparentEdge::default()
            },
            &mut rng,
        )
        .expect_err("must not build");

        assert!(matches!(error, ZkError::ValueImbalance { .. }));
    }

    #[test]
    fn too_many_payments_are_rejected() {
        let mut rng = rng();
        let to = SpendingKey(Fr::from(1u64)).address();
        let error = build(
            Vec::new(),
            vec![Payment { to, amount: 1 }; 3],
            CommitmentTree::new().root(),
            TransparentEdge {
                public_in: 1,
                ..TransparentEdge::default()
            },
            &mut rng,
        )
        .expect_err("must not build");

        assert!(matches!(error, ZkError::ValueOutOfRange { .. }));
    }

    #[test]
    fn a_fee_larger_than_the_shield_is_rejected() {
        let mut rng = rng();
        assert!(
            shield(
                10,
                100,
                SpendingKey(Fr::from(1u64)).address(),
                CommitmentTree::new().root(),
                &mut rng,
            )
            .is_err()
        );
    }
}
