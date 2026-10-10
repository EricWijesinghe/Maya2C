//! Multi-threaded proof-of-work search, under whichever rule the target height
//! calls for.
//!
//! Two rules exist and both are permanent: `ArgonBlake` below
//! [`DAG_ACTIVATION_HEIGHT`] and the memory-hard DAG at or above it. The caller
//! picks with [`PowMode`], because the caller is the one that knows what height
//! the candidate is for.
//!
//! [`DAG_ACTIVATION_HEIGHT`]: crate::crypto::dag::DAG_ACTIVATION_HEIGHT
//!
//! ## Nonce partitioning
//!
//! Threads stride through the nonce space rather than taking contiguous blocks:
//! thread `i` of `n` tries `i, i+n, i+2n, …`. Contiguous ranges would make the
//! first thread do all the work whenever a solution is found early, since low
//! nonces are tried first.
//!
//! ## Memory
//!
//! Under [`PowMode::Argon`] each concurrent hash allocates a 32 MiB Argon2id
//! buffer, so thread count is a memory decision as much as a CPU one: 8 threads
//! is a quarter-gigabyte of working set. [`suggested_threads`] accounts for
//! that.
//!
//! The DAG modes allocate nothing per thread. Every worker reads the one shared
//! cache or dataset, so the footprint is 64 MiB or 4 GiB total rather than per
//! worker — which is the whole point of a shared dataset, and why a GPU can run
//! thousands of lanes against it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::core::BlockHeader;
use crate::crypto::argon_blake::argon_blake_hash;
use crate::crypto::dag::cache::Cache;
use crate::crypto::dag::dataset::Dataset;
use crate::crypto::dag::hashimoto::{hashimoto_full, hashimoto_light};
use crate::crypto::pow::meets_target;
use crate::error::Result;

/// How often a worker re-checks the cancellation flag.
///
/// Each attempt already costs tens of milliseconds, so checking every attempt
/// is free relative to the hashing itself.
const CANCEL_CHECK_INTERVAL: u64 = 1;

/// Which proof-of-work rule a search runs under.
///
/// The height a candidate will land at decides this, and the caller knows that
/// height — so it is passed in rather than guessed at here. Mining under the
/// wrong variant produces solutions the network rejects, which is exactly the
/// failure the chain's own dispatch in `BlockHeader::pow_hash_at` prevents on
/// the validating side.
pub enum PowMode<'a> {
    /// `ArgonBlake`, the pre-fork rule.
    Argon,
    /// The DAG, recomputing each page from the epoch's 64 MiB cache.
    ///
    /// About a hundred times slower per hash than [`PowMode::DagFull`], and the
    /// only variant a machine without gigabytes to spare can run. Useful for a
    /// small chain or a test; not competitive on a live one, by design.
    DagLight(&'a Cache),
    /// The DAG, reading a materialised dataset. What a real miner runs.
    DagFull(&'a Dataset),
}

impl PowMode<'_> {
    /// Hashes `header` under this rule.
    ///
    /// `seed` is [`BlockHeader::pow_seed`], hoisted by the caller: it does not
    /// depend on the nonce, so recomputing it per attempt would be pure waste
    /// in the one loop where waste is the only thing that matters.
    fn hash(&self, header: &BlockHeader, seed: &[u8; 32]) -> Result<[u8; 32]> {
        match self {
            Self::Argon => argon_blake_hash(&header.serialize()),
            Self::DagLight(cache) => Ok(hashimoto_light(cache, seed, header.nonce).result),
            Self::DagFull(dataset) => Ok(hashimoto_full(dataset, seed, header.nonce).result),
        }
    }
}

/// A solved header.
#[derive(Clone, Debug)]
pub struct MiningResult {
    /// The header carrying the winning nonce.
    pub header: BlockHeader,
    /// Its `ArgonBlake` digest.
    pub hash: [u8; 32],
    /// Total attempts across all threads.
    pub attempts: u64,
}

/// Thread count that keeps the Argon2 working set reasonable.
///
/// Caps at 8 regardless of core count: beyond that the 32 MiB-per-thread
/// footprint starts to cost more in memory pressure than it returns in
/// parallelism.
#[must_use]
pub fn suggested_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get().min(8))
        .max(1)
}

/// Searches for a nonce making `header` satisfy its own difficulty target,
/// under the pre-fork `ArgonBlake` rule.
///
/// Kept as the plain entry point for tooling and tests that predate the fork.
/// Anything mining against a real chain must use [`mine_header_with`] and pass
/// the rule that applies at the height it is mining for.
///
/// Returns `Ok(None)` if `max_attempts` is exhausted or `cancel` is set before
/// a solution is found.
///
/// # Errors
///
/// Propagates hashing failures from [`argon_blake_hash`].
pub fn mine_header(
    header: &BlockHeader,
    threads: usize,
    cancel: &AtomicBool,
    max_attempts: Option<u64>,
) -> Result<Option<MiningResult>> {
    mine_header_with(header, threads, cancel, max_attempts, &PowMode::Argon)
}

/// Searches for a nonce making `header` satisfy its own difficulty target,
/// under `mode`.
///
/// Returns `Ok(None)` if `max_attempts` is exhausted or `cancel` is set before
/// a solution is found.
///
/// # Errors
///
/// Propagates hashing failures from the underlying rule.
pub fn mine_header_with(
    header: &BlockHeader,
    threads: usize,
    cancel: &AtomicBool,
    max_attempts: Option<u64>,
    mode: &PowMode<'_>,
) -> Result<Option<MiningResult>> {
    let threads = threads.max(1);
    let target = header.difficulty_target;
    // Constant across the whole search: the nonce is zeroed out of it.
    let seed = header.pow_seed();

    let attempts = AtomicU64::new(0);
    let found: Mutex<Option<MiningResult>> = Mutex::new(None);
    let failure: Mutex<Option<crate::error::NodeError>> = Mutex::new(None);
    // Local stop flag: set when any thread wins, so the others exit promptly
    // without disturbing the caller's `cancel`.
    let stop = AtomicBool::new(false);

    std::thread::scope(|scope| {
        for index in 0..threads {
            let attempts = &attempts;
            let found = &found;
            let failure = &failure;
            let stop = &stop;
            let header = header.clone();

            scope.spawn(move || {
                let mut candidate = header;
                let mut nonce = index as u64;
                let mut since_check = 0u64;

                loop {
                    if stop.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
                        return;
                    }

                    let total = attempts.fetch_add(1, Ordering::Relaxed) + 1;
                    if let Some(limit) = max_attempts
                        && total > limit
                    {
                        return;
                    }

                    candidate.nonce = nonce;
                    match mode.hash(&candidate, &seed) {
                        Ok(hash) => {
                            if meets_target(&hash, &target) {
                                // First writer wins; later solvers in the same
                                // round must not overwrite the recorded result.
                                if let Ok(mut slot) = found.lock()
                                    && slot.is_none()
                                {
                                    *slot = Some(MiningResult {
                                        header: candidate.clone(),
                                        hash,
                                        attempts: total,
                                    });
                                }
                                stop.store(true, Ordering::Relaxed);
                                return;
                            }
                        }
                        Err(error) => {
                            if let Ok(mut slot) = failure.lock()
                                && slot.is_none()
                            {
                                *slot = Some(error);
                            }
                            stop.store(true, Ordering::Relaxed);
                            return;
                        }
                    }

                    // Stride by thread count so no worker repeats another's
                    // nonce, and low nonces are spread across all threads.
                    nonce = match nonce.checked_add(threads as u64) {
                        Some(next) => next,
                        // Nonce space exhausted for this worker.
                        None => return,
                    };

                    since_check += 1;
                    if since_check >= CANCEL_CHECK_INTERVAL {
                        since_check = 0;
                    }
                }
            });
        }
    });

    if let Some(error) = failure.lock().ok().and_then(|mut slot| slot.take()) {
        return Err(error);
    }

    Ok(found.lock().ok().and_then(|mut slot| slot.take()))
}
