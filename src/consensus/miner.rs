//! Multi-threaded ArgonBlake proof-of-work search.
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
//! Each concurrent hash allocates a 32 MiB Argon2id buffer, so thread count is
//! a memory decision as much as a CPU one: 8 threads is a quarter-gigabyte of
//! working set. [`suggested_threads`] accounts for that.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::core::BlockHeader;
use crate::crypto::argon_blake::argon_blake_hash;
use crate::crypto::pow::meets_target;
use crate::error::Result;

/// How often a worker re-checks the cancellation flag.
///
/// Each attempt already costs tens of milliseconds, so checking every attempt
/// is free relative to the hashing itself.
const CANCEL_CHECK_INTERVAL: u64 = 1;

/// A solved header.
#[derive(Clone, Debug)]
pub struct MiningResult {
    /// The header carrying the winning nonce.
    pub header: BlockHeader,
    /// Its ArgonBlake digest.
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
        .map(|n| n.get().min(8))
        .unwrap_or(1)
        .max(1)
}

/// Searches for a nonce making `header` satisfy its own difficulty target.
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
    let threads = threads.max(1);
    let target = header.difficulty_target;

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
                    match argon_blake_hash(&candidate.serialize()) {
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
