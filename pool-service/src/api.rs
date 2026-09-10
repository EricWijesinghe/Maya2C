//! The JSON API.
//!
//! Read-only, and deliberately so. Everything that changes pool state arrives
//! over the authenticated Stratum port or is decided by the daemon's own loops.
//! An HTTP endpoint that could retarget a channel or trigger a payout would be
//! a second, weaker way into the parts of the pool that handle money.
//!
//! ## Shapes are the ones the dashboard renders
//!
//! The same types [`crate::ui`] uses, serialised. Two representations of a rig
//! would be two things to keep in step, and the one that drifts is always the
//! one without a page rendering it.
//!
//! ## Reported figures keep their prefix in JSON too
//!
//! `reported_hashrate`, `reported_power_milliwatts`, and the rest are named
//! that way in the wire format, not only on the page. A consumer writing an
//! alert against this API needs to know which numbers the pool measured and
//! which a rig asserted.

use serde::{Deserialize, Serialize};

use custom_l1_node::state::Address;

use crate::error::{PoolError, Result};
use crate::ledger::{FoundBlock, MinerBalance};
use crate::model::{PayoutBatch, WorkerStats};

/// Pool-wide status.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolStatus {
    /// Hash rate the pool measured from accepted share weight.
    pub measured_hashrate: f64,
    /// Open mining channels.
    pub channels: usize,
    /// Rigs with retained statistics.
    pub workers: usize,
    /// Sum of rig-reported power draw. Unverifiable.
    pub reported_power_milliwatts: u64,
    /// Credits owed from confirmed blocks, across every miner.
    pub unpaid: u64,
    /// Credits from blocks that could still be orphaned.
    pub immature: u64,
    /// Payout batches not yet confirmed or failed.
    pub open_batches: usize,
    /// Job the pool is currently handing out, if any.
    pub current_job: Option<u32>,
    /// Height that job builds on.
    pub height: Option<u64>,
    /// PPLNS window, as a multiple of network difficulty.
    pub pplns_factor: f64,
    /// Operator fee, as a fraction.
    pub fee_rate: f64,
    /// Smallest balance the pool will pay.
    pub min_payout: u64,
    /// Value a found block distributes to miners, after the fee.
    ///
    /// Exposed because it is the number a miner needs in order to check the
    /// pool's arithmetic, and because on this chain it is operator policy
    /// rather than a consensus constant — there is no block reward to infer it
    /// from.
    pub reward_per_block: u64,
}

/// One miner's account with the pool.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MinerView {
    /// Address, hex.
    pub address: String,
    /// Balances, kept in three separate figures.
    pub balance: MinerBalance,
    /// The miner's rigs.
    pub workers: Vec<WorkerStats>,
    /// Hash rate measured across those rigs.
    pub measured_hashrate: f64,
}

/// A list of found blocks.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockList {
    /// Newest first.
    pub blocks: Vec<FoundBlock>,
}

/// A list of payout batches.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PayoutList {
    /// Newest first.
    pub batches: Vec<PayoutBatch>,
}

/// Parses an address from a path segment.
///
/// # Errors
///
/// Returns [`PoolError::BadRequest`] for anything that is not 32 hex-encoded
/// bytes. The message names the expected length, because the commonest mistake
/// is pasting a transaction id.
pub fn parse_address(value: &str) -> Result<Address> {
    let bytes =
        hex::decode(value).map_err(|_| PoolError::BadRequest(format!("{value:?} is not hex")))?;
    bytes.try_into().map_err(|_| {
        PoolError::BadRequest(format!(
            "an address is 32 bytes of hex; {value:?} decodes to a different length"
        ))
    })
}

/// Builds a miner's view from their balance and rigs.
#[must_use]
pub fn miner_view(address: Address, balance: MinerBalance, workers: Vec<WorkerStats>) -> MinerView {
    let measured_hashrate = workers.iter().map(|worker| worker.measured_hashrate).sum();
    MinerView {
        address: hex::encode(address),
        balance,
        workers,
        measured_hashrate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_round_trips_through_the_path() {
        let address: Address = [0x5A; 32];
        assert_eq!(parse_address(&hex::encode(address)).unwrap(), address);
    }

    #[test]
    fn a_transaction_id_pasted_as_an_address_says_what_was_wrong() {
        // The commonest mistake, and the error has to name the expected length
        // rather than only refusing.
        let error = parse_address(&"ab".repeat(40)).unwrap_err();
        assert!(error.to_string().contains("32 bytes"));
    }

    #[test]
    fn a_non_hex_address_is_refused() {
        assert!(matches!(
            parse_address("not-hex"),
            Err(PoolError::BadRequest(_))
        ));
    }

    #[test]
    fn a_miner_view_sums_only_measured_rates() {
        // The reported figures stay per-rig. Summing a claim into a headline
        // number is how a claim becomes a statistic.
        let workers = vec![
            WorkerStats {
                measured_hashrate: 1_000.0,
                reported_hashrate: 9_000_000.0,
                ..WorkerStats::default()
            },
            WorkerStats {
                measured_hashrate: 500.0,
                reported_hashrate: 9_000_000.0,
                ..WorkerStats::default()
            },
        ];

        let view = miner_view([1u8; 32], MinerBalance::default(), workers);
        assert_eq!(view.measured_hashrate, 1_500.0);
    }

    #[test]
    fn reported_fields_keep_their_prefix_on_the_wire() {
        let view = miner_view(
            [1u8; 32],
            MinerBalance::default(),
            vec![WorkerStats::default()],
        );
        let json = serde_json::to_string(&view).unwrap();

        assert!(json.contains("reported_hashrate"));
        assert!(json.contains("reported_power_milliwatts"));
        assert!(json.contains("measured_hashrate"));
    }

    #[test]
    fn balances_serialise_as_three_separate_figures() {
        let view = miner_view(
            [1u8; 32],
            MinerBalance {
                unpaid: 1,
                immature: 2,
                paid: 3,
            },
            Vec::new(),
        );
        let json = serde_json::to_value(&view).unwrap();

        assert_eq!(json["balance"]["unpaid"], 1);
        assert_eq!(json["balance"]["immature"], 2);
        assert_eq!(json["balance"]["paid"], 3);
    }
}
