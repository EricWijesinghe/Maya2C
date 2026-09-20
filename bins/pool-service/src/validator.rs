//! The share validation pipeline.
//!
//! ## Why this is a separate stage at all
//!
//! One share is 25.4 ms of Argon2id below the DAG fork
//! (`src/crypto/argon_blake.rs`). Running that on the task that owns a socket
//! would put it in front of every other message that connection has queued, and
//! at 50,000 connections would put it in front of the Tokio runtime itself.
//! Shares therefore cross a bounded channel into a pool of blocking workers,
//! and the connection task waits for a small reply rather than doing the work.
//!
//! ## Backpressure raises targets; it never drops shares
//!
//! A full queue makes [`Validator::submit`] wait, which propagates back to the
//! miner as a slower acknowledgement — honest, and self-limiting. Beyond that,
//! sustained depth triggers [`crate::vardiff::Vardiff::apply_pressure`] across
//! every channel, which reduces the *arrival rate* rather than the service
//! rate.
//!
//! Dropping submissions would be the easy shed and the wrong one: a dropped
//! share is work a miner already did and will not be paid for, and a pool that
//! does that under load is a pool miners leave under load.
//!
//! ## Concurrency is bounded by memory, not by cores
//!
//! Argon2id over 32 MiB means each concurrent verification holds 32 MiB. Eight
//! workers is a quarter-gigabyte of working set, which is the same arithmetic
//! `src/consensus/miner.rs` does for mining threads and the same cap it lands
//! on. `spawn_blocking`'s default pool of 512 threads would be sixteen
//! gigabytes.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, mpsc, oneshot};

use crate::error::{PoolError, Result};
use crate::model::{ShareOutcome, WorkerKey, now_millis};
use crate::session::ShareRequest;
use crate::share;
use crate::state::{PoolEvent, PoolState};

/// Submissions that may wait for a validator.
///
/// Sized so a burst is absorbed rather than a backlog accumulated: deeper than
/// this and the queue is storing shares whose jobs will be stale before they
/// are verified, which wastes the verification.
pub const QUEUE_CAPACITY: usize = 4_096;

/// Queue depth above which targets start rising.
///
/// A quarter full. Acting at "almost full" would act too late — the controller
/// needs several share intervals to take effect, and by then the queue is
/// holding stale work.
const PRESSURE_WATERMARK: usize = QUEUE_CAPACITY / 4;

/// Shortest gap between two rounds of pressure retuning.
///
/// One retune is a factor of two in arrival rate, and it takes a while to show
/// up in the queue. Retuning faster than this drives targets to the ceiling on
/// a transient.
const PRESSURE_INTERVAL: Duration = Duration::from_secs(30);

/// Concurrent verifications.
///
/// See the module documentation: this is a memory bound, not a CPU one.
pub const DEFAULT_WORKERS: usize = 8;

/// What the pool decided about one share, and what to tell the miner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareVerdict {
    /// Credited or refused.
    pub outcome: ShareOutcome,
    /// A new share target, when the vardiff controller moved during this share.
    ///
    /// Carried back with the verdict rather than pushed separately so the miner
    /// learns about a retune in the same round trip that acknowledged the share
    /// that caused it.
    pub retune: Option<[u8; 32]>,
}

/// A share whose digest also met the network target.
#[derive(Clone, Debug)]
pub struct FoundBlockShare {
    /// Job it solved.
    pub job_id: u32,
    /// Winning nonce.
    pub nonce: u64,
    /// Header timestamp used.
    pub ntime: u64,
    /// Miner to credit as the finder.
    pub key: WorkerKey,
    /// Ledger position of the solving share.
    ///
    /// The PPLNS window is measured backwards from here. Taking the ledger tip
    /// instead would sweep in shares that arrived while the block was being
    /// submitted, and pay them from this block as well as the next.
    pub sequence: u64,
}

/// One queued verification.
struct ValidationJob {
    /// What to verify.
    request: ShareRequest,
    /// Where the verdict goes.
    reply: oneshot::Sender<Result<ShareVerdict>>,
}

/// A handle on the validation pipeline.
#[derive(Debug, Clone)]
pub struct Validator {
    /// Queue into the worker pool.
    sender: mpsc::Sender<ValidationJob>,
    /// Submissions currently queued or in flight.
    depth: Arc<AtomicUsize>,
}

impl core::fmt::Debug for ValidationJob {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ValidationJob")
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

impl Validator {
    /// Starts the pipeline.
    ///
    /// `blocks` receives every share that also solved the chain's target; the
    /// caller submits those, because doing it here would put an RPC round trip
    /// inside a blocking worker.
    #[must_use]
    pub fn spawn(
        state: Arc<PoolState>,
        workers: usize,
        blocks: mpsc::Sender<FoundBlockShare>,
    ) -> Self {
        let (sender, mut receiver) = mpsc::channel::<ValidationJob>(QUEUE_CAPACITY);
        let depth = Arc::new(AtomicUsize::new(0));

        let permits = Arc::new(Semaphore::new(workers.max(1)));
        let dispatcher_depth = Arc::clone(&depth);

        tokio::spawn(async move {
            let mut last_pressure = Instant::now() - PRESSURE_INTERVAL;

            while let Some(job) = receiver.recv().await {
                if dispatcher_depth.load(Ordering::Relaxed) > PRESSURE_WATERMARK
                    && last_pressure.elapsed() >= PRESSURE_INTERVAL
                {
                    last_pressure = Instant::now();
                    apply_pressure(&state);
                }

                // `acquire_owned` rather than a fixed set of worker tasks: the
                // permit travels with the job into `spawn_blocking` and is
                // released when the verification finishes, so a slow share
                // holds one slot rather than blocking a whole worker's queue.
                let Ok(permit) = Arc::clone(&permits).acquire_owned().await else {
                    break;
                };

                let state = Arc::clone(&state);
                let blocks = blocks.clone();
                let depth = Arc::clone(&dispatcher_depth);

                tokio::task::spawn_blocking(move || {
                    let verdict = validate(&state, &job.request, &blocks);
                    depth.fetch_sub(1, Ordering::Relaxed);
                    state
                        .metrics
                        .set_validation_queue(depth.load(Ordering::Relaxed));
                    // The miner may already be gone; that is ordinary and the
                    // credit has been recorded regardless.
                    let _ = job.reply.send(verdict);
                    drop(permit);
                });
            }
        });

        Self { sender, depth }
    }

    /// Queues a share and waits for the verdict.
    ///
    /// Waits rather than failing when the queue is full. That wait *is* the
    /// backpressure: it slows the connection that is producing shares instead
    /// of discarding the work behind them.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::Server`] if the pipeline has shut down, and
    /// propagates a verification failure — a missing DAG cache, for instance —
    /// which is the pool's fault and never the miner's.
    pub async fn submit(&self, request: ShareRequest) -> Result<ShareVerdict> {
        let (reply, answer) = oneshot::channel();
        self.depth.fetch_add(1, Ordering::Relaxed);

        self.sender
            .send(ValidationJob { request, reply })
            .await
            .map_err(|_| {
                self.depth.fetch_sub(1, Ordering::Relaxed);
                PoolError::Server("the share validator has shut down".to_string())
            })?;

        answer
            .await
            .map_err(|_| PoolError::Server("a validation worker dropped a share".to_string()))?
    }

    /// Submissions queued or in flight.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.depth.load(Ordering::Relaxed)
    }
}

/// Verifies one share and records what it was worth.
///
/// Runs on a blocking thread. Every lock it takes is a `std::sync` lock held
/// for a map lookup, and none is held across the verification itself.
fn validate(
    state: &PoolState,
    request: &ShareRequest,
    blocks: &mpsc::Sender<FoundBlockShare>,
) -> Result<ShareVerdict> {
    let job = {
        let jobs = state.jobs()?;
        match jobs.get(request.job_id) {
            Some(job) => job.clone(),
            // The job was retired between the connection task's check and this
            // one. Stale, and the miner is told so rather than left waiting.
            None => return rejected(state, request, crate::model::RejectReason::Stale),
        }
    };

    let started = Instant::now();
    let outcome = share::verify(
        &job,
        request.nonce,
        request.ntime,
        &request.target,
        &state.dag,
    )?;
    state
        .metrics
        .observe_validation(started.elapsed().as_secs_f64());

    match outcome {
        ShareOutcome::Rejected(reason) => Ok(rejected(state, request, reason)?),
        ShareOutcome::Accepted { weight, is_block } => {
            let now = now_millis();

            // The ledger write comes first. Everything after it is statistics
            // and can be rebuilt; the credit cannot.
            let sequence =
                state
                    .ledger
                    .append_share(&request.key.miner, &request.key.worker, weight, now)?;

            // Both values are read out under the channel lock and the lock is
            // released before the telemetry lock is taken. `PoolState` documents
            // the order as jobs → channels → telemetry, and taking the channel
            // registry *while holding* telemetry — to read the new bit count for
            // the dashboard — would invert it against every other path in the
            // pool. That is a deadlock that appears only under concurrent load,
            // which is exactly when a pool cannot afford one.
            let (retune, bits) = {
                let mut channels = state.channels()?;
                match channels.get_mut(request.channel_id) {
                    Some(channel) => {
                        channel.accepted += 1;
                        channel.weight = channel.weight.saturating_add(weight);
                        let retune = channel
                            .vardiff
                            .on_accepted_share(now)
                            .map(|_| channel.target());
                        (retune, channel.vardiff.bits())
                    }
                    None => (None, 0),
                }
            };

            {
                let mut telemetry = state.telemetry()?;
                telemetry.record_accepted(&request.key, weight, now);
                if retune.is_some() {
                    telemetry.record_target(&request.key, bits, now);
                }
            }

            state.metrics.record_accepted();
            state.publish(PoolEvent::Share {
                miner: hex::encode(request.key.miner),
                worker: request.key.worker.clone(),
                weight,
            });

            if is_block {
                // Handed off rather than submitted here: an RPC round trip
                // inside a blocking worker would hold one of the eight
                // verification slots for the length of a network call.
                let found = FoundBlockShare {
                    job_id: request.job_id,
                    nonce: request.nonce,
                    ntime: request.ntime,
                    key: request.key.clone(),
                    sequence,
                };
                if blocks.try_send(found).is_err() {
                    // The submitter is wedged or gone. The share is credited
                    // either way; what is lost is the block, and that must be
                    // loud rather than silent.
                    eprintln!(
                        "pool: a solved block could not be handed to the submitter \
                         (channel {})",
                        request.channel_id
                    );
                }
            }

            Ok(ShareVerdict { outcome, retune })
        }
    }
}

/// Records a rejection decided by the validator rather than the session.
fn rejected(
    state: &PoolState,
    request: &ShareRequest,
    reason: crate::model::RejectReason,
) -> Result<ShareVerdict> {
    {
        let mut channels = state.channels()?;
        if let Some(channel) = channels.get_mut(request.channel_id) {
            channel.rejected += 1;
            if reason == crate::model::RejectReason::Stale {
                channel.stale += 1;
            }
        }
    }

    state
        .telemetry()?
        .record_rejected(&request.key, reason, now_millis());
    state.metrics.record_rejected(reason);
    state.publish(PoolEvent::Rejected {
        miner: hex::encode(request.key.miner),
        worker: request.key.worker.clone(),
        reason: reason.code(),
    });

    Ok(ShareVerdict {
        outcome: ShareOutcome::Rejected(reason),
        retune: None,
    })
}

/// Raises every channel's target one step.
///
/// The load shed. Miners see harder targets and submit less often; nobody loses
/// a share they already found.
fn apply_pressure(state: &PoolState) {
    let Ok(mut channels) = state.channels() else {
        return;
    };

    for channel in channels.iter_mut() {
        if channel.vardiff.apply_pressure().is_some() {
            state.metrics.record_pressure_retune();
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    use custom_l1_node::core::BlockHeader;
    use custom_l1_node::crypto::dag::registry::{CacheRegistry, DagConfig};
    use custom_l1_node::crypto::pow::{meets_target, target_from_leading_zero_bits};
    use custom_l1_node::rpc::{HeaderInfo, MiningCandidate};

    use crate::config::PoolConfig;
    use crate::ledger::memory::MemoryLedger;
    use crate::metrics::PoolMetrics;

    /// Easy enough that a solution turns up in tens of 25 ms hashes.
    const EASY_BITS: u32 = 5;

    fn key() -> WorkerKey {
        WorkerKey {
            miner: [0xAB; 32],
            worker: "rig-1".to_string(),
        }
    }

    fn state_with_job(network_bits: u32) -> Arc<PoolState> {
        let state = Arc::new(PoolState::new(
            PoolConfig {
                reward_per_block: 1_000,
                ..PoolConfig::default()
            },
            Arc::new(MemoryLedger::new()),
            Arc::new(PoolMetrics::new()),
            Arc::new(CacheRegistry::new(DagConfig::NEVER)),
        ));

        let header = BlockHeader {
            prev_hash: [5u8; 32],
            state_root: [6u8; 32],
            timestamp: 1_700_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(network_bits),
            tx_root: [0; 32],
        };
        state
            .jobs_mut()
            .unwrap()
            .push(&MiningCandidate {
                height: 1,
                difficulty_target: hex::encode(header.difficulty_target),
                header_bytes: hex::encode(header.serialize()),
                header: HeaderInfo::from(&header),
            })
            .unwrap();

        state
    }

    /// Opens a channel directly, bypassing the protocol.
    fn open_channel(state: &PoolState) -> u32 {
        let mut channels = state.channels().unwrap();
        let id = channels.open(key(), 1.0, &state.config, 0).unwrap().id;
        let job_id = state.jobs().unwrap().current().unwrap().id;
        channels.get_mut(id).unwrap().push_job(job_id);
        id
    }

    /// Grinds a nonce meeting `bits` against the job's real header.
    fn solve(state: &PoolState, bits: u32) -> u64 {
        let jobs = state.jobs().unwrap();
        let job = jobs.current().unwrap();
        let target = target_from_leading_zero_bits(bits);

        for nonce in 0..100_000u64 {
            let header = job.header_with(nonce, job.header.timestamp);
            let digest = header.pow_hash_at(job.height, &state.dag).unwrap();
            if meets_target(&digest, &target) {
                return nonce;
            }
        }
        panic!("no solution within the attempt budget");
    }

    fn request(state: &PoolState, channel_id: u32, nonce: u64, bits: u32) -> ShareRequest {
        let job_id = state.jobs().unwrap().current().unwrap().id;
        let ntime = state.jobs().unwrap().current().unwrap().header.timestamp;
        ShareRequest {
            channel_id,
            key: key(),
            sequence_number: 1,
            job_id,
            nonce,
            ntime,
            target: target_from_leading_zero_bits(bits),
        }
    }

    #[tokio::test]
    async fn an_accepted_share_reaches_the_ledger_before_anything_else() {
        let state = state_with_job(32);
        let channel_id = open_channel(&state);
        let nonce = solve(&state, EASY_BITS);

        let (blocks, _rx) = mpsc::channel(4);
        let validator = Validator::spawn(Arc::clone(&state), 2, blocks);

        let verdict = validator
            .submit(request(&state, channel_id, nonce, EASY_BITS))
            .await
            .unwrap();

        assert!(matches!(verdict.outcome, ShareOutcome::Accepted { .. }));
        assert_eq!(state.ledger.tip_sequence().unwrap(), Some(0));
        assert_eq!(
            state
                .ledger
                .window(0, custom_l1_node::consensus::U256::MAX)
                .unwrap()
                .shares
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn a_share_below_the_target_is_refused_and_counted() {
        let state = state_with_job(32);
        let channel_id = open_channel(&state);

        let (blocks, _rx) = mpsc::channel(4);
        let validator = Validator::spawn(Arc::clone(&state), 2, blocks);

        // Nonce 0 against a 24-bit share target: overwhelmingly not a solution.
        let verdict = validator
            .submit(request(&state, channel_id, 0, 24))
            .await
            .unwrap();

        assert!(matches!(verdict.outcome, ShareOutcome::Rejected(_)));
        assert_eq!(state.ledger.tip_sequence().unwrap(), None);
        assert_eq!(
            state.channels().unwrap().get(channel_id).unwrap().rejected,
            1
        );
    }

    #[tokio::test]
    async fn a_share_that_solves_the_chain_is_handed_to_the_submitter_and_still_credited() {
        let state = state_with_job(EASY_BITS);
        let channel_id = open_channel(&state);
        let nonce = solve(&state, EASY_BITS);

        let (blocks, mut found) = mpsc::channel(4);
        let validator = Validator::spawn(Arc::clone(&state), 2, blocks);

        let verdict = validator
            .submit(request(&state, channel_id, nonce, EASY_BITS))
            .await
            .unwrap();

        assert!(matches!(
            verdict.outcome,
            ShareOutcome::Accepted { is_block: true, .. }
        ));

        let block = found.recv().await.expect("the block was handed off");
        assert_eq!(block.nonce, nonce);
        // Its ledger position, not the tip: the window is measured from here.
        assert_eq!(block.sequence, 0);
        assert_eq!(state.ledger.tip_sequence().unwrap(), Some(0));
    }

    #[tokio::test]
    async fn a_share_for_a_retired_job_is_stale_rather_than_verified() {
        let state = state_with_job(32);
        let channel_id = open_channel(&state);

        let (blocks, _rx) = mpsc::channel(4);
        let validator = Validator::spawn(Arc::clone(&state), 2, blocks);

        let mut stale = request(&state, channel_id, 0, EASY_BITS);
        stale.job_id = 9_999;

        let verdict = validator.submit(stale).await.unwrap();
        assert_eq!(
            verdict.outcome,
            ShareOutcome::Rejected(crate::model::RejectReason::Stale)
        );
        assert_eq!(state.channels().unwrap().get(channel_id).unwrap().stale, 1);
    }

    #[tokio::test]
    async fn the_queue_drains_rather_than_dropping_a_burst() {
        // The property that matters under load: every submitted share gets a
        // verdict. A pool that sheds by dropping sheds work miners already did.
        let state = state_with_job(32);
        let channel_id = open_channel(&state);

        let (blocks, _rx) = mpsc::channel(64);
        let validator = Validator::spawn(Arc::clone(&state), 4, blocks);

        let mut pending = Vec::new();
        for nonce in 0..32u64 {
            let validator = validator.clone();
            let request = request(&state, channel_id, nonce, 24);
            pending.push(tokio::spawn(async move { validator.submit(request).await }));
        }

        for handle in pending {
            handle.await.unwrap().expect("every share gets a verdict");
        }
        assert_eq!(validator.depth(), 0);
    }

    #[test]
    fn pressure_raises_every_channel_and_is_counted() {
        let state = state_with_job(32);
        open_channel(&state);
        open_channel(&state);

        let before: Vec<u32> = state
            .channels()
            .unwrap()
            .iter()
            .map(|channel| channel.vardiff.bits())
            .collect();

        apply_pressure(&state);

        let after: Vec<u32> = state
            .channels()
            .unwrap()
            .iter()
            .map(|channel| channel.vardiff.bits())
            .collect();

        assert_eq!(before.len(), 2);
        for (before, after) in before.iter().zip(after.iter()) {
            assert_eq!(*after, before + 1);
        }
        assert!(
            state
                .metrics
                .encode()
                .unwrap()
                .contains("maya_pool_vardiff_pressure_retunes_total 2")
        );
    }
}
