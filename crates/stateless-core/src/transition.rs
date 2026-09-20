//! A transfer executed against a verified witness instead of a database.
//!
//! The rules are the node's `StateDB::stage_transaction` for
//! `TxKind::Transfer`, in the same order and through the same `ledger-math`
//! calls: nonce first, then the output total, then the balance, then debit and
//! nonce advance, then each credit read back through the tree so a
//! self-transfer sees the debited balance. An absent account reads as zero and
//! is written, as the node writes it. `tests/stateless_equivalence.rs` in the
//! node pins the two against each other.
//!
//! Signatures are not checked here. The caller derives `sender` from keys it
//! has just verified — the node's rule that a sender is never a wire field.

use maya_ledger_math::{advance_nonce, credit, debit, total_outputs};

use crate::compress::{Key, Value};
use crate::error::{Result, Violation};
use crate::params::VALUE_BYTES;
use crate::partial::VerifiedTree;

/// An account as a leaf holds it: balance, then nonce, little-endian.
///
/// Byte-identical to the node's `Account::encode`, which a node test asserts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccountState {
    /// Spendable balance in base units.
    pub balance: u64,
    /// Transactions this account has sent.
    pub nonce: u64,
}

impl AccountState {
    /// Fixed-width leaf value.
    #[must_use]
    pub fn encode(&self) -> Value {
        let mut out = [0u8; VALUE_BYTES];
        out[..8].copy_from_slice(&self.balance.to_le_bytes());
        out[8..].copy_from_slice(&self.nonce.to_le_bytes());
        out
    }

    /// Decodes a leaf value. Every 16 bytes are some account.
    #[must_use]
    pub fn decode(value: &Value) -> Self {
        let (balance, nonce) = value.split_at(8);
        Self {
            balance: u64::from_le_bytes(balance.try_into().unwrap_or_default()),
            nonce: u64::from_le_bytes(nonce.try_into().unwrap_or_default()),
        }
    }
}

/// Applies one transfer.
///
/// `outputs` is walked twice — once to total, once to credit — so it must be
/// `Clone`; a mapped slice iterator is.
///
/// # Errors
///
/// [`crate::Error::Invalid`] for a wrong nonce, an insufficient balance, or an
/// overflow; [`crate::Error::Unverifiable`] if the witness does not open a key
/// the transfer reads. On error the tree may hold part of the transfer, and
/// the caller discards it with the block.
pub fn apply_transfer<D, I>(
    tree: &mut VerifiedTree<D>,
    sender: Key,
    nonce: u64,
    outputs: I,
) -> Result<()>
where
    D: Copy + Eq,
    I: IntoIterator<Item = (Key, u64)> + Clone,
{
    let mut account = load(tree, &sender)?;
    if nonce != account.nonce {
        return Err(Violation::Nonce {
            key: sender,
            expected: account.nonce,
            actual: nonce,
        }
        .into());
    }

    let overflow = |key| Violation::Overflow { key };
    let total = total_outputs(outputs.clone().into_iter().map(|(_, amount)| amount))
        .ok_or(overflow(sender))?;
    if account.balance < total {
        return Err(Violation::InsufficientBalance {
            key: sender,
            required: total,
            available: account.balance,
        }
        .into());
    }

    account.balance = debit(account.balance, total).ok_or(overflow(sender))?;
    account.nonce = advance_nonce(account.nonce).ok_or(overflow(sender))?;
    tree.set(sender, account.encode())?;

    for (recipient, amount) in outputs {
        let mut credited = load(tree, &recipient)?;
        credited.balance = credit(credited.balance, amount).ok_or(overflow(recipient))?;
        tree.set(recipient, credited.encode())?;
    }
    Ok(())
}

fn load<D: Copy + Eq>(tree: &VerifiedTree<D>, key: &Key) -> Result<AccountState> {
    Ok(tree
        .get(key)?
        .map(|value| AccountState::decode(&value))
        .unwrap_or_default())
}
