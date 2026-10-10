//! Share verification.
//!
//! ## The pool uses the chain's own rule, never a copy of it
//!
//! Everything expensive here goes through `BlockHeader::pow_hash_at` and
//! `meets_target` from `custom_l1_node`. That is the point of this crate
//! depending on the node at all: a pool that reimplements the proof-of-work
//! rule is a pool that eventually credits work the chain would reject, and the
//! divergence shows up as blocks the pool submits and the network refuses —
//! after the shares behind them have been paid for.
//!
//! `pow_hash_at` also picks the rule by height, `ArgonBlake` below the DAG
//! activation and hashimoto above it (`src/core/block.rs:142`). Hashing at the
//! wrong height is the same class of error as hashing with the wrong function.
//!
//! ## Order of operations is a cost decision
//!
//! One verification is 25.4 ms of Argon2id below the fork. Everything that can
//! be decided from integers is decided first, in
//! [`crate::channel::ChannelState::precheck`], and only what survives reaches
//! this module. Within it, the single hash answers both questions — share
//! target and network target — because the digest is the same one.
//!
//! ## A block is a share that got lucky
//!
//! There is no separate "solution" submission. A share whose digest also meets
//! the network target *is* a block, and it is credited as a share as well as
//! submitted as a block. Crediting it only as a block would quietly steal the
//! share weight from the miner who found it.

use custom_l1_node::consensus::difficulty::work_from_target;
use custom_l1_node::core::BlockHeader;
use custom_l1_node::crypto::dag::registry::CacheRegistry;
use custom_l1_node::crypto::pow::meets_target;

use crate::error::Result;
use crate::job::Job;
use crate::model::{RejectReason, ShareOutcome};

/// Work a target represents, saturated into a `u64`.
///
/// Saturation is unreachable in practice and is still written down rather than
/// asserted: `PoolConfig` caps share targets at 63 leading zero bits precisely
/// so this stays exact, but a target arriving from somewhere other than the
/// vardiff controller must degrade rather than panic. Window totals are
/// accumulated in `U256` for the same reason from the other direction — see
/// [`crate::pplns`].
#[must_use]
pub fn weight_of(target: &[u8; 32]) -> u64 {
    let work = work_from_target(target);
    let bytes = work.to_be_bytes();
    let (high, low) = bytes.split_at(24);

    if high.iter().any(|byte| *byte != 0) {
        return u64::MAX;
    }

    let mut buf = [0u8; 8];
    buf.copy_from_slice(low);
    u64::from_be_bytes(buf)
}

/// Verifies one submission against a job.
///
/// `share_target` is the channel's own target, which is never harder than the
/// network's. `dag` supplies the epoch cache for heights at or above the DAG
/// activation; below it the argument is unused, which is why the pool can run
/// without ever building a dataset.
///
/// # Errors
///
/// Propagates a proof-of-work failure from the chain — a missing DAG cache, for
/// instance. A verification that *fails* is a rejected share and returns
/// `Ok(Rejected)`; an `Err` here means the pool could not decide, which is an
/// operational fault and never the miner's.
pub fn verify(
    job: &Job,
    nonce: u64,
    ntime: u64,
    share_target: &[u8; 32],
    dag: &CacheRegistry,
) -> Result<ShareOutcome> {
    if !job.accepts_ntime(ntime) {
        return Ok(ShareOutcome::Rejected(RejectReason::InvalidNtime));
    }

    let header = job.header_with(nonce, ntime);
    let digest = header.pow_hash_at(job.height, dag)?;

    if !meets_target(&digest, share_target) {
        return Ok(ShareOutcome::Rejected(RejectReason::LowDifficulty));
    }

    Ok(ShareOutcome::Accepted {
        weight: weight_of(share_target),
        // One digest, both questions. A share that clears the network target is
        // a block, and is still credited as the share it also is.
        is_block: meets_target(&digest, &job.network_target),
    })
}

/// The block a winning share solved, ready for `submit_block`.
///
/// Empty of transactions, matching the node's own miner and the template the
/// node handed out. See [`crate::job`] for why that is the design rather than a
/// shortcut.
#[must_use]
pub fn block_for(job: &Job, nonce: u64, ntime: u64) -> custom_l1_node::core::Block {
    custom_l1_node::core::Block::new(job.header_with(nonce, ntime), Vec::new())
}

/// The header a submission claims, for logging a rejected block.
#[must_use]
pub fn header_for(job: &Job, nonce: u64, ntime: u64) -> BlockHeader {
    job.header_with(nonce, ntime)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    use custom_l1_node::crypto::dag::registry::DagConfig;
    use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
    use custom_l1_node::rpc::{HeaderInfo, MiningCandidate};

    use crate::job::JobRegistry;

    /// Leading zero bits used throughout. Low enough that a solution is found
    /// in a handful of attempts even under a 25 ms hash.
    const EASY_BITS: u32 = 5;

    fn registry_with_job(height: u64, bits: u32) -> (JobRegistry, u32) {
        let header = BlockHeader {
            prev_hash: [3u8; 32],
            state_root: [4u8; 32],
            timestamp: 1_700_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(bits),
            tx_root: [0; 32],
        };
        let candidate = MiningCandidate {
            height,
            difficulty_target: hex::encode(header.difficulty_target),
            header_bytes: hex::encode(header.serialize()),
            header: HeaderInfo::from(&header),
        };

        let mut jobs = JobRegistry::new();
        let id = jobs.push(&candidate).unwrap().expect("new work");
        (jobs, id)
    }

    /// Grinds a nonce whose digest meets `bits`, against the job's real header.
    ///
    /// Deliberately not `mine_header_with`: that searches against the header's
    /// *own* `difficulty_target`, and the target is part of the hash preimage
    /// (`src/core/block.rs:46`). Editing it to make the search cheap would
    /// produce a nonce for a different header than the one a miner submits
    /// against — the test would pass and prove nothing.
    fn solve(job: &Job, bits: u32, dag: &CacheRegistry) -> u64 {
        let target = target_from_leading_zero_bits(bits);
        for nonce in 0..100_000u64 {
            let header = job.header_with(nonce, job.header.timestamp);
            let digest = header.pow_hash_at(job.height, dag).expect("hashing");
            if meets_target(&digest, &target) {
                return nonce;
            }
        }
        panic!("no solution for {bits} bits within the attempt budget");
    }

    #[test]
    fn weight_tracks_the_difficulty_of_the_target() {
        // Each extra bit doubles the expected hashes. This is the number a
        // miner is paid on, so it must not drift from the chain's own notion of
        // work.
        assert_eq!(weight_of(&target_from_leading_zero_bits(0)), 1);
        assert_eq!(weight_of(&target_from_leading_zero_bits(8)), 256);
        assert_eq!(weight_of(&target_from_leading_zero_bits(20)), 1 << 20);
    }

    #[test]
    fn weight_saturates_rather_than_wrapping_on_an_extreme_target() {
        // Unreachable through the vardiff bounds, and it must still not wrap:
        // a wrapped weight would credit an enormous share as a tiny one.
        assert_eq!(weight_of(&[0u8; 32]), u64::MAX);
    }

    #[test]
    fn a_digest_below_the_share_target_is_credited() {
        let (jobs, id) = registry_with_job(1, 32);
        let job = jobs.get(id).unwrap();
        let dag = CacheRegistry::new(DagConfig::NEVER);
        let nonce = solve(job, EASY_BITS, &dag);

        let outcome = verify(
            job,
            nonce,
            job.header.timestamp,
            &target_from_leading_zero_bits(EASY_BITS),
            &dag,
        )
        .unwrap();

        // Credited, and not a block: the job's own target is 32 bits, which a
        // 5-bit digest almost certainly does not reach.
        assert_eq!(
            outcome,
            ShareOutcome::Accepted {
                weight: 1 << EASY_BITS,
                is_block: false,
            }
        );
    }

    #[test]
    fn a_digest_above_the_share_target_is_refused() {
        let (jobs, id) = registry_with_job(1, 32);
        let job = jobs.get(id).unwrap();
        let dag = CacheRegistry::new(DagConfig::NEVER);

        // Nonce 0 against a 32-bit target: overwhelmingly not a solution.
        let outcome = verify(
            job,
            0,
            job.header.timestamp,
            &target_from_leading_zero_bits(32),
            &dag,
        )
        .unwrap();

        assert_eq!(outcome, ShareOutcome::Rejected(RejectReason::LowDifficulty));
    }

    #[test]
    fn a_share_that_also_clears_the_network_target_is_a_block_and_still_a_share() {
        // Crediting it only as a block would steal the share weight from the
        // miner who found it.
        let (jobs, id) = registry_with_job(1, EASY_BITS);
        let job = jobs.get(id).unwrap();
        let dag = CacheRegistry::new(DagConfig::NEVER);
        let nonce = solve(job, EASY_BITS, &dag);

        let outcome = verify(
            job,
            nonce,
            job.header.timestamp,
            &target_from_leading_zero_bits(EASY_BITS),
            &dag,
        )
        .unwrap();

        match outcome {
            ShareOutcome::Accepted { weight, is_block } => {
                assert!(is_block, "digest met the network target");
                assert_eq!(weight, 1 << EASY_BITS, "and was credited as a share");
            }
            other => panic!("expected an accepted share, got {other:?}"),
        }
    }

    #[test]
    fn an_out_of_range_ntime_is_refused_without_hashing() {
        let (jobs, id) = registry_with_job(1, 32);
        let job = jobs.get(id).unwrap();
        let dag = CacheRegistry::new(DagConfig::NEVER);

        let outcome = verify(
            job,
            0,
            job.header.timestamp - 1,
            &target_from_leading_zero_bits(32),
            &dag,
        )
        .unwrap();

        assert_eq!(outcome, ShareOutcome::Rejected(RejectReason::InvalidNtime));
    }

    #[test]
    fn the_block_the_pool_submits_carries_the_solving_header() {
        let (jobs, id) = registry_with_job(7, EASY_BITS);
        let job = jobs.get(id).unwrap();
        let dag = CacheRegistry::new(DagConfig::NEVER);
        let nonce = solve(job, EASY_BITS, &dag);

        let block = block_for(job, nonce, job.header.timestamp);
        assert_eq!(block.header.nonce, nonce);
        assert_eq!(block.header.prev_hash, job.header.prev_hash);
        assert!(block.transactions.is_empty());
    }
}
