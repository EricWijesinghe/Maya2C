//! Entries waiting for an earlier nonce (review of #653).
//!
//! Gossip does not keep a sender's order: nonce `n + 1` can reach a node
//! before `n`, and a mesh peer can hold `n` back on purpose. Refusing `n + 1`
//! there lost it for good, because gossip's seen-cache drops every later copy
//! and nothing re-announces it. So a transaction that passes every check but
//! the gap rule waits here: never in the pool proper, so never proposed, and
//! moved into the pool once the missing nonce is pooled or committed.
//!
//! Bounded three ways: one entry per sender and nonce, inside the sender's
//! [`super::MAX_PER_SENDER`] and [`super::MAX_NONCE_AHEAD`]; [`MAX_QUEUED`]
//! entries and [`MAX_QUEUED_BYTES`] per pool; and [`QUEUED_FOR_BLOCKS`]
//! blocks of waiting, so a head withheld for ever does not hold room for ever.

use primitive_types::U256;
use std::collections::BTreeMap;

use super::TxHash;
use crate::core::Transaction;
use crate::error::{NodeError, Result};
use crate::state::Address;

/// Most entries one pool holds waiting for a missing nonce: a quarter of the
/// pool's own default count, so a queue kept full costs a bounded fraction of
/// what the pool itself may hold.
pub const MAX_QUEUED: usize = super::DEFAULT_CAPACITY / 4;

/// Most encoded bytes waiting in one pool, a quarter of the pool's default.
pub const MAX_QUEUED_BYTES: usize = super::DEFAULT_MAX_BYTES / 4;

/// Blocks an entry waits for its missing nonce before it is dropped. A
/// reordering in gossip resolves within a round or two; this bounds how long
/// a sender who never sends the missing nonce holds queue room.
pub const QUEUED_FOR_BLOCKS: u64 = 32;

/// One waiting entry.
pub(super) struct Queued {
    pub(super) tx: Transaction,
    hash: TxHash,
    size: usize,
    outputs: U256,
    /// The admission height when it was queued.
    pub(super) at: u64,
}

impl Queued {
    pub(super) fn new(tx: Transaction, outputs: U256, at: u64) -> Self {
        Self {
            hash: tx.txid(),
            size: tx.to_bytes().len(),
            tx,
            outputs,
            at,
        }
    }
}

/// Every waiting entry of one pool, by sender then nonce. A `BTreeMap`, so
/// promotion visits senders and nonces in a fixed order.
#[derive(Default)]
pub(super) struct Queue {
    by_sender: BTreeMap<Address, BTreeMap<u64, Queued>>,
    len: usize,
    bytes: usize,
}

impl Queue {
    /// Whether `hash` already waits at `(sender, nonce)`. Another entry at
    /// that place is refused: the first arrival keeps it, as a pooled entry
    /// would not be displaced by a copy either.
    ///
    /// # Errors
    ///
    /// [`NodeError::MempoolRejected`] when a different entry waits there.
    pub(super) fn holds(&self, sender: &Address, nonce: u64, hash: &TxHash) -> Result<bool> {
        match self.by_sender.get(sender).and_then(|run| run.get(&nonce)) {
            None => Ok(false),
            Some(q) if q.hash == *hash => Ok(true),
            Some(_) => Err(NodeError::MempoolRejected(format!(
                "another transaction already waits at nonce {nonce}"
            ))),
        }
    }

    /// Entries and summed outputs waiting for `sender`, which count toward its
    /// per-sender share as pooled ones do.
    pub(super) fn share_of(&self, sender: &Address) -> (usize, U256) {
        self.by_sender.get(sender).map_or((0, U256::zero()), |run| {
            let outputs = run
                .values()
                .fold(U256::zero(), |sum, q| sum.saturating_add(q.outputs));
            (run.len(), outputs)
        })
    }

    /// Adds an entry.
    ///
    /// # Errors
    ///
    /// [`NodeError::MempoolRejected`] when the queue is full or its place is
    /// taken.
    pub(super) fn push(&mut self, sender: Address, entry: Queued) -> Result<()> {
        if self.holds(&sender, entry.tx.nonce, &entry.hash)? {
            return Ok(());
        }
        if self.len >= MAX_QUEUED || self.bytes.saturating_add(entry.size) > MAX_QUEUED_BYTES {
            return Err(NodeError::MempoolRejected(format!(
                "nonce {} leaves a gap and the queue for gapped entries is full",
                entry.tx.nonce
            )));
        }
        self.len += 1;
        self.bytes = self.bytes.saturating_add(entry.size);
        self.by_sender
            .entry(sender)
            .or_default()
            .insert(entry.tx.nonce, entry);
        Ok(())
    }

    /// `sender`'s lowest waiting nonce.
    pub(super) fn lowest(&self, sender: &Address) -> Option<u64> {
        self.by_sender.get(sender)?.keys().next().copied()
    }

    /// Takes `sender`'s lowest waiting nonce out.
    pub(super) fn pop_lowest(&mut self, sender: &Address) -> Option<Queued> {
        let run = self.by_sender.get_mut(sender)?;
        let (_, entry) = run.pop_first()?;
        if run.is_empty() {
            self.by_sender.remove(sender);
        }
        self.len -= 1;
        self.bytes = self.bytes.saturating_sub(entry.size);
        Some(entry)
    }

    /// Drops every entry queued more than [`QUEUED_FOR_BLOCKS`] before
    /// `height`, and returns the senders still waiting, in order.
    pub(super) fn expire(&mut self, height: u64) -> Vec<Address> {
        let (mut len, mut bytes) = (0, 0usize);
        self.by_sender.retain(|_, run| {
            run.retain(|_, q| q.at.saturating_add(QUEUED_FOR_BLOCKS) >= height);
            len += run.len();
            bytes = run.values().fold(bytes, |b, q| b.saturating_add(q.size));
            !run.is_empty()
        });
        (self.len, self.bytes) = (len, bytes);
        self.by_sender.keys().copied().collect()
    }

    /// Entries waiting.
    pub(super) fn len(&self) -> usize {
        self.len
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const ALICE: Address = [1; 32];

    fn entry(nonce: u64, at: u64) -> Queued {
        Queued::new(Transaction::new(vec![], vec![], nonce), U256::one(), at)
    }

    #[test]
    fn one_entry_per_sender_and_nonce_and_the_lowest_leaves_first() {
        let mut q = Queue::default();
        q.push(ALICE, entry(5, 0)).unwrap();
        q.push(ALICE, entry(3, 0)).unwrap();
        assert_eq!(q.share_of(&ALICE), (2, U256::from(2)));
        let mut other = entry(3, 0);
        other.hash = [9; 32];
        assert!(q.push(ALICE, other).is_err(), "nonce 3 is taken");
        assert_eq!(q.pop_lowest(&ALICE).unwrap().tx.nonce, 3);
        assert_eq!(q.pop_lowest(&ALICE).unwrap().tx.nonce, 5);
        assert!(q.pop_lowest(&ALICE).is_none());
        assert_eq!((q.len(), q.bytes), (0, 0));
    }

    #[test]
    fn a_full_queue_refuses_more() {
        let mut q = Queue::default();
        for nonce in 0..u64::try_from(MAX_QUEUED).unwrap() {
            let mut sender = [0u8; 32];
            sender[..8].copy_from_slice(&nonce.to_le_bytes());
            q.push(sender, entry(1, 0)).unwrap();
        }
        assert!(q.push(ALICE, entry(1, 0)).is_err());
    }

    #[test]
    fn entries_past_their_wait_are_dropped() {
        let mut q = Queue::default();
        q.push(ALICE, entry(1, 10)).unwrap();
        q.push(ALICE, entry(2, 20)).unwrap();
        assert_eq!(q.expire(10 + QUEUED_FOR_BLOCKS), vec![ALICE]);
        assert_eq!(q.len(), 2, "still inside the wait");
        assert_eq!(q.expire(11 + QUEUED_FOR_BLOCKS), vec![ALICE]);
        assert_eq!(q.share_of(&ALICE).0, 1);
        assert!(q.expire(21 + QUEUED_FOR_BLOCKS).is_empty());
        assert_eq!((q.len(), q.bytes), (0, 0));
    }
}
