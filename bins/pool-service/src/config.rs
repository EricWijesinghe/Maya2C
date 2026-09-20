//! Pool configuration, and the checks that run before anything binds.
//!
//! ## Why validation is aggressive here
//!
//! Most of these values are policy about other people's money. A pool that
//! starts with `fee_rate = 1.5` does not fail — it quietly takes 150% of every
//! reward and pays miners nothing, and the first report of the problem comes
//! from a miner rather than from the process. So every field with a meaningful
//! range is range-checked at load, and [`PoolConfig::validate`] refuses rather
//! than clamps.
//!
//! ## The treasury is the part with no default
//!
//! `docs/stratum-v2.md` §5 is explicit that this chain has no block reward:
//! `apply_block_checked` credits no subsidy and fees burn to `FEE_SINK`. PPLNS
//! therefore accrues *share weight*, and coin only enters at settlement, out of
//! an account the operator funds. There is no defensible default for whose
//! account that is or what a block is worth, so both are required and the
//! daemon refuses to start without them.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use custom_l1_node::state::Address;

use crate::error::{PoolError, Result};

/// Default interval the vardiff controller steers each channel toward.
///
/// `docs/stratum-v2.md` §4 does the arithmetic: at 50,000 connections, one
/// share per minute per channel is 21 cores of Argon2id and one share per
/// second is 1,270. This constant is the difference.
pub const DEFAULT_SHARE_INTERVAL: Duration = Duration::from_secs(60);

/// Default PPLNS window, as a multiple of network difficulty.
///
/// A window of exactly one difficulty is the same expected work as one block,
/// which makes a miner's payout as variable as solo mining and defeats the
/// point of pooling. Two is the usual starting point: it halves that variance
/// while still being short enough that a miner who leaves stops earning
/// promptly.
pub const DEFAULT_PPLNS_FACTOR: f64 = 2.0;

/// Default confirmations before credits from a found block become payable.
///
/// At a 15-second target this is fifteen minutes. Chosen against reorg depth
/// rather than wall time: paying out on a block that is later orphaned means
/// paying from the treasury for work that earned nothing.
pub const DEFAULT_CONFIRMATIONS: u64 = 60;

/// Largest number of outputs the payout builder puts in one transaction.
///
/// Far below the 65,536 ceiling `MAX_COLLECTION_LEN` imposes and far below the
/// 8 MiB gossip limit at 40 bytes an output. The binding consideration is
/// neither: a batch that fails takes every payout in it back to the queue, so
/// the batch size is a blast radius.
pub const DEFAULT_MAX_OUTPUTS_PER_BATCH: usize = 1_000;

/// How much of the reward the operator keeps, and the ceiling on it.
///
/// Not a policy judgement — 100% is simply where "pool fee" stops describing
/// the arrangement, and a fee above it would pay negative amounts.
const MAX_FEE_RATE: f64 = 1.0;

/// Where the vardiff controller may not go, in leading zero bits.
///
/// The floor keeps a fast rig from being handed a target so easy that its share
/// rate alone saturates the validator pool; the ceiling keeps a slow one from
/// being handed a target it will not hit in a day, which reads to the miner as
/// a broken pool.
const MIN_TARGET_BITS: u32 = 8;

/// See [`MIN_TARGET_BITS`].
///
/// The ceiling is 63 rather than something near 256 for a representational
/// reason: a share's weight is `2^bits` and is stored as a `u64`, so a target
/// above 63 bits would silently saturate and credit a miner less than they
/// earned. Window totals are accumulated in `U256` precisely because *those*
/// do overflow — see [`crate::pplns`]. A 2^63 share target is many orders of
/// magnitude beyond any per-share difficulty a real rig would be handed, so
/// nothing is lost by refusing to go there.
const MAX_TARGET_BITS: u32 = 63;

/// Everything the daemon needs to run.
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// Stratum listen address.
    pub stratum_addr: SocketAddr,
    /// Dashboard and JSON API listen address.
    pub api_addr: SocketAddr,
    /// Prometheus exporter address.
    ///
    /// A separate port, for the reason `src/metrics/mod.rs` gives: what the
    /// exporter serves is operational data, and here it also includes
    /// per-worker earnings. It belongs inside the pod network.
    pub metrics_addr: Option<SocketAddr>,
    /// Node JSON-RPC endpoint the pool pulls templates from and submits to.
    pub node_rpc: String,
    /// Directory holding the share ledger.
    pub data_dir: PathBuf,
    /// Encrypted treasury keystore.
    pub treasury_keystore: PathBuf,
    /// Value one found block distributes to miners, in base units.
    ///
    /// Required, because this chain mints no reward of its own. See the module
    /// documentation.
    pub reward_per_block: u64,
    /// Fraction of each reward the operator retains, in `[0, 1)`.
    pub fee_rate: f64,
    /// Operator's fee destination.
    pub fee_address: Address,
    /// PPLNS window as a multiple of network difficulty.
    pub pplns_factor: f64,
    /// Confirmations before credits from a block become payable.
    pub confirmations: u64,
    /// Smallest balance the payout engine will send, in base units.
    ///
    /// A payout below the cost of the transaction that carries it destroys
    /// value on the way to the miner.
    pub min_payout: u64,
    /// Largest number of outputs in one payout transaction.
    pub max_outputs_per_batch: usize,
    /// Ceiling on what one payout batch may move, in base units.
    ///
    /// A custody control, not an accounting one: the treasury key is hot on a
    /// network-facing daemon, and this bounds what a single compromised batch
    /// can carry.
    pub max_batch_value: u64,
    /// Share interval the vardiff controller steers toward.
    pub share_interval: Duration,
    /// Vardiff bounds, in leading zero bits.
    pub min_target_bits: u32,
    /// See [`PoolConfig::min_target_bits`].
    pub max_target_bits: u32,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            stratum_addr: "0.0.0.0:3333".parse().expect("literal address"),
            api_addr: "0.0.0.0:8080".parse().expect("literal address"),
            // Off unless asked for, matching `src/bin/node.rs`: the deployment
            // binds it to the pod network only.
            metrics_addr: None,
            node_rpc: "http://127.0.0.1:8545".to_string(),
            data_dir: PathBuf::from("pool-data"),
            treasury_keystore: PathBuf::from("treasury.key"),
            // Deliberately zero, and rejected by `validate`. A plausible-looking
            // default would be a policy invented by the code rather than chosen
            // by the operator.
            reward_per_block: 0,
            fee_rate: 0.01,
            fee_address: [0u8; 32],
            pplns_factor: DEFAULT_PPLNS_FACTOR,
            confirmations: DEFAULT_CONFIRMATIONS,
            min_payout: 1_000,
            max_outputs_per_batch: DEFAULT_MAX_OUTPUTS_PER_BATCH,
            max_batch_value: u64::MAX,
            share_interval: DEFAULT_SHARE_INTERVAL,
            min_target_bits: 16,
            max_target_bits: 48,
        }
    }
}

impl PoolConfig {
    /// Checks the configuration is coherent before anything binds.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::Config`] naming the first field that is unusable.
    pub fn validate(&self) -> Result<()> {
        let bad = |message: String| Err(PoolError::Config(message));

        if self.reward_per_block == 0 {
            return bad(
                "reward_per_block is unset. This chain mints no block reward \
                 (docs/stratum-v2.md §5), so the value a found block distributes \
                 is operator policy funded from the treasury, and there is no \
                 honest default for it"
                    .to_string(),
            );
        }

        if !self.fee_rate.is_finite() || self.fee_rate < 0.0 || self.fee_rate >= MAX_FEE_RATE {
            return bad(format!(
                "fee_rate {} is outside [0, {MAX_FEE_RATE})",
                self.fee_rate
            ));
        }

        if !self.pplns_factor.is_finite() || self.pplns_factor <= 0.0 {
            return bad(format!(
                "pplns_factor {} must be a positive, finite multiple of network \
                 difficulty",
                self.pplns_factor
            ));
        }

        if self.confirmations == 0 {
            return bad(
                "confirmations is zero, which pays out on a block that has not \
                 survived a single competing tip"
                    .to_string(),
            );
        }

        if self.max_outputs_per_batch == 0 {
            return bad("max_outputs_per_batch is zero, so no payout could be built".to_string());
        }

        if self.max_batch_value == 0 {
            return bad("max_batch_value is zero, so every batch would be refused".to_string());
        }

        if self.share_interval.is_zero() {
            return bad(
                "share_interval is zero: vardiff would drive every channel's \
                 target toward an unbounded share rate"
                    .to_string(),
            );
        }

        if self.min_target_bits < MIN_TARGET_BITS
            || self.max_target_bits > MAX_TARGET_BITS
            || self.min_target_bits >= self.max_target_bits
        {
            return bad(format!(
                "vardiff bounds {}..{} must be an increasing range inside \
                 {MIN_TARGET_BITS}..{MAX_TARGET_BITS}",
                self.min_target_bits, self.max_target_bits
            ));
        }

        Ok(())
    }

    /// The share of a reward that reaches miners, after the operator's fee.
    #[must_use]
    pub fn miner_share(&self, reward: u64) -> u64 {
        // The fee is taken off the top and the remainder is what PPLNS splits,
        // so the rounding direction matters: truncating the *fee* leaves the
        // spare unit with the miners, which is the direction to be wrong in.
        let fee = (reward as f64 * self.fee_rate) as u64;
        reward.saturating_sub(fee)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn valid() -> PoolConfig {
        PoolConfig {
            reward_per_block: 1_000_000,
            ..PoolConfig::default()
        }
    }

    #[test]
    fn the_default_configuration_refuses_to_start() {
        // The whole point of the zero default: an operator who sets nothing
        // gets an error naming the decision they have not made, rather than a
        // pool that runs and pays nothing.
        assert!(PoolConfig::default().validate().is_err());
    }

    #[test]
    fn a_funded_configuration_validates() {
        assert!(valid().validate().is_ok());
    }

    #[test]
    fn a_fee_rate_at_or_above_one_is_refused() {
        for rate in [1.0, 1.5, f64::NAN, -0.1] {
            let config = PoolConfig {
                fee_rate: rate,
                ..valid()
            };
            assert!(config.validate().is_err(), "fee_rate {rate} was accepted");
        }
    }

    #[test]
    fn inverted_vardiff_bounds_are_refused() {
        let config = PoolConfig {
            min_target_bits: 90,
            max_target_bits: 20,
            ..valid()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn zero_confirmations_are_refused() {
        let config = PoolConfig {
            confirmations: 0,
            ..valid()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn the_fee_rounds_in_the_miners_favour() {
        // 3 * 0.5 is 1.5; the fee truncates to 1 and the miners keep 2.
        let config = PoolConfig {
            fee_rate: 0.5,
            ..valid()
        };
        assert_eq!(config.miner_share(3), 2);
    }
}
