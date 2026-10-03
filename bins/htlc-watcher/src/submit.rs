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

use custom_l1_node::core::{ChainTag, Transaction, TxKind, TxOutput};
use custom_l1_node::crypto::hybrid::HybridSigningKey;
use maya_htlc_lattice::CommitmentId;

use crate::chain::{Fees, SwapChain};
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
    /// The fee allowance paid to a swap's counterparty with this watcher's
    /// lock, so a counterparty new to the chain can pay for its claim.
    Allowance(CommitmentId),
}

/// Signs a transaction of `kind` with `outputs`, adding the fee output a
/// fee-charging chain requires: twice the base fee on the signed size, as
/// `l1-wallet` computes it. The fee output's amount is fixed-width, so the
/// probe's size is the final size.
///
/// # Errors
///
/// [`WatcherError::Signing`].
pub fn sign_with_fee(
    kind: &TxKind,
    mut outputs: Vec<TxOutput>,
    nonce: u64,
    key: &HybridSigningKey,
    chain_tag: &ChainTag,
    fees: Option<Fees>,
) -> Result<Transaction> {
    let sign = |outputs: Vec<TxOutput>| {
        let mut tx = Transaction::with_kind(kind.clone(), nonce);
        tx.outputs = outputs;
        tx.sign(key, chain_tag)
            .map_err(|e| WatcherError::Signing(e.to_string()))?;
        Ok::<_, WatcherError>(tx)
    };
    let Some(fees) = fees else {
        return sign(outputs);
    };
    outputs.push(TxOutput {
        amount: 0,
        recipient: fees.collector,
    });
    let size = u64::try_from(sign(outputs.clone())?.to_bytes().len()).unwrap_or(u64::MAX);
    if let Some(fee) = outputs.last_mut() {
        fee.amount = fees.base_fee.saturating_mul(size).saturating_mul(2);
    }
    sign(outputs)
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
        self.submit_with_outputs(side, chain, key, purpose, kind, Vec::new())
            .await
    }

    /// [`Pending::submit`] with transfer outputs, e.g. a fee allowance.
    ///
    /// # Errors
    ///
    /// As [`Pending::submit`], and a failure reading the chain's fee terms.
    pub async fn submit_with_outputs(
        &mut self,
        side: ChainSide,
        chain: &dyn SwapChain,
        key: &HybridSigningKey,
        purpose: Purpose,
        kind: TxKind,
        outputs: Vec<TxOutput>,
    ) -> Result<()> {
        let nonce = self.next_nonce(side)?;
        let chain_tag = chain.chain_tag().await?;
        let tx = sign_with_fee(&kind, outputs, nonce, key, &chain_tag, chain.fees().await?)?;
        let raw = tx.to_bytes();

        let side_key = Side::from(side);
        self.raw.insert((side_key, nonce), raw.clone());
        self.purposes.insert(purpose, (side_key, nonce));
        self.next.insert(side_key, nonce.saturating_add(1));
        chain.broadcast(&raw).await
    }
}
