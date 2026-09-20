//! The pool's own view of miners, shares, and payouts.
//!
//! Everything here is `serde`, because these are the types the JSON API and the
//! dashboard render. Nothing here reaches consensus; the chain types stay in
//! `custom_l1_node` and meet these only in [`crate::payout`].
//!
//! ## Two identities, deliberately separate
//!
//! A **miner** is an address — the thing that gets paid. A **worker** is one
//! rig belonging to that miner, and exists only so an operator can tell which
//! of their machines stopped. Credits accrue to the miner; statistics accrue to
//! the worker. Keeping them apart is what makes splitting a farm across more
//! workers payout-neutral, which [`crate::pplns`] tests explicitly.

use std::time::Duration;

use custom_l1_node::state::Address;
use serde::{Deserialize, Serialize};

use crate::error::{PoolError, Result};

/// Longest worker name the pool will accept.
///
/// A worker name is a map key and a metric label, so its length is an
/// allocation the connecting side chooses. SV2 caps strings at 255 bytes; this
/// is tighter because a 255-byte rig name is not a rig name.
pub const MAX_WORKER_NAME: usize = 32;

/// Worker name used when a miner supplies an address and nothing else.
pub const DEFAULT_WORKER_NAME: &str = "default";

/// One rig, belonging to one miner.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WorkerKey {
    /// The address credits accrue to.
    pub miner: Address,
    /// The rig's own name, unique within that miner.
    pub worker: String,
}

impl WorkerKey {
    /// Renders the key the way a miner wrote it.
    #[must_use]
    pub fn display(&self) -> String {
        format!("{}.{}", hex::encode(self.miner), self.worker)
    }
}

/// Parses the `user_identity` a channel opened with.
///
/// The format is `<address hex>[.<worker name>]`. An address is required
/// because it is where the money goes, and a pool that invented a default
/// address would be a pool that pays itself.
///
/// # Errors
///
/// Returns [`PoolError::BadRequest`] for a missing, malformed, or over-long
/// identity.
pub fn parse_user_identity(identity: &str) -> Result<WorkerKey> {
    let (address_part, worker_part) = match identity.split_once('.') {
        Some((address, worker)) => (address, worker),
        None => (identity, DEFAULT_WORKER_NAME),
    };

    let bytes = hex::decode(address_part)
        .map_err(|_| PoolError::BadRequest(format!("user identity {identity:?} is not hex")))?;
    let miner: Address = bytes.try_into().map_err(|_| {
        PoolError::BadRequest(format!(
            "user identity {identity:?} does not begin with a 32-byte address"
        ))
    })?;

    if worker_part.is_empty() || worker_part.len() > MAX_WORKER_NAME {
        return Err(PoolError::BadRequest(format!(
            "worker name must be 1..={MAX_WORKER_NAME} bytes, got {}",
            worker_part.len()
        )));
    }

    // A name that is not printable ASCII would reach a metric label and an HTML
    // page. Refusing it here is cheaper than escaping it in two places.
    if !worker_part
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(PoolError::BadRequest(format!(
            "worker name {worker_part:?} must be alphanumeric, '-', or '_'"
        )));
    }

    Ok(WorkerKey {
        miner,
        worker: worker_part.to_string(),
    })
}

/// Why a share was refused.
///
/// Maps one-to-one onto the SV2 error codes so a rejection reaches the miner as
/// the same string the protocol already defines, and reaches Prometheus as a
/// closed set of labels — `src/metrics/mod.rs` explains why an open one would
/// be a memory-exhaustion vector on the scraper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RejectReason {
    /// The nonce fell outside the channel's assigned range.
    NonceOutOfRange,
    /// The job is unknown or has been retired.
    Stale,
    /// This exact `(channel, job, nonce)` was already submitted.
    Duplicate,
    /// The digest did not meet the channel's target.
    LowDifficulty,
    /// The channel id is unknown on this connection.
    UnknownChannel,
    /// The timestamp was before the job's floor, or implausibly far ahead.
    InvalidNtime,
}

impl RejectReason {
    /// The SV2 error code carried back to the miner.
    #[must_use]
    pub fn code(self) -> &'static str {
        use maya_stratum_v2::messages::mining::error_codes as codes;
        match self {
            Self::NonceOutOfRange => codes::NONCE_OUT_OF_RANGE,
            Self::Stale => codes::STALE_SHARE,
            Self::Duplicate => codes::DUPLICATE_SHARE,
            Self::LowDifficulty => codes::DIFFICULTY_TOO_LOW,
            Self::UnknownChannel => codes::UNKNOWN_CHANNEL,
            Self::InvalidNtime => codes::INVALID_NTIME,
        }
    }
}

/// What the validator decided about one submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareOutcome {
    /// Credited.
    Accepted {
        /// Work the share proves, as `2^bits` of the channel's target.
        weight: u64,
        /// Whether the digest also met the *network* target, making this share
        /// a block the pool should submit.
        is_block: bool,
    },
    /// Refused, with the reason the miner is told.
    Rejected(RejectReason),
}

/// A credited share, as the ledger stores it.
///
/// Append-only. Nothing in the pool rewrites a share record: PPLNS reads the
/// window backwards from the tip, and a record that could be edited is a credit
/// that could be moved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareRecord {
    /// Monotonic ledger position. Assigned by the ledger, never by a peer.
    pub sequence: u64,
    /// Who gets paid for it.
    pub miner: Address,
    /// Which of their rigs found it.
    pub worker: String,
    /// Work proven, in units of `2^bits`.
    pub weight: u64,
    /// When the pool accepted it, in milliseconds since the epoch.
    pub accepted_at_millis: u64,
}

/// Live statistics for one rig.
///
/// Everything under `reported_` came from the rig itself and is unverifiable —
/// see [`maya_stratum_v2::messages::mining::SubmitWorkerTelemetry`]. It is here
/// for dashboards and is structurally kept out of [`crate::pplns`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkerStats {
    /// Miner address, hex.
    pub miner: String,
    /// Rig name.
    pub worker: String,
    /// Hash rate the pool *measured* from accepted share weight, in hashes per
    /// second. This is the number that cannot be inflated by claiming it.
    pub measured_hashrate: f64,
    /// Hash rate the rig claimed when it opened or updated its channel.
    ///
    /// Kept beside the measured figure rather than instead of it: a large gap
    /// between the two is the signal an operator actually wants, and averaging
    /// them would erase it.
    pub reported_hashrate: f64,
    /// Shares credited.
    pub accepted_shares: u64,
    /// Shares refused, all reasons.
    pub rejected_shares: u64,
    /// Stale shares, a subset of `rejected_shares`.
    ///
    /// Broken out because staleness is the pool's fault as often as the rig's:
    /// at a 15-second block target, slow job push shows up here first.
    pub stale_shares: u64,
    /// Accepted share weight, all time.
    pub accepted_weight: u64,
    /// Share target currently assigned, in leading zero bits.
    pub target_bits: u32,
    /// Reported wall power, in milliwatts.
    pub reported_power_milliwatts: u64,
    /// Reported hottest sensor, in millidegrees Celsius.
    pub reported_temperature_millicelsius: i32,
    /// Reported fan duty cycle.
    pub reported_fan_percent: u8,
    /// Seconds since the last accepted share, or `None` if there has been none.
    pub seconds_since_share: Option<u64>,
    /// Whether a channel for this worker is currently open.
    pub connected: bool,
}

impl WorkerStats {
    /// Accepted shares as a fraction of all submissions.
    ///
    /// Returns `None` rather than 1.0 for a worker that has submitted nothing:
    /// a rig with no shares has no efficiency, and reporting perfect efficiency
    /// for one is how a dead rig looks healthy on a dashboard.
    #[must_use]
    pub fn efficiency(&self) -> Option<f64> {
        let total = self.accepted_shares + self.rejected_shares;
        if total == 0 {
            return None;
        }
        Some(self.accepted_shares as f64 / total as f64)
    }

    /// Joules per hash implied by the reported power and the measured rate.
    ///
    /// `None` when either input is missing. Deliberately combines a *reported*
    /// numerator with a *measured* denominator, and is named `reported_` in the
    /// API for that reason: the pool can verify the work, never the watts.
    #[must_use]
    pub fn reported_joules_per_hash(&self) -> Option<f64> {
        if self.reported_power_milliwatts == 0 || self.measured_hashrate <= 0.0 {
            return None;
        }
        Some((self.reported_power_milliwatts as f64 / 1_000.0) / self.measured_hashrate)
    }
}

/// A miner's account with the pool.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MinerAccount {
    /// Address, hex.
    pub address: String,
    /// Credits earned from blocks that have matured but not yet been paid.
    pub unpaid: u64,
    /// Credits from blocks that are found but not yet confirmed.
    ///
    /// Separate from `unpaid` because these can still be taken away: a block
    /// that is orphaned earns nothing, and showing the two as one number would
    /// make a reversal look like theft.
    pub immature: u64,
    /// Total ever paid.
    pub paid: u64,
    /// Rigs seen for this miner.
    pub workers: Vec<WorkerStats>,
}

/// Where a payout batch has got to.
///
/// The ordering matters and is one-way apart from the two failure edges:
/// `Pending → Signed → Submitted → Confirmed`, with `Submitted → Orphaned`
/// when a reorg buries the transaction and `Pending → Failed` when the
/// treasury refuses. Nothing moves backwards from `Submitted`, because the
/// bytes are on the network by then and re-signing them would be a second
/// transaction spending the same nonce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayoutState {
    /// Selected and written down, not yet signed.
    Pending,
    /// Signed against a reserved treasury nonce, not yet broadcast.
    Signed,
    /// Broadcast, awaiting confirmations.
    Submitted,
    /// Confirmed to the configured depth.
    Confirmed,
    /// Broadcast, then lost to a reorg. Re-queued for a fresh batch.
    Orphaned,
    /// Refused before signing, and not retried automatically.
    Failed,
}

/// One miner's line in a payout batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayoutEntry {
    /// Recipient.
    pub miner: Address,
    /// Amount, in base units.
    pub amount: u64,
}

/// A batch of payouts, and its progress on chain.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PayoutBatch {
    /// Monotonic batch number, assigned by the ledger.
    pub id: u64,
    /// Treasury nonce reserved for this batch.
    ///
    /// Reserved before signing and never reused. This is the field that makes a
    /// crash between signing and broadcast safe: the resumed batch re-broadcasts
    /// the same bytes rather than signing new ones, so the chain sees one
    /// transaction whichever path the daemon took.
    pub nonce: u64,
    /// Recipients and amounts.
    pub entries: Vec<PayoutEntry>,
    /// Signed transaction bytes, once signed.
    #[serde(with = "hex_bytes")]
    pub signed_tx: Vec<u8>,
    /// Transaction id, once signed.
    pub txid: Option<String>,
    /// Current state.
    pub state: PayoutState,
    /// When the batch was created, in milliseconds since the epoch.
    pub created_at_millis: u64,
    /// Height the transaction was first seen at, once confirmed.
    pub included_height: Option<u64>,
}

impl PayoutBatch {
    /// Total value the batch moves.
    ///
    /// Saturating rather than wrapping: this feeds the treasury's spend cap,
    /// and a wrapped total would compare as small.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.entries
            .iter()
            .fold(0u64, |sum, entry| sum.saturating_add(entry.amount))
    }
}

/// Hex encoding for byte vectors in JSON.
mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serializes bytes as a hex string.
    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(bytes))
    }

    /// Parses bytes from a hex string.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        hex::decode(&text).map_err(serde::de::Error::custom)
    }
}

/// Milliseconds since the Unix epoch.
///
/// The pool timestamps its own records with the local clock and never with a
/// value a peer supplied. `src/metrics/mod.rs` makes the same distinction for
/// block age: a number derived from someone else's clock measures their skew
/// as much as anything else.
#[must_use]
pub fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const ADDRESS_HEX: &str = "11223344556677889900aabbccddeeff\
                               11223344556677889900aabbccddeeff";

    #[test]
    fn an_identity_without_a_worker_gets_the_default_name() {
        let key = parse_user_identity(ADDRESS_HEX).unwrap();
        assert_eq!(key.worker, DEFAULT_WORKER_NAME);
    }

    #[test]
    fn an_identity_with_a_worker_splits_on_the_first_dot() {
        let key = parse_user_identity(&format!("{ADDRESS_HEX}.rig-3")).unwrap();
        assert_eq!(key.worker, "rig-3");
        assert_eq!(key.miner[0], 0x11);
    }

    #[test]
    fn an_identity_without_an_address_is_refused() {
        // No default address, ever: a pool that invents one pays itself.
        assert!(parse_user_identity("rig-3").is_err());
        assert!(parse_user_identity("").is_err());
    }

    #[test]
    fn a_short_address_is_refused() {
        assert!(parse_user_identity("1122334455").is_err());
    }

    #[test]
    fn a_worker_name_that_would_reach_a_metric_label_is_refused() {
        for name in ["rig 3", "rig/3", "<script>", "rig\n3"] {
            let identity = format!("{ADDRESS_HEX}.{name}");
            assert!(
                parse_user_identity(&identity).is_err(),
                "worker name {name:?} was accepted"
            );
        }
    }

    #[test]
    fn an_over_long_worker_name_is_refused() {
        let identity = format!("{ADDRESS_HEX}.{}", "a".repeat(MAX_WORKER_NAME + 1));
        assert!(parse_user_identity(&identity).is_err());
    }

    #[test]
    fn a_worker_that_has_submitted_nothing_has_no_efficiency() {
        // Not 1.0. A dead rig must not read as a perfect one.
        assert_eq!(WorkerStats::default().efficiency(), None);
    }

    #[test]
    fn efficiency_counts_every_rejection() {
        let stats = WorkerStats {
            accepted_shares: 3,
            rejected_shares: 1,
            ..WorkerStats::default()
        };
        assert_eq!(stats.efficiency(), Some(0.75));
    }

    #[test]
    fn a_batch_total_saturates_rather_than_wrapping() {
        let batch = PayoutBatch {
            id: 1,
            nonce: 1,
            entries: vec![
                PayoutEntry {
                    miner: [0u8; 32],
                    amount: u64::MAX,
                },
                PayoutEntry {
                    miner: [1u8; 32],
                    amount: 10,
                },
            ],
            signed_tx: Vec::new(),
            txid: None,
            state: PayoutState::Pending,
            created_at_millis: 0,
            included_height: None,
        };

        // A wrapped total would compare as small against the spend cap, which
        // is the one comparison that must never be fooled.
        assert_eq!(batch.total(), u64::MAX);
    }
}
