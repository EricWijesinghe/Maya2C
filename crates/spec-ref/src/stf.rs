//! The transfer state transition — `spec/03-state.md` STF-1 .. STF-8.

use std::collections::BTreeMap;

use crate::Address;

/// An account record. An address with no record reads as zero balance, zero
/// nonce (STF-7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Account {
    /// Balance in base units.
    pub balance: u64,
    /// Transactions sent.
    pub nonce: u64,
}

/// A transfer as the state transition sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    /// The address the signing keys hash to (TX-2).
    pub sender: Address,
    /// Whether both signatures verify (TX-1). An input, not computed here.
    pub signature_valid: bool,
    /// Replay counter.
    pub nonce: u64,
    /// `(recipient, amount)` in order.
    pub outputs: Vec<(Address, u64)>,
}

/// Why a block is invalid. The names are the spec's error kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// TX-1.
    BadSignature,
    /// STF-1.
    InvalidNonce,
    /// STF-2, STF-4, STF-5.
    BalanceOverflow,
    /// STF-3.
    InsufficientBalance,
}

impl Rejection {
    /// The spec's name for it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::BadSignature => "BadSignature",
            Self::InvalidNonce => "InvalidNonce",
            Self::BalanceOverflow => "BalanceOverflow",
            Self::InsufficientBalance => "InsufficientBalance",
        }
    }
}

/// Applies one transfer to `state`, in the order the spec fixes.
///
/// # Errors
///
/// The first rule the transfer breaks.
pub fn apply_transfer(
    state: &mut BTreeMap<Address, Account>,
    tx: &Transfer,
) -> Result<(), Rejection> {
    if !tx.signature_valid {
        return Err(Rejection::BadSignature); // TX-1, before any balance rule
    }
    let mut sender = state.get(&tx.sender).copied().unwrap_or_default();
    if tx.nonce != sender.nonce {
        return Err(Rejection::InvalidNonce); // STF-1
    }
    let mut total: u64 = 0;
    for (_, amount) in &tx.outputs {
        total = total
            .checked_add(*amount)
            .ok_or(Rejection::BalanceOverflow)?; // STF-2
    }
    if sender.balance < total {
        return Err(Rejection::InsufficientBalance); // STF-3
    }
    sender.balance -= total;
    sender.nonce = sender
        .nonce
        .checked_add(1)
        .ok_or(Rejection::BalanceOverflow)?; // STF-4
    state.insert(tx.sender, sender); // STF-6: debit lands before any credit
    for (recipient, amount) in &tx.outputs {
        let mut r = state.get(recipient).copied().unwrap_or_default();
        r.balance = r
            .balance
            .checked_add(*amount)
            .ok_or(Rejection::BalanceOverflow)?; // STF-5
        state.insert(*recipient, r);
    }
    Ok(())
}

/// STF-8: a block applies every transfer in order, or none of them.
///
/// # Errors
///
/// The index and rule of the first transfer that fails; `state` is then
/// returned unchanged because the caller's copy was never touched.
pub fn apply_block(
    state: &BTreeMap<Address, Account>,
    block: &[Transfer],
) -> Result<BTreeMap<Address, Account>, (usize, Rejection)> {
    let mut next = state.clone();
    for (index, tx) in block.iter().enumerate() {
        apply_transfer(&mut next, tx).map_err(|r| (index, r))?;
    }
    Ok(next)
}
