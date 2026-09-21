//! Building joinsplits from a wallet's point of view.
//!
//! Every joinsplit has exactly two inputs and two outputs; the builder pads
//! with zero-value dummies, so an observer cannot tell a one-note spend from
//! a two-note one.

use crate::ZkError;
use crate::gadgets::merkle::MerklePath;
use crate::hash::{DIGEST, Digest, F};

use super::joinsplit::{INPUTS, OUTPUTS};
use super::note::{Address, Note, SpendingKey};
use super::tree::TREE_DEPTH;
use super::witness::{JoinSplitWitness, SpendWitness};

/// A note the wallet spends.
pub struct Spend<'a> {
    /// The note.
    pub note: Note,
    /// Its key.
    pub key: &'a SpendingKey,
    /// Its path under the anchor.
    pub path: MerklePath,
}

/// A note the wallet creates.
#[derive(Clone, Copy, Debug)]
pub struct Payment {
    /// Recipient.
    pub to: Address,
    /// Amount.
    pub amount: u64,
}

/// The transparent side of a joinsplit.
#[derive(Clone, Copy, Debug, Default)]
pub struct TransparentEdge {
    /// Value entering the pool from the signer.
    pub public_in: u64,
    /// Value leaving the pool to `recipient`.
    pub public_out: u64,
    /// Fee.
    pub fee: u64,
    /// Recipient of `public_out`.
    pub recipient: [u8; 32],
}

/// A built joinsplit: the witness to prove, and the notes it creates (the
/// wallet must remember them to spend them later).
pub struct BuiltJoinSplit {
    /// The witness.
    pub witness: JoinSplitWitness,
    /// Both created notes, real payments first then pads — both are
    /// appended to the tree, so a wallet tracking positions needs both.
    pub created: Vec<Note>,
}

fn dummy_spend() -> Result<SpendWitness, ZkError> {
    let sk = SpendingKey::random()?;
    let mut note = Note::dummy()?;
    note.address = sk.address();
    Ok(SpendWitness {
        note,
        sk: sk.0.to_field(),
        path: MerklePath {
            siblings: vec![[F::default(); DIGEST]; TREE_DEPTH],
            index: 0,
        },
        is_dummy: true,
    })
}

/// Builds a joinsplit from up to two spends and up to two payments.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] for too many spends or payments; entropy failure.
/// Balance is checked when proving, not here.
pub fn build(
    spends: Vec<Spend<'_>>,
    payments: Vec<Payment>,
    anchor: Digest,
    edge: TransparentEdge,
) -> Result<BuiltJoinSplit, ZkError> {
    if spends.len() > INPUTS || payments.len() > OUTPUTS {
        return Err(ZkError::Unsatisfied(
            "a joinsplit takes at most two spends and two payments",
        ));
    }
    let mut inputs = Vec::with_capacity(INPUTS);
    for s in spends {
        inputs.push(SpendWitness {
            note: s.note,
            sk: s.key.0.to_field(),
            path: s.path,
            is_dummy: false,
        });
    }
    while inputs.len() < INPUTS {
        inputs.push(dummy_spend()?);
    }
    let mut created = Vec::with_capacity(OUTPUTS);
    for p in &payments {
        created.push(Note::new(p.amount, p.to)?);
    }
    while created.len() < OUTPUTS {
        created.push(Note::dummy()?);
    }
    let witness = JoinSplitWitness {
        inputs: inputs
            .try_into()
            .map_err(|_| ZkError::Unsatisfied("input count"))?,
        outputs: [created[0], created[1]],
        anchor,
        public_in: edge.public_in,
        public_out: edge.public_out,
        fee: edge.fee,
        recipient: edge.recipient,
    };
    Ok(BuiltJoinSplit { witness, created })
}

/// Transparent `amount` in, one note of `amount − fee` for `to`.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] if the fee exceeds the amount.
pub fn shield(
    amount: u64,
    fee: u64,
    to: Address,
    anchor: Digest,
) -> Result<BuiltJoinSplit, ZkError> {
    let value = amount
        .checked_sub(fee)
        .ok_or(ZkError::Unsatisfied("fee exceeds the amount"))?;
    build(
        Vec::new(),
        vec![Payment { to, amount: value }],
        anchor,
        TransparentEdge {
            public_in: amount,
            fee,
            ..TransparentEdge::default()
        },
    )
}

/// Notes to notes.
///
/// # Errors
///
/// As [`build`].
pub fn transfer(
    spends: Vec<Spend<'_>>,
    payments: Vec<Payment>,
    anchor: Digest,
    fee: u64,
) -> Result<BuiltJoinSplit, ZkError> {
    build(
        spends,
        payments,
        anchor,
        TransparentEdge {
            fee,
            ..TransparentEdge::default()
        },
    )
}

/// Notes to a transparent `recipient`, with optional change notes.
///
/// # Errors
///
/// As [`build`].
pub fn unshield(
    spends: Vec<Spend<'_>>,
    change: Vec<Payment>,
    amount: u64,
    fee: u64,
    recipient: [u8; 32],
    anchor: Digest,
) -> Result<BuiltJoinSplit, ZkError> {
    build(
        spends,
        change,
        anchor,
        TransparentEdge {
            public_out: amount,
            fee,
            recipient,
            public_in: 0,
        },
    )
}
