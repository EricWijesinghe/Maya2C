//! Turning a decision into signed transactions without tripping over nonces.
//!
//! Three problems, one ledger of pending transactions:
//!
//! - **Rebroadcast the same bytes.** Hybrid signing is randomised, so re-signing
//!   a still-pending claim would make a second transaction with the same nonce.
//!   Pending transactions are rebroadcast as they were first signed.
//! - **Do not resubmit what is in flight.** [`policy::decide`] says "claim" on
//!   every tick until the claim is mined; a purpose that already has a pending
//!   transaction is not submitted again.
//! - **Consecutive nonces in one tick.** The chain reports only the committed
//!   nonce, so new transactions are numbered after the highest still pending,
//!   and every pending one is rebroadcast so a dropped transaction never leaves
//!   a gap that stalls the rest.
//!
//! [`policy::decide`]: crate::policy::decide

use std::collections::BTreeMap;

use custom_l1_node::core::{Transaction, TxKind};
use custom_l1_node::crypto::hybrid::HybridSigningKey;
use maya_htlc_lattice::CommitmentId;

use crate::chain::SwapChain;
use crate::error::{Result, WatcherError};
use crate::swap::ChainSide;

/// What a pending transaction is for: one per swap and kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Purpose {
    /// Funding a lock.
    Lock(CommitmentId),
    /// Claiming a swap's inbound lock.
    Claim(CommitmentId),
    /// Refunding a swap's outbound lock.
    Refund(CommitmentId),
}

/// `ChainSide` with an ordering, for map keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Side {
    Maya,
    Counterparty,
}

impl From<ChainSide> for Side {
    fn from(side: ChainSide) -> Self {
        match side {
            ChainSide::Maya => Self::Maya,
            ChainSide::Counterparty => Self::Counterparty,
        }
    }
}

/// Transactions broadcast and not yet mined.
#[derive(Debug, Default)]
pub struct Pending {
    raw: BTreeMap<(Side, u64), Vec<u8>>,
    purposes: BTreeMap<Purpose, (Side, u64)>,
    next: BTreeMap<Side, u64>,
}

impl Pending {
    /// Forgets what the chain has mined and rebroadcasts the rest.
    ///
    /// # Errors
    ///
    /// An RPC failure reading the nonce. A failed rebroadcast is not an error:
    /// the transaction may already be mined, and the next tick retries.
    pub async fn refresh(
        &mut self,
        side: ChainSide,
        chain: &dyn SwapChain,
        key: &HybridSigningKey,
    ) -> Result<()> {
        let side = Side::from(side);
        let committed = chain.next_nonce(&key.address()).await?;
        self.raw
            .retain(|(entry, nonce), _| *entry != side || *nonce >= committed);
        let raw = &self.raw;
        self.purposes
            .retain(|_, (entry, nonce)| *entry != side || raw.contains_key(&(side, *nonce)));

        let mut next = committed;
        for ((_, nonce), bytes) in self.raw.range((side, committed)..=(side, u64::MAX)) {
            // Best effort, as above.
            let _ = chain.broadcast(bytes).await;
            next = nonce.saturating_add(1);
        }
        self.next.insert(side, next);
        Ok(())
    }

    /// Whether a transaction for `purpose` is still waiting to be mined.
    #[must_use]
    pub fn in_flight(&self, purpose: Purpose) -> bool {
        self.purposes.contains_key(&purpose)
    }

    /// The nonce the next new transaction on `side` will carry.
    ///
    /// # Errors
    ///
    /// [`WatcherError::Refused`] if [`Pending::refresh`] has not run for that
    /// side, so no nonce is known.
    pub fn next_nonce(&self, side: ChainSide) -> Result<u64> {
        self.next
            .get(&Side::from(side))
            .copied()
            .ok_or_else(|| WatcherError::Refused("nonce not refreshed for that chain".to_owned()))
    }

    /// Signs `kind` at the next nonce, remembers it, and broadcasts it.
    ///
    /// # Errors
    ///
    /// [`WatcherError::Signing`], or the broadcast's failure. A failed
    /// broadcast stays pending, so the next tick retries the same bytes.
    pub async fn submit(
        &mut self,
        side: ChainSide,
        chain: &dyn SwapChain,
        key: &HybridSigningKey,
        purpose: Purpose,
        kind: TxKind,
    ) -> Result<()> {
        let nonce = self.next_nonce(side)?;
        let mut tx = Transaction::with_kind(kind, nonce);
        tx.sign(key)
            .map_err(|e| WatcherError::Signing(e.to_string()))?;
        let raw = tx.to_bytes();

        let side_key = Side::from(side);
        self.raw.insert((side_key, nonce), raw.clone());
        self.purposes.insert(purpose, (side_key, nonce));
        self.next.insert(side_key, nonce.saturating_add(1));
        chain.broadcast(&raw).await
    }
}
