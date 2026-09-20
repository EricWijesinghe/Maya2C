//! Prometheus instrumentation for the pool.
//!
//! Built on the node's registry conventions (`src/metrics/mod.rs`) and served
//! by the node's exporter through
//! [`custom_l1_node::metrics::server::Exposition`] — one accept loop, one
//! routing table, one set of decisions about what an exporter should refuse to
//! serve.
//!
//! ## Labels are a closed set, and here that matters more than usual
//!
//! `src/metrics/mod.rs` refuses open-ended label values because a hostile peer
//! could otherwise create unbounded time series and exhaust the scraper. The
//! pool has the same exposure through a wider door: a rejection reason derived
//! from error text, or a per-worker label taken from the `user_identity` a
//! stranger chose, would let one connection mint series at will. Rejection
//! reasons are therefore a fixed `&'static str` set, and **nothing here is
//! labelled per worker.** Per-rig figures belong to the dashboard and the JSON
//! API, which page and can be authorised; a metric labelled by rig name is a
//! cardinality bomb with 50,000 fuses.
//!
//! ## Measured and reported stay separate here too
//!
//! `maya_pool_hashrate_hashes_per_second` is derived from accepted share weight.
//! `maya_pool_reported_power_watts` is a sum of numbers rigs sent about
//! themselves. An operator alerting on efficiency needs to know which is which,
//! and the metric names are where that gets said.

use std::sync::atomic::{AtomicI64, AtomicU64};

use prometheus_client::encoding::text::encode;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;

use crate::model::RejectReason;

/// Bucket bounds for share validation, in seconds.
///
/// Centred on the 25.4 ms an ArgonBlake verification costs
/// (`src/crypto/argon_blake.rs`), with room below for the DAG rule — about a
/// millisecond — and room above to show a saturated validator pool.
fn validation_buckets() -> impl Iterator<Item = f64> {
    [0.0005, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 1.0, 5.0].into_iter()
}

/// Bucket bounds for job push, in seconds.
///
/// Sub-second across the whole useful range, because that is the requirement:
/// at a 15-second block target a job that takes a second to reach a rig has
/// already spent 7% of its life in the pool.
fn job_push_buckets() -> impl Iterator<Item = f64> {
    [0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5].into_iter()
}

/// Labels distinguishing why a share was refused.
#[derive(Clone, Debug, Hash, PartialEq, Eq, prometheus_client::encoding::EncodeLabelSet)]
pub struct RejectionLabels {
    /// Short, bounded reason string, from [`RejectReason::code`].
    pub reason: &'static str,
}

/// Every metric the pool exports.
#[derive(Debug)]
pub struct PoolMetrics {
    /// Registry handed to the exporter.
    registry: Registry,

    /// Open mining channels.
    channels: Gauge<i64, AtomicI64>,
    /// Rigs the pool is retaining statistics for.
    workers: Gauge<i64, AtomicI64>,
    /// Pool hash rate measured from accepted share weight.
    hashrate: Gauge<i64, AtomicI64>,
    /// Sum of self-reported rig power, in watts. Unverifiable; see the module
    /// documentation.
    reported_power: Gauge<i64, AtomicI64>,
    /// Submissions waiting for a validator.
    ///
    /// The saturation signal. `docs/stratum-v2.md` §4 explains why this and not
    /// the connection count is what decides whether the pool keeps up.
    validation_queue: Gauge<i64, AtomicI64>,
    /// Credits owed to miners from confirmed blocks.
    unpaid: Gauge<i64, AtomicI64>,
    /// Credits from blocks that could still be orphaned.
    immature: Gauge<i64, AtomicI64>,
    /// Treasury balance.
    ///
    /// The one gauge an operator should page on. This chain mints no block
    /// reward, so payouts come from a funded account and an empty account
    /// means every payout stops.
    treasury_balance: Gauge<i64, AtomicI64>,
    /// Payout batches not yet in a terminal state.
    open_batches: Gauge<i64, AtomicI64>,

    /// Shares credited.
    shares_accepted: Counter<u64, AtomicU64>,
    /// Shares refused, by reason.
    shares_rejected: Family<RejectionLabels, Counter<u64, AtomicU64>>,
    /// Blocks the pool found and submitted.
    blocks_found: Counter<u64, AtomicU64>,
    /// Found blocks later lost to a reorg.
    blocks_orphaned: Counter<u64, AtomicU64>,
    /// Payout batches broadcast.
    payouts_broadcast: Counter<u64, AtomicU64>,
    /// Payout batches confirmed to depth.
    payouts_confirmed: Counter<u64, AtomicU64>,
    /// Payout batches the treasury refused.
    payouts_refused: Counter<u64, AtomicU64>,
    /// Value paid out, in base units.
    payout_value: Counter<u64, AtomicU64>,
    /// Vardiff retunes caused by validator backpressure.
    ///
    /// Distinct from ordinary retunes on purpose: this one rising means the
    /// pool is shedding load, which is a capacity signal and not a miner one.
    pressure_retunes: Counter<u64, AtomicU64>,

    /// Wall time spent verifying one share.
    validation_duration: Histogram,
    /// Wall time from a new template to the last channel being told about it.
    job_push_duration: Histogram,
}

impl Default for PoolMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl PoolMetrics {
    /// Builds the registry and registers every collector.
    #[must_use]
    pub fn new() -> Self {
        let mut registry = Registry::with_prefix("maya_pool");

        let channels = Gauge::default();
        let workers = Gauge::default();
        let hashrate = Gauge::default();
        let reported_power = Gauge::default();
        let validation_queue = Gauge::default();
        let unpaid = Gauge::default();
        let immature = Gauge::default();
        let treasury_balance = Gauge::default();
        let open_batches = Gauge::default();

        let shares_accepted = Counter::default();
        let shares_rejected = Family::<RejectionLabels, Counter<u64, AtomicU64>>::default();
        let blocks_found = Counter::default();
        let blocks_orphaned = Counter::default();
        let payouts_broadcast = Counter::default();
        let payouts_confirmed = Counter::default();
        let payouts_refused = Counter::default();
        let payout_value = Counter::default();
        let pressure_retunes = Counter::default();

        let validation_duration = Histogram::new(validation_buckets());
        let job_push_duration = Histogram::new(job_push_buckets());

        registry.register("channels_open", "Open mining channels", channels.clone());
        registry.register(
            "workers_tracked",
            "Rigs the pool holds statistics for",
            workers.clone(),
        );
        registry.register(
            "hashrate_hashes_per_second",
            "Pool hash rate measured from accepted share weight",
            hashrate.clone(),
        );
        registry.register(
            "reported_power_watts",
            "Sum of rig-reported power draw. Self-reported and unverifiable",
            reported_power.clone(),
        );
        registry.register(
            "validation_queue_depth",
            "Share submissions waiting for a validator",
            validation_queue.clone(),
        );
        registry.register(
            "unpaid_balance",
            "Credits owed to miners from confirmed blocks",
            unpaid.clone(),
        );
        registry.register(
            "immature_balance",
            "Credits from blocks that could still be orphaned",
            immature.clone(),
        );
        registry.register(
            "treasury_balance",
            "Balance of the account payouts are spent from",
            treasury_balance.clone(),
        );
        registry.register(
            "payout_batches_open",
            "Payout batches not yet confirmed or failed",
            open_batches.clone(),
        );

        registry.register(
            "shares_accepted",
            "Shares credited to a miner",
            shares_accepted.clone(),
        );
        registry.register(
            "shares_rejected",
            "Shares refused, by reason",
            shares_rejected.clone(),
        );
        registry.register(
            "blocks_found",
            "Blocks the pool submitted to the chain",
            blocks_found.clone(),
        );
        registry.register(
            "blocks_orphaned",
            "Found blocks later replaced by a reorg",
            blocks_orphaned.clone(),
        );
        registry.register(
            "payouts_broadcast",
            "Payout batches broadcast",
            payouts_broadcast.clone(),
        );
        registry.register(
            "payouts_confirmed",
            "Payout batches confirmed to the configured depth",
            payouts_confirmed.clone(),
        );
        registry.register(
            "payouts_refused",
            "Payout batches the treasury declined to sign",
            payouts_refused.clone(),
        );
        registry.register(
            "payout_value",
            "Value moved by broadcast payouts, in base units",
            payout_value.clone(),
        );
        registry.register(
            "vardiff_pressure_retunes",
            "Targets raised because the validator pool was saturated",
            pressure_retunes.clone(),
        );

        registry.register(
            "share_validation_duration_seconds",
            "Wall time verifying one share",
            validation_duration.clone(),
        );
        registry.register(
            "job_push_duration_seconds",
            "Wall time from a new template to the last channel being notified",
            job_push_duration.clone(),
        );

        Self {
            registry,
            channels,
            workers,
            hashrate,
            reported_power,
            validation_queue,
            unpaid,
            immature,
            treasury_balance,
            open_batches,
            shares_accepted,
            shares_rejected,
            blocks_found,
            blocks_orphaned,
            payouts_broadcast,
            payouts_confirmed,
            payouts_refused,
            payout_value,
            pressure_retunes,
            validation_duration,
            job_push_duration,
        }
    }

    /// Renders the registry in Prometheus text exposition format.
    ///
    /// # Errors
    ///
    /// Returns a formatting error only if encoding fails, which for an
    /// in-memory string is not reachable in practice.
    pub fn encode(&self) -> Result<String, std::fmt::Error> {
        let mut buffer = String::new();
        encode(&mut buffer, &self.registry)?;
        Ok(buffer)
    }

    /// Sets the open channel count.
    pub fn set_channels(&self, count: usize) {
        self.channels.set(clamp(count as u64));
    }

    /// Sets the tracked worker count.
    pub fn set_workers(&self, count: usize) {
        self.workers.set(clamp(count as u64));
    }

    /// Sets the measured pool hash rate.
    pub fn set_hashrate(&self, hashes_per_second: f64) {
        // Rounded into an integer gauge: `prometheus-client` has no float gauge
        // with an atomic backing, and a hash rate expressed to a fraction of a
        // hash is precision nobody has.
        self.hashrate.set(clamp(hashes_per_second.max(0.0) as u64));
    }

    /// Sets the sum of rig-reported power, taking milliwatts.
    pub fn set_reported_power_milliwatts(&self, milliwatts: u64) {
        self.reported_power.set(clamp(milliwatts / 1_000));
    }

    /// Sets the validator queue depth.
    pub fn set_validation_queue(&self, depth: usize) {
        self.validation_queue.set(clamp(depth as u64));
    }

    /// Sets the aggregate unpaid and immature balances.
    pub fn set_balances(&self, unpaid: u64, immature: u64) {
        self.unpaid.set(clamp(unpaid));
        self.immature.set(clamp(immature));
    }

    /// Sets the treasury balance.
    pub fn set_treasury_balance(&self, balance: u64) {
        self.treasury_balance.set(clamp(balance));
    }

    /// Sets the open payout batch count.
    pub fn set_open_batches(&self, count: usize) {
        self.open_batches.set(clamp(count as u64));
    }

    /// Counts a credited share.
    pub fn record_accepted(&self) {
        self.shares_accepted.inc();
    }

    /// Counts a refused share.
    pub fn record_rejected(&self, reason: RejectReason) {
        self.shares_rejected
            .get_or_create(&RejectionLabels {
                reason: reason.code(),
            })
            .inc();
    }

    /// Counts a submitted block.
    pub fn record_block_found(&self) {
        self.blocks_found.inc();
    }

    /// Counts a block lost to a reorg.
    pub fn record_block_orphaned(&self, count: u64) {
        self.blocks_orphaned.inc_by(count);
    }

    /// Counts broadcast payouts and the value they moved.
    pub fn record_payouts(&self, broadcast: u64, confirmed: u64, value: u64) {
        self.payouts_broadcast.inc_by(broadcast);
        self.payouts_confirmed.inc_by(confirmed);
        self.payout_value.inc_by(value);
    }

    /// Counts a payout the treasury declined to sign.
    pub fn record_payout_refused(&self) {
        self.payouts_refused.inc();
    }

    /// Counts a target raised under backpressure.
    pub fn record_pressure_retune(&self) {
        self.pressure_retunes.inc();
    }

    /// Records how long one share took to verify.
    pub fn observe_validation(&self, seconds: f64) {
        self.validation_duration.observe(seconds);
    }

    /// Records how long a new job took to reach every channel.
    pub fn observe_job_push(&self, seconds: f64) {
        self.job_push_duration.observe(seconds);
    }
}

impl custom_l1_node::metrics::server::Exposition for PoolMetrics {
    fn encode(&self) -> Result<String, std::fmt::Error> {
        Self::encode(self)
    }
}

/// Narrows a `u64` into the `i64` a gauge holds.
///
/// Saturating rather than wrapping: a balance above `i64::MAX` is not a
/// reachable state on this chain, and if one ever appeared, a gauge reading
/// hugely negative would send an operator looking in the wrong direction.
fn clamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_metric_carries_the_pool_prefix() {
        // Sharing a Prometheus instance with the node means the two must not
        // collide on a name.
        let metrics = PoolMetrics::new();
        metrics.set_channels(3);

        let text = metrics.encode().unwrap();
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            if line.is_empty() {
                continue;
            }
            assert!(
                line.starts_with("maya_pool_"),
                "metric outside the pool prefix: {line}"
            );
        }
    }

    #[test]
    fn rejection_labels_come_from_a_closed_set() {
        // One connection able to mint label values is one connection able to
        // exhaust the scraper.
        let metrics = PoolMetrics::new();
        for reason in [
            RejectReason::Stale,
            RejectReason::Duplicate,
            RejectReason::LowDifficulty,
            RejectReason::NonceOutOfRange,
            RejectReason::UnknownChannel,
            RejectReason::InvalidNtime,
        ] {
            metrics.record_rejected(reason);
        }

        let text = metrics.encode().unwrap();
        assert!(text.contains(r#"reason="stale-share""#));
        assert!(text.contains(r#"reason="duplicate-share""#));
    }

    #[test]
    fn no_metric_is_labelled_by_worker() {
        // A rig-name label is a cardinality bomb with one fuse per connection.
        let metrics = PoolMetrics::new();
        metrics.set_workers(10);
        metrics.record_accepted();

        let text = metrics.encode().unwrap();
        assert!(!text.contains("worker="));
        assert!(!text.contains("miner="));
    }

    #[test]
    fn measured_and_reported_are_named_apart() {
        let metrics = PoolMetrics::new();
        metrics.set_hashrate(1_234.9);
        metrics.set_reported_power_milliwatts(4_500_000);

        let text = metrics.encode().unwrap();
        assert!(text.contains("maya_pool_hashrate_hashes_per_second 1234"));
        assert!(text.contains("maya_pool_reported_power_watts 4500"));
    }

    #[test]
    fn a_balance_beyond_a_gauge_saturates_rather_than_going_negative() {
        let metrics = PoolMetrics::new();
        metrics.set_treasury_balance(u64::MAX);

        let text = metrics.encode().unwrap();
        assert!(text.contains(&format!("maya_pool_treasury_balance {}", i64::MAX)));
    }

    #[test]
    fn the_exporter_can_serve_this_registry() {
        // The trait impl is the whole point of generalising the node's
        // exporter; a compile-time check that it still fits.
        fn assert_servable<M: custom_l1_node::metrics::server::Exposition>() {}
        assert_servable::<PoolMetrics>();
    }
}
