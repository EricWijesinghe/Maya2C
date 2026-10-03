//! Signatures verified once (Master Prompt 12, bottleneck #1).
//!
//! Hybrid verification was 93 % of block application (`reports/12-performance.md`),
//! and on a DAG-BFT node every transaction was verified at least three times:
//! mempool admission, the builder's `select_applicable`, then `apply_block`.
//! Verification is a pure function of the transaction's bytes, so a result can
//! be remembered.
//!
//! # What the key covers, and why not `txid`
//!
//! The key is BLAKE3 over the chain tag (ADR-036) and the transaction's
//! complete wire encoding — signing bytes, keys and every signature. For a v5/v6 frame `txid` would do
//! (it hashes both signatures), but a v8 multisig id deliberately excludes
//! approvals, and a cache keyed on it would let a frame with forged approvals
//! ride on a genuine one's result. The wire hash has no such exception.
//!
//! # What is cached
//!
//! Hybrid (v5/v6) frames, whose verification ignores height and policy, and
//! suite-tagged (v7) frames, whose only height- and policy-dependent step —
//! whether the suite is admissible — runs on every call, before the cache is
//! consulted; what is cached for them is the signature check alone. Multisig
//! (v8) frames are verified every time: their threshold rules read policy in
//! more than one place. Only *successes* are stored.
//!
//! Bounded FIFO: at capacity the oldest entry goes. An evicted entry costs one
//! re-verification, never a wrong answer.

use std::collections::{BTreeSet, VecDeque};
use std::sync::Mutex;

use maya_crypto_pq::agility::SuitePolicy;

use crate::core::{ChainTag, Transaction};
use crate::error::Result;

/// Entries kept. Two full blocks' worth of a large mempool with room over.
pub const CAPACITY: usize = 131_072;

/// A bounded set of wire hashes that verified.
#[derive(Debug, Default)]
pub struct VerifiedCache {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    set: BTreeSet<[u8; 32]>,
    order: VecDeque<[u8; 32]>,
}

impl VerifiedCache {
    /// Verifies `tx` at `height`, or answers from the cache.
    ///
    /// The cache key includes the chain tag, so a transaction verified on one
    /// chain will not hit for another chain (ADR-036).
    ///
    /// # Errors
    ///
    /// Exactly what `Transaction::verify_at` returns; the cache never turns a
    /// failure into a success.
    pub fn verify(
        &self,
        tx: &Transaction,
        height: u64,
        policy: &SuitePolicy,
        chain: &ChainTag,
    ) -> Result<()> {
        if tx.multisig.is_some() {
            return tx.verify_at(height, policy, chain);
        }
        if let Some(auth) = &tx.suite_auth {
            crate::crypto::suites::check_admissible(policy, auth.suite, height)?;
        }
        // Cache key includes chain tag to prevent cross-chain cache hits (ADR-036).
        let mut hasher = blake3::Hasher::new_derive_key("maya.verified_cache.v1");
        hasher.update(&chain.0);
        hasher.update(&tx.to_bytes());
        let key = *hasher.finalize().as_bytes();
        if self.contains(&key) {
            return Ok(());
        }
        tx.verify_at(height, policy, chain)?;
        self.insert(key);
        Ok(())
    }

    /// Entries held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().set.len()
    }

    /// Whether nothing is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A poisoned lock holds a set of hashes; every entry in it is still a
        // hash that verified, so recovering it cannot produce a wrong answer.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn contains(&self, key: &[u8; 32]) -> bool {
        self.lock().set.contains(key)
    }

    fn insert(&self, key: [u8; 32]) {
        let mut inner = self.lock();
        if !inner.set.insert(key) {
            return;
        }
        inner.order.push_back(key);
        while inner.order.len() > CAPACITY {
            if let Some(old) = inner.order.pop_front() {
                inner.set.remove(&old);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::core::transaction::TxOutput;
    use crate::crypto::hybrid::signing_key_from_seed;

    fn signed(amount: u64) -> Transaction {
        let mut tx = Transaction::new(
            vec![],
            vec![TxOutput {
                amount,
                recipient: [9; 32],
            }],
            0,
        );
        tx.sign(
            &signing_key_from_seed(&[4; 32]).unwrap(),
            &crate::core::ChainTag::from_genesis([42; 32]),
        )
        .unwrap();
        tx
    }

    #[test]
    fn a_verified_transaction_is_remembered_and_a_tampered_one_is_not_let_through() {
        let cache = VerifiedCache::default();
        let policy = crate::crypto::suites::verification_policy();
        let chain = ChainTag::from_genesis([42; 32]);
        let tx = signed(5);
        cache.verify(&tx, 1, &policy, &chain).unwrap();
        assert_eq!(cache.len(), 1);
        cache.verify(&tx, 1, &policy, &chain).unwrap();
        assert_eq!(cache.len(), 1, "a hit adds nothing");
        // Same signature, different amount: a different key, and it fails.
        let mut forged = tx.clone();
        forged.outputs[0].amount = 6;
        assert!(cache.verify(&forged, 1, &policy, &chain).is_err());
        assert_eq!(cache.len(), 1, "failures are never stored");
    }
}
