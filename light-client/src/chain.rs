//! Following headers, and believing proofs against them.
//!
//! ## Fork choice is on work, never on length
//!
//! A thousand headers mined at trivial difficulty are cheaper than ten mined at
//! real difficulty, so a client that preferred the longer chain would prefer the
//! cheaper lie. [`HeaderChain`] accumulates `work_from_target` — the node's own
//! function, not a reimplementation — and the tip is whichever branch has the
//! most of it.
//!
//! Ties keep the incumbent, matching `consensus::chain`: switching on equal work
//! would let anyone with a header at the current height flip the tip for free.
//!
//! ## Why the difficulty floor is checked here
//!
//! Without it, none of the above matters. An attacker mines at
//! `target = 0xFF..FF`, where every digest qualifies, and produces an arbitrarily
//! long chain in an instant. The floor is what makes each header cost something,
//! and it is the single check that separates SPV from credulity.
//!
//! It is deliberately *not* a full difficulty-retarget validation. Recomputing
//! the retarget would need every header in the window, which a client syncing
//! from a checkpoint may not have. The floor is the weaker, cheaper property,
//! and the gap between them is real: a chain could sit at the floor forever and
//! this client would follow it. Anyone who needs more should run a full node.

use std::collections::BTreeMap;

use custom_l1_node::consensus::difficulty::work_from_target;
use custom_l1_node::consensus::uint::U256;
use custom_l1_node::core::BlockHeader;
use custom_l1_node::crypto::dag::registry::CacheRegistry;
use custom_l1_node::crypto::pow::meets_target;
use custom_l1_node::state::AccountProof;

use crate::error::{LightClientError, Result};

/// One header the client has accepted, and what it cost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderRecord {
    /// The header itself.
    pub header: BlockHeader,
    /// Its height.
    pub height: u64,
    /// Cumulative work of this header and every ancestor.
    pub total_work: U256,
}

/// A chain of headers, with the most-work branch as its tip.
///
/// Holds every header it has been given, not only the best chain: a branch that
/// loses today can win tomorrow, and a client that discarded it would have to
/// re-download it to follow the reorg.
pub struct HeaderChain {
    records: BTreeMap<[u8; 32], HeaderRecord>,
    /// Best chain by height, for proof lookup.
    best: BTreeMap<u64, [u8; 32]>,
    tip: Option<[u8; 32]>,
    /// Easiest target any header on this network may carry.
    pow_limit: [u8; 32],
    /// Whether to verify each header's digest.
    ///
    /// Off only for tests that build synthetic chains, and named so that a
    /// reader can see immediately that a client with it off is not checking
    /// anything. The node's own `ChainConfig` carries the same switch for the
    /// same reason.
    verify_pow: bool,
    dag: CacheRegistry,
}

impl HeaderChain {
    /// Starts a chain from a trusted genesis or checkpoint header.
    ///
    /// The starting header is **not** verified. It is the client's root of
    /// trust, and where it comes from — a binary, an operator, a friend — is the
    /// one thing SPV cannot bootstrap for itself.
    #[must_use]
    pub fn new(genesis: BlockHeader, height: u64, pow_limit: [u8; 32], dag: CacheRegistry) -> Self {
        let id = genesis.id();
        let total_work = work_from_target(&genesis.difficulty_target);

        let mut records = BTreeMap::new();
        records.insert(
            id,
            HeaderRecord {
                header: genesis,
                height,
                total_work,
            },
        );
        let mut best = BTreeMap::new();
        best.insert(height, id);

        Self {
            records,
            best,
            tip: Some(id),
            pow_limit,
            verify_pow: true,
            dag,
        }
    }

    /// The same chain with proof-of-work verification disabled.
    ///
    /// For tests that build synthetic chains, where mining a real header would
    /// cost more than the test is worth. A client constructed this way follows
    /// whichever headers it is handed and checks nothing about their cost.
    #[must_use]
    pub fn without_pow_verification(mut self) -> Self {
        self.verify_pow = false;
        self
    }

    /// The current tip.
    #[must_use]
    pub fn tip(&self) -> Option<&HeaderRecord> {
        self.tip.and_then(|id| self.records.get(&id))
    }

    /// Cumulative work of the best chain.
    #[must_use]
    pub fn total_work(&self) -> U256 {
        self.tip().map_or(U256::ZERO, |record| record.total_work)
    }

    /// The header the best chain holds at `height`.
    #[must_use]
    pub fn header_at(&self, height: u64) -> Option<&HeaderRecord> {
        self.best.get(&height).and_then(|id| self.records.get(id))
    }

    /// Headers accepted so far, across every branch.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the chain holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Accepts a header that extends a known one.
    ///
    /// Returns whether it became the new tip. A header that is valid but on a
    /// losing branch is *stored and returns `false`* — it is not an error, and
    /// discarding it would mean re-downloading the branch if it later wins.
    ///
    /// # Errors
    ///
    /// - [`LightClientError::UnknownParent`] if the parent is not held.
    /// - [`LightClientError::TargetBelowFloor`] for a target easier than the
    ///   network floor — the check that makes a fake chain expensive.
    /// - [`LightClientError::InsufficientWork`] if the digest does not satisfy
    ///   the header's own target.
    pub fn accept(&mut self, header: BlockHeader) -> Result<bool> {
        let id = header.id();
        let hex_id = hex::encode(id);

        let Some(parent) = self.records.get(&header.prev_hash) else {
            return Err(LightClientError::UnknownParent {
                header: hex_id,
                parent: hex::encode(header.prev_hash),
            });
        };
        let height = parent.height.saturating_add(1);
        let parent_work = parent.total_work;

        // The floor first: it is a comparison, where the digest check may be a
        // DAG lookup. A header that cannot possibly be legal should not cost a
        // cache read to reject.
        if header.difficulty_target > self.pow_limit {
            return Err(LightClientError::TargetBelowFloor { header: hex_id });
        }

        if self.verify_pow {
            let digest = header
                .pow_hash_at(height, &self.dag)
                .map_err(|error| LightClientError::Node(error.to_string()))?;
            if !meets_target(&digest, &header.difficulty_target) {
                return Err(LightClientError::InsufficientWork(hex_id));
            }
        }

        let total_work = parent_work.saturating_add(work_from_target(&header.difficulty_target));
        self.records.insert(
            id,
            HeaderRecord {
                header,
                height,
                total_work,
            },
        );

        // Work, not height. Ties keep the incumbent: switching on equal work
        // would let anyone holding a header at the current height flip the tip
        // for nothing.
        if total_work > self.total_work() {
            self.tip = Some(id);
            self.rebuild_best();
            return Ok(true);
        }
        Ok(false)
    }

    /// Rewrites the height index by walking back from the tip.
    ///
    /// Rebuilt rather than patched, because a reorg can replace an arbitrary
    /// suffix and an index updated incrementally would keep entries from the
    /// branch that lost — which is exactly the state in which a proof verifies
    /// against a root the client no longer believes.
    fn rebuild_best(&mut self) {
        self.best.clear();
        let mut cursor = self.tip;
        while let Some(id) = cursor {
            let Some(record) = self.records.get(&id) else {
                break;
            };
            self.best.insert(record.height, id);
            if record.header.prev_hash == [0u8; 32]
                || !self.records.contains_key(&record.header.prev_hash)
            {
                break;
            }
            cursor = Some(record.header.prev_hash);
        }
    }
}

/// A light client: a header chain plus the ability to believe proofs against it.
pub struct LightClient {
    chain: HeaderChain,
}

impl LightClient {
    /// Wraps a header chain.
    #[must_use]
    pub const fn new(chain: HeaderChain) -> Self {
        Self { chain }
    }

    /// The underlying header chain.
    #[must_use]
    pub const fn chain(&self) -> &HeaderChain {
        &self.chain
    }

    /// The underlying header chain, mutably.
    pub const fn chain_mut(&mut self) -> &mut HeaderChain {
        &mut self.chain
    }

    /// Unwraps the client, returning the chain it was following.
    ///
    /// For a caller that wants to keep syncing headers and rebuild the client
    /// afterwards, without the chain being borrowed the whole time.
    #[must_use]
    pub fn into_chain(self) -> HeaderChain {
        self.chain
    }

    /// Checks an account proof against the state root committed at `height`.
    ///
    /// The height is a parameter rather than something the proof carries, and
    /// that is the design: a proof verified against a root it supplied itself
    /// would be a proof of nothing. The client looks up the header *it* accepted
    /// and uses that root.
    ///
    /// # Errors
    ///
    /// - [`LightClientError::UnknownHeight`] if no header is held at `height`.
    /// - [`LightClientError::ProofMismatch`] if the proof does not reproduce
    ///   that header's `state_root`.
    /// - [`LightClientError::Node`] if the proof is structurally malformed —
    ///   distinct from not matching, because a proof that cannot be evaluated
    ///   and one that evaluates to the wrong root are different failures.
    pub fn verify_account(&self, height: u64, proof: &AccountProof) -> Result<()> {
        let Some(record) = self.chain.header_at(height) else {
            return Err(LightClientError::UnknownHeight(height));
        };

        if !proof.verify(&record.header.state_root)? {
            return Err(LightClientError::ProofMismatch {
                address: hex::encode(proof.address),
                height,
            });
        }
        Ok(())
    }

    /// The balance a verified proof establishes.
    ///
    /// Returns the balance only when the proof checks out, so there is no shape
    /// of this API in which a caller reads a number it has not verified.
    ///
    /// # Errors
    ///
    /// As [`LightClient::verify_account`].
    pub fn verified_balance(&self, height: u64, proof: &AccountProof) -> Result<u64> {
        self.verify_account(height, proof)?;
        Ok(proof.account.balance)
    }
}

/// Prints the chain's shape without its proof-of-work caches.
///
/// Hand-written because [`CacheRegistry`] holds tens of megabytes per epoch and
/// does not implement `Debug`. A derived one would either fail to compile or,
/// worse, print a dataset into a log line.
impl core::fmt::Debug for HeaderChain {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HeaderChain")
            .field("headers", &self.records.len())
            .field("height", &self.tip().map(|record| record.height))
            .field("tip", &self.tip.map(hex::encode))
            .field("verify_pow", &self.verify_pow)
            .finish_non_exhaustive()
    }
}

impl core::fmt::Debug for LightClient {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LightClient")
            .field("chain", &self.chain)
            .finish()
    }
}
