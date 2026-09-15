//! Evidence that a peer is Byzantine, and quarantine.
//!
//! # What counts
//!
//! Only what a peer is provably responsible for. With gossip validation on
//! (`behaviour.rs`), an honest node forwards a message only after it checked
//! it, so a message that fails the same stateless check came from a peer that
//! either wrote it or relayed it unchecked. Either is the peer's fault.
//!
//! | Offence | Evidence | Weight |
//! |---|---|---|
//! | [`Offence::MalformedFrame`] | gossip bytes that do not decode | 25 |
//! | [`Offence::InvalidSignature`] | a transaction whose hybrid signature fails | 50 |
//! | [`Offence::TxRootMismatch`] | a block body that disagrees with its header — invariant 24's relay substitution | 50 |
//! | [`Offence::InvalidBlock`] | a block the chain refuses for a reason inside the block | 50 |
//! | [`Offence::BadSyncResponse`] | a sync answer carrying blocks nobody asked for, or undecodable ones | 50 |
//!
//! # What does not, and why
//!
//! - **Late block propagation.** A distant honest peer is late, and scoring
//!   lateness would let an attacker get honest peers quarantined by being
//!   closer — the first step of an eclipse. It is recorded for monitoring
//!   ([`PeerHealth::observe_latency`]) and never scored.
//! - **Double proposals.** A proof-of-work block has no proposer. Two blocks at
//!   one height are a fork, forks are normal, and relaying both is honest.
//! - **Transactions refused for state reasons** — a stale nonce, an
//!   insufficient balance, a full pool. Honest relays race each other, so these
//!   are `Ignore`, not `Reject`.
//! - **Unknown parents.** A block whose parent has not arrived is out of order,
//!   not wrong.
//!
//! # Quarantine
//!
//! A score decays with a half-life. Crossing [`GuardConfig::quarantine_score`]
//! quarantines the peer: the driver blacklists it in gossipsub (every message
//! it relays or authored is dropped), blocks its connections, and disconnects
//! it. Quarantine expires, because a node that bans forever eventually bans an
//! honest peer that was briefly misconfigured; each repeat doubles the length
//! up to a cap. This is all node-local and changes no consensus rule.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use libp2p::PeerId;
use libp2p::gossipsub::MessageAcceptance;

use crate::core::Block;
use crate::error::{NodeError, Result};

/// Something a peer is provably responsible for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Offence {
    /// Gossip bytes that do not decode.
    MalformedFrame,
    /// A transaction whose signature does not verify.
    InvalidSignature,
    /// A block body that disagrees with its header's `tx_root`.
    TxRootMismatch,
    /// A block the chain refuses for a reason inside the block.
    InvalidBlock,
    /// A sync response that answers a question nobody asked.
    BadSyncResponse,
}

/// Every offence, for counters.
pub const OFFENCES: [Offence; 5] = [
    Offence::MalformedFrame,
    Offence::InvalidSignature,
    Offence::TxRootMismatch,
    Offence::InvalidBlock,
    Offence::BadSyncResponse,
];

impl Offence {
    /// Score added per occurrence.
    #[must_use]
    pub const fn weight(self) -> f64 {
        match self {
            Self::MalformedFrame => 25.0,
            Self::InvalidSignature
            | Self::TxRootMismatch
            | Self::InvalidBlock
            | Self::BadSyncResponse => 50.0,
        }
    }

    /// Fixed label for metrics. Never derived from peer input.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::MalformedFrame => "malformed_frame",
            Self::InvalidSignature => "invalid_signature",
            Self::TxRootMismatch => "tx_root_mismatch",
            Self::InvalidBlock => "invalid_block",
            Self::BadSyncResponse => "bad_sync_response",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::MalformedFrame => 0,
            Self::InvalidSignature => 1,
            Self::TxRootMismatch => 2,
            Self::InvalidBlock => 3,
            Self::BadSyncResponse => 4,
        }
    }
}

/// What gossipsub is told about one message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Valid: forward it.
    Accept,
    /// Not forwarded, not the sender's fault.
    Ignore,
    /// Not forwarded, and the sender answers for it.
    Reject(Offence),
}

impl Verdict {
    /// The gossipsub acceptance.
    #[must_use]
    pub const fn acceptance(self) -> MessageAcceptance {
        match self {
            Self::Accept => MessageAcceptance::Accept,
            Self::Ignore => MessageAcceptance::Ignore,
            Self::Reject(_) => MessageAcceptance::Reject,
        }
    }

    /// The offence, if the sender answers for one.
    #[must_use]
    pub const fn offence(self) -> Option<Offence> {
        match self {
            Self::Reject(offence) => Some(offence),
            Self::Accept | Self::Ignore => None,
        }
    }
}

/// The verdict on a gossiped transaction, from the mempool's answer.
#[must_use]
pub fn classify_transaction(result: &Result<bool>) -> Verdict {
    match result {
        Ok(true) => Verdict::Accept,
        Ok(false) => Verdict::Ignore,
        Err(
            NodeError::Decode(_) | NodeError::InvalidLength { .. } | NodeError::MalformedPublicKey,
        ) => Verdict::Reject(Offence::MalformedFrame),
        Err(
            NodeError::SignatureVerification
            | NodeError::HashSignatureVerification
            | NodeError::MissingSignature,
        ) => Verdict::Reject(Offence::InvalidSignature),
        Err(_) => Verdict::Ignore,
    }
}

/// The verdict on a gossiped block frame: stateless checks only.
///
/// Decodes and checks `tx_root`, both cheap. Everything that needs the chain —
/// proof of work, execution, the state root — is the importer's, which reports
/// an [`Offence::InvalidBlock`] afterwards through `NodeHandle::report_offence`.
#[must_use]
pub fn classify_block_frame(decoded: &Result<Block>) -> Verdict {
    match decoded {
        Err(_) => Verdict::Reject(Offence::MalformedFrame),
        Ok(block) if block.check_tx_root().is_err() => Verdict::Reject(Offence::TxRootMismatch),
        Ok(_) => Verdict::Accept,
    }
}

/// Whether a chain refusal is the block's fault.
///
/// An allow-list, not a deny-list: only refusals that are facts about the
/// block itself score, and anything else — a new `NodeError` variant included —
/// does not until somebody classifies it. The failure modes are not symmetric:
/// an unscored invalid block costs one import, a scored honest block costs an
/// honest peer.
///
/// Not scored, deliberately: `NodeError::Network`, which the chain uses both for
/// an unknown parent (not the block's fault) and for a wrong difficulty or
/// insufficient work (the block's fault, but indistinguishable here without
/// parsing a message); storage, archive, prune-horizon and proof-of-work-cache
/// failures, which are this node's.
#[must_use]
pub fn classify_import(error: &NodeError) -> Option<Offence> {
    match error {
        NodeError::TxRootMismatch { .. }
        | NodeError::StateRootMismatch { .. }
        | NodeError::InvariantViolation(_)
        | NodeError::SignatureVerification
        | NodeError::HashSignatureVerification
        | NodeError::MissingSignature
        | NodeError::MalformedPublicKey
        | NodeError::InvalidNonce { .. }
        | NodeError::InsufficientBalance { .. }
        | NodeError::BalanceOverflow
        | NodeError::MixedTransactionKind(_)
        | NodeError::Decode(_)
        | NodeError::InvalidLength { .. } => Some(Offence::InvalidBlock),
        _ => None,
    }
}

/// Thresholds and durations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuardConfig {
    /// Score at which a peer is quarantined.
    pub quarantine_score: f64,
    /// Time for a score to halve.
    pub half_life: Duration,
    /// Length of a first quarantine.
    pub first_quarantine: Duration,
    /// Longest quarantine, however many repeats.
    pub max_quarantine: Duration,
    /// Peers tracked at once. Identities are free, so this is what stops a
    /// flood of them growing the table without bound.
    pub max_tracked_peers: usize,
}

impl GuardConfig {
    /// Two invalid signatures, or four malformed frames, inside a few minutes.
    pub const DEFAULT: Self = Self {
        quarantine_score: 100.0,
        half_life: Duration::from_secs(600),
        first_quarantine: Duration::from_secs(600),
        max_quarantine: Duration::from_secs(24 * 3600),
        max_tracked_peers: 4_096,
    };
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// What the driver must do after an offence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing yet.
    None,
    /// Quarantine the peer until `until`.
    Quarantine {
        /// When it ends.
        until: Instant,
        /// How many times this peer has been quarantined, this one included.
        strikes: u32,
    },
}

/// One peer's record, for monitoring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeerReport {
    /// Current, decayed score.
    pub score: f64,
    /// Occurrences of each offence, in [`OFFENCES`] order.
    pub offences: [u32; 5],
    /// Quarantines so far.
    pub strikes: u32,
    /// Whether quarantined now.
    pub quarantined: bool,
    /// Messages dropped because the peer was quarantined.
    pub dropped: u64,
    /// Mean observed block propagation delay, unscored.
    pub mean_latency: Option<Duration>,
}

#[derive(Clone, Debug)]
struct PeerRecord {
    score: f64,
    updated: Instant,
    offences: [u32; 5],
    strikes: u32,
    until: Option<Instant>,
    dropped: u64,
    latency_total: Duration,
    latency_samples: u32,
}

impl PeerRecord {
    fn new(now: Instant) -> Self {
        Self {
            score: 0.0,
            updated: now,
            offences: [0; 5],
            strikes: 0,
            until: None,
            dropped: 0,
            latency_total: Duration::ZERO,
            latency_samples: 0,
        }
    }

    fn decay(&mut self, half_life: Duration, now: Instant) {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        let halves = elapsed / half_life.as_secs_f64().max(f64::MIN_POSITIVE);
        self.score *= 0.5f64.powf(halves);
        self.updated = now;
    }
}

/// The table of peers and their evidence.
#[derive(Clone, Debug)]
pub struct PeerHealth {
    config: GuardConfig,
    peers: HashMap<PeerId, PeerRecord>,
}

impl PeerHealth {
    /// An empty table.
    #[must_use]
    pub fn new(config: GuardConfig) -> Self {
        Self {
            config,
            peers: HashMap::new(),
        }
    }

    /// Records `offence` against `peer`.
    pub fn record(&mut self, peer: PeerId, offence: Offence, now: Instant) -> Action {
        self.make_room(&peer);
        let config = self.config;
        let record = self
            .peers
            .entry(peer)
            .or_insert_with(|| PeerRecord::new(now));
        record.decay(config.half_life, now);
        record.offences[offence.index()] = record.offences[offence.index()].saturating_add(1);
        if record.until.is_some_and(|until| until > now) {
            return Action::None;
        }
        record.score += offence.weight();
        if record.score < config.quarantine_score {
            return Action::None;
        }

        record.strikes = record.strikes.saturating_add(1);
        let doublings = record.strikes.saturating_sub(1).min(16);
        let length = config
            .first_quarantine
            .saturating_mul(1u32 << doublings)
            .min(config.max_quarantine);
        let until = now + length;
        record.until = Some(until);
        record.score = 0.0;
        Action::Quarantine {
            until,
            strikes: record.strikes,
        }
    }

    /// Ends every quarantine due by `now`, returning the peers released.
    pub fn release_expired(&mut self, now: Instant) -> Vec<PeerId> {
        let mut released = Vec::new();
        for (peer, record) in &mut self.peers {
            if record.until.is_some_and(|until| until <= now) {
                record.until = None;
                released.push(*peer);
            }
        }
        released
    }

    /// Whether `peer` is quarantined at `now`.
    #[must_use]
    pub fn is_quarantined(&self, peer: &PeerId, now: Instant) -> bool {
        self.peers
            .get(peer)
            .and_then(|record| record.until)
            .is_some_and(|until| until > now)
    }

    /// Counts a message dropped from a quarantined peer.
    pub fn note_dropped(&mut self, peer: &PeerId) {
        if let Some(record) = self.peers.get_mut(peer) {
            record.dropped = record.dropped.saturating_add(1);
        }
    }

    /// Records a block propagation delay. Monitoring only: never scored.
    pub fn observe_latency(&mut self, peer: PeerId, delay: Duration, now: Instant) {
        self.make_room(&peer);
        let record = self
            .peers
            .entry(peer)
            .or_insert_with(|| PeerRecord::new(now));
        record.latency_total = record.latency_total.saturating_add(delay);
        record.latency_samples = record.latency_samples.saturating_add(1);
    }

    /// The record for `peer`, decayed to `now`.
    #[must_use]
    pub fn report(&self, peer: &PeerId, now: Instant) -> Option<PeerReport> {
        let mut record = self.peers.get(peer)?.clone();
        record.decay(self.config.half_life, now);
        Some(PeerReport {
            score: record.score,
            offences: record.offences,
            strikes: record.strikes,
            quarantined: record.until.is_some_and(|until| until > now),
            dropped: record.dropped,
            mean_latency: (record.latency_samples > 0)
                .then(|| record.latency_total / record.latency_samples),
        })
    }

    /// Peers tracked.
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.peers.len()
    }

    /// Evicts the lowest-scored unquarantined peer if the table is full and
    /// `incoming` is new. A quarantined peer is never evicted: forgetting it
    /// would release it.
    fn make_room(&mut self, incoming: &PeerId) {
        if self.peers.len() < self.config.max_tracked_peers || self.peers.contains_key(incoming) {
            return;
        }
        let victim = self
            .peers
            .iter()
            .filter(|(_, record)| record.until.is_none())
            .min_by(|a, b| a.1.score.total_cmp(&b.1.score))
            .map(|(peer, _)| *peer);
        if let Some(victim) = victim {
            self.peers.remove(&victim);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> GuardConfig {
        GuardConfig {
            max_tracked_peers: 3,
            ..GuardConfig::DEFAULT
        }
    }

    #[test]
    fn two_invalid_signatures_quarantine_and_one_does_not() {
        let mut health = PeerHealth::new(config());
        let (peer, now) = (PeerId::random(), Instant::now());
        assert_eq!(
            health.record(peer, Offence::InvalidSignature, now),
            Action::None
        );
        assert!(matches!(
            health.record(peer, Offence::InvalidSignature, now),
            Action::Quarantine { strikes: 1, .. }
        ));
        assert!(health.is_quarantined(&peer, now));
        assert!(!health.is_quarantined(&peer, now + GuardConfig::DEFAULT.first_quarantine));
    }

    #[test]
    fn a_score_halves_every_half_life() {
        let mut health = PeerHealth::new(config());
        let (peer, now) = (PeerId::random(), Instant::now());
        health.record(peer, Offence::InvalidSignature, now);
        let later = now + GuardConfig::DEFAULT.half_life;
        let score = health.report(&peer, later).expect("tracked").score;
        assert!((score - 25.0).abs() < 1e-6, "{score}");
        // Spread out, the same two offences never reach the threshold.
        assert_eq!(
            health.record(peer, Offence::InvalidSignature, later),
            Action::None
        );
    }

    #[test]
    fn repeat_quarantines_double_and_stop_at_the_cap() {
        let mut health = PeerHealth::new(config());
        let peer = PeerId::random();
        let mut now = Instant::now();
        let mut lengths = Vec::new();
        for _ in 0..12 {
            health.record(peer, Offence::InvalidBlock, now);
            let Action::Quarantine { until, .. } = health.record(peer, Offence::InvalidBlock, now)
            else {
                panic!("expected a quarantine");
            };
            lengths.push(until - now);
            now = until;
            assert_eq!(health.release_expired(now), vec![peer]);
        }
        assert_eq!(lengths[1], lengths[0] * 2);
        assert_eq!(
            *lengths.last().expect("some"),
            GuardConfig::DEFAULT.max_quarantine
        );
    }

    #[test]
    fn offences_during_a_quarantine_count_but_do_not_extend_it() {
        let mut health = PeerHealth::new(config());
        let (peer, now) = (PeerId::random(), Instant::now());
        health.record(peer, Offence::InvalidSignature, now);
        health.record(peer, Offence::InvalidSignature, now);
        for _ in 0..10 {
            assert_eq!(
                health.record(peer, Offence::MalformedFrame, now),
                Action::None
            );
        }
        let report = health.report(&peer, now).expect("tracked");
        assert_eq!(report.offences[Offence::MalformedFrame.index()], 10);
        assert_eq!(report.strikes, 1);
    }

    #[test]
    fn a_full_table_evicts_the_cleanest_peer_and_never_a_quarantined_one() {
        let mut health = PeerHealth::new(config());
        let now = Instant::now();
        let jailed = PeerId::random();
        health.record(jailed, Offence::InvalidBlock, now);
        health.record(jailed, Offence::InvalidBlock, now);
        let (clean, dirty) = (PeerId::random(), PeerId::random());
        health.observe_latency(clean, Duration::from_millis(5), now);
        health.record(dirty, Offence::MalformedFrame, now);

        health.record(PeerId::random(), Offence::MalformedFrame, now);

        assert_eq!(health.tracked(), 3);
        assert!(health.is_quarantined(&jailed, now));
        assert!(health.report(&clean, now).is_none());
        assert!(health.report(&dirty, now).is_some());
    }

    #[test]
    fn latency_is_recorded_and_never_scored() {
        let mut health = PeerHealth::new(config());
        let (peer, now) = (PeerId::random(), Instant::now());
        for _ in 0..1_000 {
            health.observe_latency(peer, Duration::from_secs(30), now);
        }
        let report = health.report(&peer, now).expect("tracked");
        assert_eq!(report.score, 0.0);
        assert_eq!(report.mean_latency, Some(Duration::from_secs(30)));
    }

    #[test]
    fn transactions_refused_for_state_reasons_are_ignored_not_rejected() {
        assert_eq!(classify_transaction(&Ok(true)), Verdict::Accept);
        assert_eq!(classify_transaction(&Ok(false)), Verdict::Ignore);
        let stale = Err(NodeError::InvalidNonce {
            address: String::new(),
            expected: 1,
            actual: 0,
        });
        assert_eq!(classify_transaction(&stale), Verdict::Ignore);
        assert_eq!(
            classify_transaction(&Err(NodeError::SignatureVerification)),
            Verdict::Reject(Offence::InvalidSignature)
        );
        assert_eq!(
            classify_transaction(&Err(NodeError::Decode("x".into()))),
            Verdict::Reject(Offence::MalformedFrame)
        );
    }

    #[test]
    fn an_unknown_parent_or_a_storage_failure_is_not_the_blocks_fault() {
        assert_eq!(
            classify_import(&NodeError::Network("unknown block".into())),
            None
        );
        assert_eq!(classify_import(&NodeError::Storage("disk".into())), None);
        assert_eq!(
            classify_import(&NodeError::StateRootMismatch {
                expected: String::new(),
                actual: String::new(),
            }),
            Some(Offence::InvalidBlock)
        );
    }
}
