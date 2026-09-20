//! Prometheus instrumentation.
//!
//! ## Why metrics live on their own port
//!
//! Peer topology and mempool contents are operational data, not public data.
//! Serving them alongside JSON-RPC would publish the node's peer set and its
//! pending transactions to anyone who can reach the RPC endpoint, which for a
//! load-balanced node is the whole internet. The exporter therefore binds
//! separately and is expected to stay inside the pod network.
//!
//! ## Measuring propagation honestly
//!
//! "Block propagation latency" is usually computed as the gap between a block's
//! header timestamp and the moment a node saw it. That number is real, but it
//! measures clock skew between the miner and the observer just as much as it
//! measures network delay — an operator whose NTP has drifted will see a
//! latency spike caused entirely by their own clock.
//!
//! So both are exported, under names that say which is which:
//!
//! - [`Metrics::observe_block_age`] records `maya_block_observed_age_seconds`,
//!   the skew-sensitive end-to-end figure.
//! - [`Metrics::observe_import`] records `maya_block_import_duration_seconds`,
//!   measured entirely against the local clock and therefore skew-free.
//!
//! Alert on the second; watch the first for trends.

pub mod server;

use std::sync::atomic::{AtomicI64, AtomicU64};

use prometheus_client::encoding::text::encode;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;

/// Bucket bounds for block age, in seconds.
///
/// Spread around the 15-second target block time: sub-second at the fast end to
/// resolve a healthy mesh, out to a minute to catch a partition.
fn age_buckets() -> impl Iterator<Item = f64> {
    [0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 15.0, 30.0, 60.0].into_iter()
}

/// Bucket bounds for block import, in seconds.
///
/// Import is validation plus a RocksDB batch, so the interesting range is much
/// tighter than propagation.
fn import_buckets() -> impl Iterator<Item = f64> {
    [0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5].into_iter()
}

/// Labels distinguishing why a block was rejected.
#[derive(Clone, Debug, Hash, PartialEq, Eq, prometheus_client::encoding::EncodeLabelSet)]
pub struct RejectionLabels {
    /// Short, bounded reason string.
    ///
    /// Deliberately a small closed set: a label whose values come from error
    /// text would let a hostile peer create unbounded time series and exhaust
    /// the scraper's memory.
    pub reason: &'static str,
}

/// Every metric the node exports.
#[derive(Debug)]
pub struct Metrics {
    /// Registry handed to the exporter.
    registry: Registry,

    /// Currently connected libp2p peers.
    peers: Gauge<i64, AtomicI64>,
    /// Transactions waiting in the mempool.
    mempool_transactions: Gauge<i64, AtomicI64>,
    /// Height of the active chain tip.
    height: Gauge<i64, AtomicI64>,
    /// Proof-of-work difficulty at the tip, as leading zero bits.
    ///
    /// The target itself is a 256-bit value, which does not fit a gauge.
    /// Leading zero bits is its log scale and is what operators actually read.
    difficulty_bits: Gauge<i64, AtomicI64>,
    /// Notes held by the shielded pool.
    shielded_notes: Gauge<i64, AtomicI64>,
    /// Value currently inside the shielded pool.
    shielded_balance: Gauge<i64, AtomicI64>,

    /// Blocks accepted into the chain.
    blocks_imported: Counter<u64, AtomicU64>,
    /// Blocks rejected, by reason.
    blocks_rejected: Family<RejectionLabels, Counter<u64, AtomicU64>>,

    /// Post-quantum sessions established, inbound and outbound.
    ///
    /// Every connection runs the ML-KEM-768 upgrade, so this is also the
    /// connection count — but naming it for the handshake is the point. An
    /// operator watching a post-quantum rollout wants to see this rise, and
    /// wants `pq_sessions_rotated` to rise with it rather than instead of it.
    pq_sessions_established: Counter<u64, AtomicU64>,
    /// Sessions closed because their keys aged past the rotation epoch.
    pq_sessions_rotated: Counter<u64, AtomicU64>,
    /// The rotation epoch this node currently considers itself in.
    pq_rotation_epoch: Gauge<i64, AtomicI64>,

    /// Gap between a block's stated timestamp and local receipt.
    block_observed_age: Histogram,
    /// Wall time spent validating and committing a block locally.
    block_import_duration: Histogram,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    /// Builds the registry and registers every collector.
    #[must_use]
    pub fn new() -> Self {
        let mut registry = <Registry>::with_prefix("maya");

        let peers = Gauge::default();
        registry.register(
            "peers_connected",
            "libp2p peers currently connected",
            peers.clone(),
        );

        let mempool_transactions = Gauge::default();
        registry.register(
            "mempool_transactions",
            "Transactions waiting in the mempool",
            mempool_transactions.clone(),
        );

        let height = Gauge::default();
        registry.register("chain_height", "Height of the active tip", height.clone());

        let difficulty_bits = Gauge::default();
        registry.register(
            "difficulty_leading_zero_bits",
            "Proof-of-work difficulty at the tip, in leading zero bits",
            difficulty_bits.clone(),
        );

        let shielded_notes = Gauge::default();
        registry.register(
            "shielded_notes",
            "Note commitments in the shielded pool",
            shielded_notes.clone(),
        );

        let shielded_balance = Gauge::default();
        registry.register(
            "shielded_balance",
            "Value held inside the shielded pool, in base units",
            shielded_balance.clone(),
        );

        // Registered without the `_total` suffix: prometheus-client appends it
        // to counter sample lines itself, so naming it here yields
        // `maya_blocks_imported_total_total`.
        let blocks_imported = Counter::default();
        registry.register(
            "blocks_imported",
            "Blocks accepted from peers. Excludes blocks this node mined itself, \
             which never travel the import path — use chain_height to tell \
             whether the chain is advancing at all",
            blocks_imported.clone(),
        );

        let pq_sessions_established = Counter::<u64, AtomicU64>::default();
        registry.register(
            "pq_sessions_established",
            "ML-KEM-768 transport sessions established",
            pq_sessions_established.clone(),
        );

        let pq_sessions_rotated = Counter::<u64, AtomicU64>::default();
        registry.register(
            "pq_sessions_rotated",
            "Transport sessions closed because their post-quantum keys aged out",
            pq_sessions_rotated.clone(),
        );

        let pq_rotation_epoch = Gauge::<i64, AtomicI64>::default();
        registry.register(
            "pq_rotation_epoch",
            "Rotation epoch this node considers itself in",
            pq_rotation_epoch.clone(),
        );

        let blocks_rejected = Family::<RejectionLabels, Counter>::default();
        registry.register(
            "blocks_rejected",
            "Blocks rejected, by reason",
            blocks_rejected.clone(),
        );

        let block_observed_age = Histogram::new(age_buckets());
        registry.register(
            "block_observed_age_seconds",
            "Gap between a block's header timestamp and local receipt; \
             includes clock skew between miner and observer",
            block_observed_age.clone(),
        );

        let block_import_duration = Histogram::new(import_buckets());
        registry.register(
            "block_import_duration_seconds",
            "Wall time spent validating and committing a block locally",
            block_import_duration.clone(),
        );

        Self {
            registry,
            peers,
            mempool_transactions,
            height,
            difficulty_bits,
            shielded_notes,
            shielded_balance,
            blocks_imported,
            blocks_rejected,
            pq_sessions_established,
            pq_sessions_rotated,
            pq_rotation_epoch,
            block_observed_age,
            block_import_duration,
        }
    }

    /// Records the number of connected peers.
    pub fn set_peers(&self, count: usize) {
        self.peers.set(count as i64);
    }

    /// Records mempool depth.
    pub fn set_mempool_depth(&self, transactions: usize) {
        self.mempool_transactions.set(transactions as i64);
    }

    /// Records the tip height.
    pub fn set_height(&self, height: u64) {
        self.height.set(height as i64);
    }

    /// Records difficulty as the number of leading zero bits in the target.
    pub fn set_difficulty(&self, target: &[u8; 32]) {
        self.difficulty_bits
            .set(i64::from(leading_zero_bits(target)));
    }

    /// Records shielded pool size.
    pub fn set_shielded(&self, notes: u64, balance: u64) {
        self.shielded_notes.set(notes as i64);
        // Saturating rather than wrapping: a balance beyond `i64::MAX` would
        // otherwise report as negative, which reads as a bug in the chain
        // rather than in the exporter.
        self.shielded_balance
            .set(i64::try_from(balance).unwrap_or(i64::MAX));
    }

    /// Publishes the post-quantum transport totals.
    ///
    /// Takes absolutes rather than incrementing, because the source is a pair
    /// of running counters the network driver owns and this loop samples — the
    /// same pattern as [`Metrics::set_peers`]. `inc_by` on a monotonic counter
    /// would double-count every sample.
    pub fn set_pq_sessions(&self, established: u64, rotated: u64, epoch: u64) {
        self.pq_sessions_established
            .inc_by(established.saturating_sub(self.pq_sessions_established.get()));
        self.pq_sessions_rotated
            .inc_by(rotated.saturating_sub(self.pq_sessions_rotated.get()));

        // `try_from` rather than `as`: prometheus gauges are i64, and an epoch
        // that overflowed would report a negative one, which reads as a bug in
        // the exporter rather than in whatever produced the epoch.
        self.pq_rotation_epoch
            .set(i64::try_from(epoch).unwrap_or(i64::MAX));
    }

    /// Counts an accepted block.
    pub fn record_import(&self) {
        self.blocks_imported.inc();
    }

    /// Counts a rejected block.
    pub fn record_rejection(&self, reason: &'static str) {
        self.blocks_rejected
            .get_or_create(&RejectionLabels { reason })
            .inc();
    }

    /// Observes how old a block appeared to be on arrival.
    ///
    /// A block whose timestamp is in the future — clock skew, or a miner
    /// reaching forward — yields a negative age. Those are clamped to zero
    /// rather than dropped, so the observation count still matches the number
    /// of blocks seen.
    pub fn observe_block_age(&self, header_timestamp: u64, received_at: u64) {
        let age = received_at.saturating_sub(header_timestamp);
        self.block_observed_age.observe(age as f64);
    }

    /// Observes how long importing a block took.
    pub fn observe_import(&self, duration: std::time::Duration) {
        self.block_import_duration.observe(duration.as_secs_f64());
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
}

/// Counts leading zero bits in a 256-bit target.
fn leading_zero_bits(target: &[u8; 32]) -> u32 {
    let mut bits = 0;
    for byte in target {
        bits += byte.leading_zeros();
        if *byte != 0 {
            break;
        }
    }
    bits
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn leading_zero_bits_counts_across_byte_boundaries() {
        assert_eq!(leading_zero_bits(&[0u8; 32]), 256);
        let mut target = [0u8; 32];
        target[0] = 0x80;
        assert_eq!(leading_zero_bits(&target), 0);
        target[0] = 0x01;
        assert_eq!(leading_zero_bits(&target), 7);
        target = [0u8; 32];
        target[1] = 0x01;
        assert_eq!(leading_zero_bits(&target), 15);
    }

    #[test]
    fn the_exposition_parses_and_names_are_prefixed() {
        let metrics = Metrics::new();
        metrics.set_peers(3);
        metrics.set_mempool_depth(7);
        metrics.set_height(42);

        let text = metrics.encode().expect("encode");
        assert!(text.contains("maya_peers_connected 3"));
        assert!(text.contains("maya_mempool_transactions 7"));
        assert!(text.contains("maya_chain_height 42"));
        // Every metric must carry help and type lines, or a scraper treats it
        // as untyped.
        assert!(text.contains("# HELP maya_peers_connected"));
        assert!(text.contains("# TYPE maya_peers_connected gauge"));
    }

    #[test]
    fn counters_accumulate() {
        let metrics = Metrics::new();
        metrics.record_import();
        metrics.record_import();
        metrics.record_rejection("bad_pow");

        let text = metrics.encode().expect("encode");
        assert!(text.contains("maya_blocks_imported_total 2"));
        assert!(text.contains(r#"maya_blocks_rejected_total{reason="bad_pow"} 1"#));
    }

    #[test]
    fn histograms_record_observations() {
        let metrics = Metrics::new();
        metrics.observe_block_age(100, 103);
        metrics.observe_import(std::time::Duration::from_millis(12));

        let text = metrics.encode().expect("encode");
        assert!(text.contains("maya_block_observed_age_seconds_count 1"));
        assert!(text.contains("maya_block_observed_age_seconds_sum 3.0"));
        assert!(text.contains("maya_block_import_duration_seconds_count 1"));
    }

    #[test]
    fn a_block_from_the_future_reports_zero_age_rather_than_wrapping() {
        let metrics = Metrics::new();
        // Received before its own timestamp: skew, not a negative delay.
        metrics.observe_block_age(500, 100);

        let text = metrics.encode().expect("encode");
        assert!(text.contains("maya_block_observed_age_seconds_sum 0.0"));
        assert!(text.contains("maya_block_observed_age_seconds_count 1"));
    }

    #[test]
    fn difficulty_is_reported_as_leading_zero_bits() {
        let metrics = Metrics::new();
        let mut target = [0u8; 32];
        target[2] = 0x40;
        metrics.set_difficulty(&target);
        assert!(
            metrics
                .encode()
                .expect("encode")
                .contains("maya_difficulty_leading_zero_bits 17")
        );
    }

    #[test]
    fn an_oversized_shielded_balance_saturates_instead_of_going_negative() {
        let metrics = Metrics::new();
        metrics.set_shielded(2, u64::MAX);
        let text = metrics.encode().expect("encode");
        assert!(text.contains(&format!("maya_shielded_balance {}", i64::MAX)));
        assert!(!text.contains("maya_shielded_balance -"));
    }
}
