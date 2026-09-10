//! State every part of the daemon shares.
//!
//! ## Lock discipline
//!
//! Three locks, and an order: `jobs` before `channels` before `telemetry`.
//! Nothing takes them in another order, and nothing holds one across an
//! `.await`. Both rules exist for the same reason — a connection task that
//! blocks while holding the channel registry stalls every other connection,
//! and at 50,000 of them that is the whole pool.
//!
//! The order is worth stating rather than assuming, because the natural way to
//! write one particular piece of code violates it: after a vardiff retune the
//! dashboard wants the channel's new bit count, and reaching for the channel
//! registry while already holding telemetry is the obvious way to get it.
//! [`crate::validator`] reads it out under the channel lock instead and
//! releases that lock before touching telemetry.
//!
//! `std::sync::Mutex` rather than Tokio's, deliberately. Every critical section
//! here is a map lookup and a few field updates; a Tokio mutex would add a
//! future and a scheduler hop to work that finishes in nanoseconds, and its one
//! real advantage — being held across an await — is a thing this module
//! forbids anyway.
//!
//! ## Events
//!
//! A broadcast channel, carrying what the dashboard needs to redraw. A slow
//! browser falls behind and its receiver reports `Lagged`; the handler skips
//! ahead rather than stalling, exactly as `explorer/src/server.rs` does. Pool
//! accounting must never block on a client — one wedged tab would otherwise
//! stop shares being credited.

use std::sync::{Arc, Mutex, RwLock};

use custom_l1_node::crypto::dag::registry::CacheRegistry;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::channel::ChannelRegistry;
use crate::config::PoolConfig;
use crate::error::{PoolError, Result};
use crate::job::JobRegistry;
use crate::ledger::ShareLedger;
use crate::metrics::PoolMetrics;
use crate::telemetry::TelemetryBook;

/// Events buffered for a subscriber before it is considered lagged.
///
/// Deep enough to absorb a burst of shares; shallow enough that a tab left open
/// on a laptop lid does not pin megabytes.
const EVENT_BUFFER: usize = 256;

/// Something the dashboard should redraw for.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PoolEvent {
    /// A share was credited.
    Share {
        /// Miner address, hex.
        miner: String,
        /// Rig name.
        worker: String,
        /// Work the share proved.
        weight: u64,
    },
    /// A share was refused.
    Rejected {
        /// Miner address, hex.
        miner: String,
        /// Rig name.
        worker: String,
        /// SV2 error code.
        reason: &'static str,
    },
    /// The pool submitted a block.
    Block {
        /// Block id, hex.
        id: String,
        /// Height it was submitted at.
        height: u64,
        /// Miner whose share solved it.
        finder: String,
    },
    /// A payout batch was broadcast.
    Payout {
        /// Batch id.
        batch: u64,
        /// Recipients.
        recipients: usize,
        /// Value moved, in base units.
        value: u64,
    },
    /// Work changed.
    Job {
        /// New job id.
        id: u32,
        /// Height the job builds on.
        height: u64,
    },
}

/// Everything the daemon's tasks share.
pub struct PoolState {
    /// Policy.
    pub config: PoolConfig,
    /// Work templates.
    pub jobs: RwLock<JobRegistry>,
    /// Open channels.
    pub channels: Mutex<ChannelRegistry>,
    /// Per-rig statistics.
    pub telemetry: Mutex<TelemetryBook>,
    /// Share credits and payouts.
    pub ledger: Arc<dyn ShareLedger>,
    /// Instrumentation.
    pub metrics: Arc<PoolMetrics>,
    /// Epoch caches for DAG-height verification.
    ///
    /// The *light* registry: a pool verifies, it does not search, and light
    /// verification needs the 64 MiB cache rather than the 4 GiB dataset
    /// (`src/crypto/dag/mod.rs`). That is what lets the pool run on an ordinary
    /// machine.
    pub dag: Arc<CacheRegistry>,
    /// Live events for the dashboard.
    pub events: broadcast::Sender<PoolEvent>,
}

impl PoolState {
    /// Assembles the shared state.
    #[must_use]
    pub fn new(
        config: PoolConfig,
        ledger: Arc<dyn ShareLedger>,
        metrics: Arc<PoolMetrics>,
        dag: Arc<CacheRegistry>,
    ) -> Self {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        Self {
            config,
            jobs: RwLock::new(JobRegistry::new()),
            channels: Mutex::new(ChannelRegistry::new()),
            telemetry: Mutex::new(TelemetryBook::new()),
            ledger,
            metrics,
            dag,
            events,
        }
    }

    /// Publishes an event, ignoring the absence of subscribers.
    ///
    /// A pool with nobody watching the dashboard is an ordinary pool, not an
    /// error, so a send failure here is dropped rather than propagated.
    pub fn publish(&self, event: PoolEvent) {
        let _ = self.events.send(event);
    }

    /// Locks the channel registry.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::Ledger`] if a previous holder panicked. A poisoned
    /// lock means some earlier critical section left the registry
    /// half-updated, and continuing on it would hand out a nonce range twice.
    pub fn channels(&self) -> Result<std::sync::MutexGuard<'_, ChannelRegistry>> {
        self.channels
            .lock()
            .map_err(|_| PoolError::Server("the channel registry lock is poisoned".to_string()))
    }

    /// Locks the telemetry book.
    ///
    /// # Errors
    ///
    /// As [`PoolState::channels`].
    pub fn telemetry(&self) -> Result<std::sync::MutexGuard<'_, TelemetryBook>> {
        self.telemetry
            .lock()
            .map_err(|_| PoolError::Server("the telemetry lock is poisoned".to_string()))
    }

    /// Reads the job registry.
    ///
    /// # Errors
    ///
    /// As [`PoolState::channels`].
    pub fn jobs(&self) -> Result<std::sync::RwLockReadGuard<'_, JobRegistry>> {
        self.jobs
            .read()
            .map_err(|_| PoolError::Server("the job registry lock is poisoned".to_string()))
    }

    /// Writes the job registry.
    ///
    /// # Errors
    ///
    /// As [`PoolState::channels`].
    pub fn jobs_mut(&self) -> Result<std::sync::RwLockWriteGuard<'_, JobRegistry>> {
        self.jobs
            .write()
            .map_err(|_| PoolError::Server("the job registry lock is poisoned".to_string()))
    }

    /// Refreshes the gauges that describe the pool's own size.
    ///
    /// Called on a timer rather than on every change: a gauge written on every
    /// share is a contended atomic on the hot path, and Prometheus reads it
    /// every fifteen seconds regardless.
    ///
    /// # Errors
    ///
    /// Propagates lock poisoning and ledger failures.
    pub fn sample_gauges(&self, now_millis: u64) -> Result<()> {
        let channels = self.channels()?.len();
        let (workers, hashrate, power) = {
            let telemetry = self.telemetry()?;
            (
                telemetry.len(),
                telemetry.total_hashrate(now_millis),
                telemetry.total_reported_power_milliwatts(),
            )
        };

        self.metrics.set_channels(channels);
        self.metrics.set_workers(workers);
        self.metrics.set_hashrate(hashrate);
        self.metrics.set_reported_power_milliwatts(power);

        let mut unpaid = 0u64;
        let mut immature = 0u64;
        for (_, balance) in self.ledger.balances()? {
            unpaid = unpaid.saturating_add(balance.unpaid);
            immature = immature.saturating_add(balance.immature);
        }
        self.metrics.set_balances(unpaid, immature);
        self.metrics
            .set_open_batches(self.ledger.open_batches()?.len());

        Ok(())
    }
}

impl core::fmt::Debug for PoolState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // No lock is taken: a `Debug` that can block is a `Debug` that can
        // deadlock the first time someone puts it in a log line.
        f.debug_struct("PoolState")
            .field("node_rpc", &self.config.node_rpc)
            .field("subscribers", &self.events.receiver_count())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use custom_l1_node::crypto::dag::registry::DagConfig;

    use crate::ledger::memory::MemoryLedger;

    fn state() -> PoolState {
        PoolState::new(
            PoolConfig {
                reward_per_block: 1_000,
                ..PoolConfig::default()
            },
            Arc::new(MemoryLedger::new()),
            Arc::new(PoolMetrics::new()),
            Arc::new(CacheRegistry::new(DagConfig::NEVER)),
        )
    }

    #[test]
    fn publishing_with_no_subscribers_is_not_an_error() {
        // A pool nobody is watching is an ordinary pool.
        let state = state();
        state.publish(PoolEvent::Job { id: 1, height: 2 });
    }

    #[tokio::test]
    async fn a_subscriber_receives_events() {
        let state = state();
        let mut receiver = state.events.subscribe();

        state.publish(PoolEvent::Job { id: 7, height: 9 });

        match receiver.recv().await.unwrap() {
            PoolEvent::Job { id, height } => {
                assert_eq!(id, 7);
                assert_eq!(height, 9);
            }
            other => panic!("unexpected event {other:?}"),
        }
    }

    #[test]
    fn sampling_gauges_reads_through_every_lock_without_deadlocking() {
        let state = state();
        state.sample_gauges(0).unwrap();

        let text = state.metrics.encode().unwrap();
        assert!(text.contains("maya_pool_channels_open 0"));
    }

    #[test]
    fn the_debug_rendering_takes_no_locks() {
        // Held here to prove it: a `Debug` that locks deadlocks the first time
        // it appears inside a critical section's own log line.
        let state = state();
        let _guard = state.channels().unwrap();
        let _ = format!("{state:?}");
    }
}
