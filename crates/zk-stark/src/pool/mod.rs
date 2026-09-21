//! The shielded pool — ADR-008.
//!
//! - **Mint:** a transparent value becomes a note; the proof hides the owner.
//! - **Transfer:** a note in the tree is spent (its nullifier published) and a
//!   new note created, value conserved minus a public fee; the proof hides
//!   which note, both owners and both values.
//! - **Unshield:** a note is spent into a public amount.
//!
//! [`ShieldedPool`] is the state machine a chain would run: it keeps the
//! commitment tree, every root it has had, the spent nullifiers, and the
//! transparent value locked in the pool, and it refuses an unknown root, a
//! reused nullifier, a proof that does not verify, and any operation that
//! would take more value out than is locked.

pub mod mint;
pub mod note;
pub mod spend;
pub mod tree;

mod witness;

use std::collections::BTreeSet;

use p3_field::{PrimeCharacteristicRing as _, PrimeField32 as _};

use crate::gadgets::SecretDigest;
use crate::gadgets::merkle::MerklePath;
use crate::gadgets::range::{LIMB_BITS, MAX_BITS};
use crate::hash::{DIGEST, Digest, F};
use crate::{Proof, ZkError};

use mint::MintAir;
use note::{Note, value_digest};
use spend::{Mode, PUB_FEE, PUB_NF, PUB_OUT, PUB_ROOT, PUBLIC, SpendAir};
use tree::CommitmentTree;

/// What a spend proof claims, in public.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpendStatement {
    /// Transfer or unshield.
    pub mode: Mode,
    /// The tree root the input note was proved against.
    pub root: Digest,
    /// The input note's nullifier.
    pub nullifier: Digest,
    /// Transfer: the new note's commitment. Unshield: `value_digest(amount)`.
    pub out: Digest,
    /// Public fee.
    pub fee: u64,
}

impl SpendStatement {
    fn public(&self) -> Vec<F> {
        let mut p = vec![F::ZERO; PUBLIC];
        p[PUB_ROOT..PUB_ROOT + DIGEST].copy_from_slice(&self.root);
        p[PUB_NF..PUB_NF + DIGEST].copy_from_slice(&self.nullifier);
        p[PUB_OUT..PUB_OUT + DIGEST].copy_from_slice(&self.out);
        p[PUB_FEE] = F::from_u64(self.fee);
        p
    }

    /// For an unshield, the amount leaving the pool.
    #[must_use]
    pub fn amount(&self) -> Option<u64> {
        (self.mode == Mode::Unshield).then(|| {
            u64::from(self.out[0].as_canonical_u32())
                | (u64::from(self.out[1].as_canonical_u32()) << LIMB_BITS)
        })
    }
}

const MAX_VALUE: u64 = (1 << MAX_BITS) - 1;
const MAX_FEE: u64 = (1 << LIMB_BITS) - 1;

/// Proves a mint of `note`. Returns the proof and the public commitment.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] if the value is not below `2^60`.
pub fn prove_mint(note: &Note) -> Result<(Proof, Digest), ZkError> {
    if note.value > MAX_VALUE {
        return Err(ZkError::Unsatisfied("note value is not below 2^60"));
    }
    let proof = crate::prove(&MintAir::default(), mint::trace(note), &mint::public(note))?;
    Ok((proof, note.commitment()))
}

/// Verifies a mint of `value` into `commitment`.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] for a value above `2^60`, else as [`crate::verify`].
pub fn verify_mint(proof: &Proof, commitment: &Digest, value: u64) -> Result<(), ZkError> {
    if value > MAX_VALUE {
        return Err(ZkError::Unsatisfied("mint value is not below 2^60"));
    }
    let mut public = commitment.to_vec();
    public.extend_from_slice(&value_digest(value)[..2]);
    crate::verify(&MintAir::default(), proof, &public)
}

/// Proves spending `input` (at `path`, owned by `sk`) into `output`.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] if the witness is inconsistent: `sk` does not own
/// the input, values do not balance, a value or the fee is out of range.
pub fn prove_transfer(
    sk: &SecretDigest,
    input: &Note,
    path: &MerklePath,
    output: &Note,
    fee: u64,
) -> Result<(Proof, SpendStatement), ZkError> {
    witness::prove(
        Mode::Transfer,
        sk,
        input,
        path,
        Some(output),
        output.value,
        fee,
    )
}

/// Proves spending `input` into the public `amount`.
///
/// # Errors
///
/// As [`prove_transfer`].
pub fn prove_unshield(
    sk: &SecretDigest,
    input: &Note,
    path: &MerklePath,
    amount: u64,
    fee: u64,
) -> Result<(Proof, SpendStatement), ZkError> {
    witness::prove(Mode::Unshield, sk, input, path, None, amount, fee)
}

/// Verifies a spend proof against its statement.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] for an out-of-range fee or amount (checked
/// natively, since they are public), else as [`crate::verify`].
pub fn verify_spend(proof: &Proof, statement: &SpendStatement) -> Result<(), ZkError> {
    if statement.fee > MAX_FEE {
        return Err(ZkError::Unsatisfied("fee is not below 2^30"));
    }
    if statement.mode == Mode::Unshield && statement.out[2..].iter().any(|x| *x != F::ZERO) {
        return Err(ZkError::Unsatisfied("unshield amount is not two limbs"));
    }
    crate::verify(&SpendAir::new(statement.mode), proof, &statement.public())
}

/// Why the pool refused an operation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PoolError {
    /// The proof, or its statement's ranges, did not check.
    #[error(transparent)]
    Proof(#[from] ZkError),
    /// The root is not one this tree has had.
    #[error("unknown commitment-tree root")]
    UnknownRoot,
    /// The nullifier has been published before.
    #[error("nullifier already spent")]
    DoubleSpend,
    /// The tree has no room.
    #[error("commitment tree is full")]
    TreeFull,
    /// A transfer statement submitted as an unshield, or the reverse.
    #[error("statement mode does not match the operation")]
    WrongMode,
    /// More value would leave than is locked.
    #[error("pool balance would go negative")]
    Underflow,
}

/// The pool's consensus state.
#[derive(Debug, Default)]
pub struct ShieldedPool {
    tree: CommitmentTree,
    nullifiers: BTreeSet<[u32; DIGEST]>,
    locked: u64,
}

fn key(d: &Digest) -> [u32; DIGEST] {
    core::array::from_fn(|i| d[i].as_canonical_u32())
}

impl ShieldedPool {
    /// The commitment tree.
    #[must_use]
    pub fn tree(&self) -> &CommitmentTree {
        &self.tree
    }

    /// Transparent value held by the pool.
    #[must_use]
    pub fn locked(&self) -> u64 {
        self.locked
    }

    /// Accepts a mint; returns the note's leaf index.
    ///
    /// # Errors
    ///
    /// A bad proof, a full tree, or a balance overflow.
    pub fn mint(
        &mut self,
        proof: &Proof,
        commitment: &Digest,
        value: u64,
    ) -> Result<u64, PoolError> {
        verify_mint(proof, commitment, value)?;
        let locked = self.locked.checked_add(value).ok_or(PoolError::Underflow)?;
        let index = self.tree.append(*commitment).ok_or(PoolError::TreeFull)?;
        self.locked = locked;
        Ok(index)
    }

    fn check_spend(
        &self,
        proof: &Proof,
        statement: &SpendStatement,
        mode: Mode,
    ) -> Result<(), PoolError> {
        if statement.mode != mode {
            return Err(PoolError::WrongMode);
        }
        if !self.tree.is_known_root(&statement.root) {
            return Err(PoolError::UnknownRoot);
        }
        if self.nullifiers.contains(&key(&statement.nullifier)) {
            return Err(PoolError::DoubleSpend);
        }
        verify_spend(proof, statement)?;
        Ok(())
    }

    /// Accepts a transfer; returns the new note's leaf index. The fee leaves
    /// the pool.
    ///
    /// # Errors
    ///
    /// Wrong mode, unknown root, double spend, a bad proof, a full tree.
    pub fn transfer(
        &mut self,
        proof: &Proof,
        statement: &SpendStatement,
    ) -> Result<u64, PoolError> {
        self.check_spend(proof, statement, Mode::Transfer)?;
        let locked = self
            .locked
            .checked_sub(statement.fee)
            .ok_or(PoolError::Underflow)?;
        let index = self.tree.append(statement.out).ok_or(PoolError::TreeFull)?;
        self.nullifiers.insert(key(&statement.nullifier));
        self.locked = locked;
        Ok(index)
    }

    /// Accepts an unshield; returns the amount paid out.
    ///
    /// # Errors
    ///
    /// Wrong mode, unknown root, double spend, a bad proof, an underflow.
    pub fn unshield(
        &mut self,
        proof: &Proof,
        statement: &SpendStatement,
    ) -> Result<u64, PoolError> {
        self.check_spend(proof, statement, Mode::Unshield)?;
        let amount = statement.amount().ok_or(PoolError::WrongMode)?;
        let out = amount
            .checked_add(statement.fee)
            .ok_or(PoolError::Underflow)?;
        let locked = self.locked.checked_sub(out).ok_or(PoolError::Underflow)?;
        self.nullifiers.insert(key(&statement.nullifier));
        self.locked = locked;
        Ok(amount)
    }
}

#[cfg(test)]
mod tests;
