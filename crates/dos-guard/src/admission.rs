//! Mempool admission policy.
//!
//! - At most [`Policy::per_sender`] pending transactions per sender.
//! - A transaction with the same `(sender, nonce)` as one pending replaces it
//!   only if its fee is at least [`Policy::bump_percent`] higher — otherwise
//!   re-broadcasting at +1 unit would be free spam.
//! - When full, the transaction with the lowest fee **per byte** is evicted,
//!   and a newcomer is admitted only if it pays more per byte than that one.
//!   Pricing by byte is what makes a 13 KB post-quantum transaction pay for
//!   its size, and an oversized one pay proportionally more.

use std::collections::BTreeMap;

/// Admission parameters.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// Pool capacity, transactions.
    pub capacity: usize,
    /// Pending transactions allowed per sender.
    pub per_sender: usize,
    /// Minimum fee increase for a replacement, percent.
    pub bump_percent: u64,
}

/// A pending transaction as the policy sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pending {
    /// Sender.
    pub sender: [u8; 32],
    /// Nonce.
    pub nonce: u64,
    /// Total fee offered.
    pub fee: u64,
    /// Serialized size.
    pub size: u64,
}

/// Why a transaction was not admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The sender already has `per_sender` pending.
    SenderFull,
    /// A replacement that does not raise the fee by `bump_percent`.
    UnderpricedReplacement,
    /// The pool is full and this pays no more per byte than the cheapest.
    PoolFull,
}

/// What admission did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admitted {
    /// Added.
    Added,
    /// Replaced the pending transaction with the same nonce.
    Replaced,
    /// Added after evicting the cheapest-per-byte transaction.
    Evicted(Pending),
}

/// The policy's view of the pool.
pub struct Pool {
    policy: Policy,
    txs: BTreeMap<([u8; 32], u64), Pending>,
}

/// Fee per byte, compared exactly as a cross-multiplication.
fn cheaper(a: &Pending, b: &Pending) -> bool {
    u128::from(a.fee) * u128::from(b.size.max(1)) < u128::from(b.fee) * u128::from(a.size.max(1))
}

impl Pool {
    /// An empty pool.
    #[must_use]
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            txs: BTreeMap::new(),
        }
    }

    /// Transactions pending.
    #[must_use]
    pub fn len(&self) -> usize {
        self.txs.len()
    }

    /// Whether the pool is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }

    /// Applies the policy to `tx`.
    ///
    /// # Errors
    ///
    /// The [`Refusal`] that applies.
    pub fn admit(&mut self, tx: Pending) -> Result<Admitted, Refusal> {
        if let Some(old) = self.txs.get(&(tx.sender, tx.nonce)) {
            let needed = u128::from(old.fee) * u128::from(100 + self.policy.bump_percent);
            if u128::from(tx.fee) * 100 < needed {
                return Err(Refusal::UnderpricedReplacement);
            }
            self.txs.insert((tx.sender, tx.nonce), tx);
            return Ok(Admitted::Replaced);
        }
        let from_sender = self
            .txs
            .range((tx.sender, 0)..=(tx.sender, u64::MAX))
            .count();
        if from_sender >= self.policy.per_sender {
            return Err(Refusal::SenderFull);
        }
        if self.txs.len() < self.policy.capacity {
            self.txs.insert((tx.sender, tx.nonce), tx);
            return Ok(Admitted::Added);
        }
        let cheapest = *self
            .txs
            .values()
            .fold(None::<&Pending>, |m, p| match m {
                Some(c) if !cheaper(p, c) => Some(c),
                _ => Some(p),
            })
            .ok_or(Refusal::PoolFull)?;
        if !cheaper(&cheapest, &tx) {
            return Err(Refusal::PoolFull);
        }
        self.txs.remove(&(cheapest.sender, cheapest.nonce));
        self.txs.insert((tx.sender, tx.nonce), tx);
        Ok(Admitted::Evicted(cheapest))
    }
}
