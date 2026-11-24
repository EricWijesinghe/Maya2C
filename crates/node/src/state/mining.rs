//! Mining rewards (ADR-044): the record, the schedule, and the share digest.
//!
//! Miners never order blocks; DAG-BFT does. A miner earns by putting a
//! [`MiningShare`](crate::core::mining_payload::MiningShare) batch on chain
//! whose DAG proofs of work meet the epoch's share target.
//!
//! Settlement is pro rata and pulled (MINE-01, ADR-044 amendment 2026-11-24):
//! a share credits work, never coins. When the epoch closes, its miner budget
//! is split by work, `⌊budget × mine / total⌋`, and each miner collects its
//! part with its next share or a claim. Nothing at the boundary iterates
//! miners, so closing an epoch costs the same for one miner as for any number,
//! and a share landing late in the epoch earns exactly what an early one does:
//! there is no first-come race for solo or distant miners to lose.
//!
//! Everything here is integer arithmetic over committed state: no floats, no
//! wall-clock (invariant 20).
//!
//! Coins are never minted. Genesis allocates the unissued supply to
//! [`MINING_RESERVE`], an address no key derives, and rewards are transfers out
//! of it, so the conservation equation (no native mint, ever) is unchanged.
//!
//! | Key | Holds |
//! |---|---|
//! | `k:mine` | the [`MiningRecord`] |
//! | `k:mseen:<digest>` | one spent share, so it cannot be paid twice |
//! | `k:mcredit:<miner>` | a miner's [`MinerCredit`]: shares in its last epoch |
//! | `k:mpot:<epoch>` | a closed epoch's [`EpochPot`], until every credit is collected |
//!
//! Both sit under the staking prefix, which is already a committed layer
//! (invariant 25).

use serde::{Deserialize, Serialize};

use maya_ledger_math::Amount;

use crate::consensus::uint::U256;
use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};
use crate::state::account::Address;

/// The mining record.
pub const MINING_KEY: &[u8] = b"k:mine";

/// Prefix of the spent-share set.
pub const SEEN_PREFIX: &[u8] = b"k:mseen:";

/// Prefix of miner credits, one per miner.
pub const CREDIT_PREFIX: &[u8] = b"k:mcredit:";

/// Prefix of closed epochs' pots.
pub const POT_PREFIX: &[u8] = b"k:mpot:";

// Both must stay inside the committed staking layer.
const _: () = assert!(MINING_KEY[0] == b'k' && MINING_KEY[1] == b':');
const _: () = assert!(SEEN_PREFIX[0] == b'k' && SEEN_PREFIX[1] == b':');
const _: () = assert!(CREDIT_PREFIX[0] == b'k' && CREDIT_PREFIX[1] == b':');
const _: () = assert!(POT_PREFIX[0] == b'k' && POT_PREFIX[1] == b':');

/// Holds the unissued supply. ASCII `maya2c/mining/reserve/v1`, zero-padded:
/// not a hash of any key pair, so no signature can spend from it.
pub const MINING_RESERVE: Address = [
    0x6d, 0x61, 0x79, 0x61, 0x32, 0x63, 0x2f, 0x6d, 0x69, 0x6e, 0x69, 0x6e, 0x67, 0x2f, 0x72, 0x65,
    0x73, 0x65, 0x72, 0x76, 0x65, 0x2f, 0x76, 0x31, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Basis points in a whole.
pub const BPS: u64 = 10_000;

/// Mining record format; see [`MiningRecord::encode`].
const RECORD_VERSION: u8 = 5;

/// The retarget moves at most this factor per epoch, either way.
pub const MAX_RETARGET_FACTOR: u64 = 4;

/// The most a schedule may ever pay, 2^192. Every amount below it times any
/// `u64` factor (a fee step, a share count, a basis point) still fits 256
/// bits, so no reward computation can overflow. `validate` enforces it on
/// every record, from genesis or decode (2026-10-28 decision).
pub const LIFETIME_EMISSION_CEILING: Amount = Amount([0, 0, 0, 1]);

/// Domain of the digest a share's proof of work commits to.
const SHARE_DOMAIN: &str = "maya2c mining share v1";

/// Which DAG parameters shares are hashed under. Consensus: every node must
/// agree, so it is fixed in genesis.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DagProfile {
    /// The 4 GiB dataset of `docs/dag-pow.md`.
    Mainnet,
    /// The 4 MiB test dataset, for devnets and tests.
    Testing,
}

impl DagProfile {
    const fn tag(self) -> u8 {
        match self {
            Self::Mainnet => 0,
            Self::Testing => 1,
        }
    }

    fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            0 => Ok(Self::Mainnet),
            1 => Ok(Self::Testing),
            _ => Err(NodeError::Decode(format!("mining dag profile {tag}"))),
        }
    }

    /// The DAG parameters this profile names.
    #[must_use]
    pub const fn params(self) -> crate::crypto::dag::Params {
        match self {
            Self::Mainnet => crate::crypto::dag::Params::MAINNET,
            Self::Testing => crate::crypto::dag::Params::TESTING,
        }
    }
}

/// Genesis mining parameters (`bft.mining`). Absent: a fees-only chain.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MiningGenesis {
    /// Units issued in epoch 0, before any halving: 256-bit (ADR-046), a JSON
    /// number or a decimal string.
    #[serde(with = "crate::state::amount_serde::lenient")]
    pub initial_epoch_reward: primitive_types::U256,
    /// Epochs between halvings.
    pub halving_epochs: u64,
    /// The floor the halvings stop at, paid every epoch until the last height
    /// a `u64` reaches: mining never ends (ADR-046 decision 4). Zero, or
    /// absent: the reward halves to nothing, as before.
    #[serde(default, with = "crate::state::amount_serde::lenient")]
    pub tail_epoch_reward: primitive_types::U256,
    /// Miners' part of each epoch's reward; the rest goes to the committee.
    pub miner_share_bps: u64,
    /// Shares per epoch the retarget steers toward.
    pub target_shares: u64,
    /// Most shares one block may carry.
    pub max_shares_per_block: u64,
    /// Share target in force for epoch 0, big-endian hex.
    pub initial_target: String,
    /// DAG parameters shares are hashed under.
    pub dag: DagProfile,
}

impl MiningGenesis {
    /// The epoch-0 record.
    ///
    /// # Errors
    ///
    /// A zero halving interval, target or cap, a miner share above 100%, or a
    /// malformed target.
    pub fn record(&self, epoch_blocks: u64) -> Result<MiningRecord> {
        let bytes = hex::decode(&self.initial_target)
            .map_err(|e| NodeError::Decode(format!("bft.mining.initial_target: {e}")))?;
        let target: [u8; 32] = bytes
            .try_into()
            .map_err(|_| NodeError::Decode("bft.mining.initial_target: not 32 bytes".into()))?;
        let record = MiningRecord {
            initial_epoch_reward: maya_stateless_core::amount::to_amount(self.initial_epoch_reward),
            halving_epochs: self.halving_epochs,
            tail_epoch_reward: maya_stateless_core::amount::to_amount(self.tail_epoch_reward),
            miner_share_bps: self.miner_share_bps,
            target_shares: self.target_shares,
            max_shares_per_block: self.max_shares_per_block,
            dag: self.dag,
            epoch_blocks,
            epoch: 0,
            anchor: [0; 32],
            target,
            shares: 0,
            miners: 0,
            target_limit: target,
        };
        record.validate()?;
        Ok(record)
    }
}

/// The mining state: fixed parameters plus the current epoch's progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MiningRecord {
    /// As [`MiningGenesis::initial_epoch_reward`].
    pub initial_epoch_reward: Amount,
    /// As [`MiningGenesis::halving_epochs`].
    pub halving_epochs: u64,
    /// As [`MiningGenesis::tail_epoch_reward`].
    pub tail_epoch_reward: Amount,
    /// As [`MiningGenesis::miner_share_bps`].
    pub miner_share_bps: u64,
    /// As [`MiningGenesis::target_shares`].
    pub target_shares: u64,
    /// As [`MiningGenesis::max_shares_per_block`].
    pub max_shares_per_block: u64,
    /// As [`MiningGenesis::dag`].
    pub dag: DagProfile,
    /// The staking epoch length; mining epochs are the same epochs.
    pub epoch_blocks: u64,
    /// The epoch shares are accepted for.
    pub epoch: u64,
    /// Id of the parent of the block that opened `epoch` (its `prev_hash`);
    /// zero in epoch 0. Work cannot be done before that block is final. Never
    /// the opening block's own id: that id covers the state root this record
    /// is part of, which no header can commit to (#642).
    pub anchor: [u8; 32],
    /// The share target, big-endian. A digest at or below it is a share.
    pub target: [u8; 32],
    /// Shares accepted in `epoch`. Every share lands while the block cap has
    /// room, so this is the epoch's demand, which the retarget reads.
    pub shares: u64,
    /// Miners credited in `epoch`: how many credits its pot will wait for.
    pub miners: u64,
    /// The easiest target retargeting may reach: the genesis target. Without
    /// a floor, empty epochs would ease the target until every digest is a
    /// share (security review of ADR-044).
    pub target_limit: [u8; 32],
}

impl MiningRecord {
    /// The whole reward of `epoch`: the initial reward halved once per
    /// `halving_epochs`, never below the tail. With no tail it reaches zero
    /// once every bit has shifted out (at most 256 halvings, ADR-046).
    #[must_use]
    pub fn epoch_reward(&self, epoch: u64) -> Amount {
        let halvings = u32::try_from(epoch / self.halving_epochs).unwrap_or(u32::MAX);
        self.initial_epoch_reward
            .shr(halvings)
            .max(self.tail_epoch_reward)
    }

    /// An upper bound on everything this schedule pays, from epoch 0 to the
    /// last epoch a `u64` height reaches. A reserve funded with it never runs
    /// dry, so the tail is paid for as long as the chain can count blocks.
    /// `None` at or past [`LIFETIME_EMISSION_CEILING`].
    #[must_use]
    pub fn lifetime_emission(&self) -> Option<Amount> {
        // u128 counts: with one-block epochs there are 2^64 of them.
        let epochs_total = u128::from(u64::MAX / self.epoch_blocks) + 1;
        let era = u128::from(self.halving_epochs);
        let mut total = Amount::ZERO;
        let mut start: u128 = 0;
        for halvings in 0u32..=256 {
            if start >= epochs_total {
                break;
            }
            let halved = self.initial_epoch_reward.shr(halvings);
            let left = epochs_total - start;
            // From the first era at or under the tail, every epoch pays the tail.
            let (reward, epochs) = if halved <= self.tail_epoch_reward {
                (self.tail_epoch_reward, left)
            } else {
                (halved, era.min(left))
            };
            total = total.checked_add(reward.checked_mul(Amount::from_u128(epochs))?)?;
            if halved <= self.tail_epoch_reward {
                break;
            }
            start = start.saturating_add(era);
        }
        (total < LIFETIME_EMISSION_CEILING).then_some(total)
    }

    /// Miners' part of `epoch`'s reward.
    #[must_use]
    pub fn miner_budget(&self, epoch: u64) -> Amount {
        bps_of(self.epoch_reward(epoch), self.miner_share_bps)
    }

    /// What one share earns if the epoch closes at `target_shares`: an
    /// estimate for miners and RPC, never a consensus value. What a share is
    /// paid is its pro-rata part of the budget, [`share_payout`].
    #[must_use]
    pub fn share_reward(&self) -> Amount {
        // `validate` keeps `target_shares` positive.
        self.miner_budget(self.epoch)
            .checked_div(Amount::from_u64(self.target_shares))
            .unwrap_or(Amount::ZERO)
    }

    /// The committee's part of `epoch`'s reward.
    #[must_use]
    pub fn validator_reward(&self, epoch: u64) -> Amount {
        // The miner budget is a share of at most 100% of the reward.
        self.epoch_reward(epoch)
            .saturating_sub(self.miner_budget(epoch))
    }

    /// The next epoch's share target: more shares than wanted makes shares
    /// harder. Clamped to a factor of [`MAX_RETARGET_FACTOR`] either way, never
    /// easier than [`Self::target_limit`], and never zero.
    #[must_use]
    pub fn retarget(&self) -> [u8; 32] {
        let low = (self.target_shares / MAX_RETARGET_FACTOR).max(1);
        let high = self.target_shares.saturating_mul(MAX_RETARGET_FACTOR);
        let observed = self.demand().clamp(low, high);
        let previous = U256::from_be_bytes(&self.target);
        // Overflow means past 256 bits, easier than any limit: saturate to the
        // limit rather than wrap to a hard target.
        let limit = U256::from_be_bytes(&self.target_limit);
        let next = previous
            .mul_div_u64(self.target_shares, observed)
            .unwrap_or(limit);
        if next > limit {
            limit.to_be_bytes()
        } else if next.is_zero() {
            U256::ONE.to_be_bytes()
        } else {
            next.to_be_bytes()
        }
    }

    /// The epoch's demand: its share count. No budget refuses a share any
    /// more, so the count is no longer cut off where the budget ran out.
    fn demand(&self) -> u64 {
        self.shares
    }

    /// Fixed-width encoding, version 5: pro-rata settlement replaces `paid`
    /// and `spent_after` with `miners`. Version 4 added the tail reward,
    /// version 3 had no tail, version 2 held amounts as `u64` and version 1
    /// lacked `spent_after`; none was on a public chain with mining active.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + 64 + 8 * 8 + 32 + 1 + 64);
        out.push(RECORD_VERSION);
        out.extend_from_slice(&self.initial_epoch_reward.to_le_bytes());
        out.extend_from_slice(&self.tail_epoch_reward.to_le_bytes());
        for value in [
            self.halving_epochs,
            self.miner_share_bps,
            self.target_shares,
            self.max_shares_per_block,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.push(self.dag.tag());
        for value in [self.epoch_blocks, self.epoch] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&self.anchor);
        out.extend_from_slice(&self.target);
        out.extend_from_slice(&self.shares.to_le_bytes());
        out.extend_from_slice(&self.miners.to_le_bytes());
        out.extend_from_slice(&self.target_limit);
        out
    }

    /// Decodes [`Self::encode`].
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] on a wrong version, profile or length.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        if r.read_u8()? != RECORD_VERSION {
            return Err(NodeError::Decode("mining record version".into()));
        }
        let record = Self {
            initial_epoch_reward: Amount::from_le_bytes(r.read_array()?),
            tail_epoch_reward: Amount::from_le_bytes(r.read_array()?),
            halving_epochs: r.read_u64()?,
            miner_share_bps: r.read_u64()?,
            target_shares: r.read_u64()?,
            max_shares_per_block: r.read_u64()?,
            dag: DagProfile::from_tag(r.read_u8()?)?,
            epoch_blocks: r.read_u64()?,
            epoch: r.read_u64()?,
            anchor: r.read_array()?,
            target: r.read_array()?,
            shares: r.read_u64()?,
            miners: r.read_u64()?,
            target_limit: r.read_array()?,
        };
        r.finish()?;
        record.validate()?;
        Ok(record)
    }

    /// The parameter rules, in one place for genesis and for decode, so a
    /// record that genesis would refuse cannot arrive another way (a snapshot,
    /// a corrupted store).
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] naming the rule broken.
    pub fn validate(&self) -> Result<()> {
        let bad = |what: &str| Err(NodeError::Decode(format!("mining: {what}")));
        if self.halving_epochs == 0 || self.target_shares == 0 || self.epoch_blocks == 0 {
            return bad("halving_epochs, target_shares and epoch_blocks must be positive");
        }
        if self.max_shares_per_block == 0 {
            return bad("max_shares_per_block must be positive");
        }
        if self.miner_share_bps > BPS {
            return bad("miner_share_bps above 10000");
        }
        if self.tail_epoch_reward > self.initial_epoch_reward {
            return bad("tail_epoch_reward above initial_epoch_reward");
        }
        // Here, not only at genesis, so a snapshot or a corrupted store cannot
        // carry a schedule whose arithmetic could overflow.
        if self.lifetime_emission().is_none() {
            return bad("lifetime emission at or above 2^192");
        }
        if U256::from_be_bytes(&self.target) > U256::from_be_bytes(&self.target_limit) {
            return bad("target easier than its limit");
        }
        if U256::from_be_bytes(&self.target).is_zero() {
            return bad("target is zero");
        }
        // A share that rounds to nothing would have miners working for free
        // from block 1.
        let per_share = self
            .miner_budget(0)
            .checked_div(Amount::from_u64(self.target_shares))
            .unwrap_or(Amount::ZERO);
        if self.miner_share_bps > 0 && per_share.is_zero() {
            return bad("a share in epoch 0 would pay nothing");
        }
        // And forever after (pre-audit S-027): every epoch pays at least the
        // tail, so a tail whose share rounds to nothing would stop paying
        // miners in the tail era while validators were still paid.
        let tail_share = bps_of(self.tail_epoch_reward, self.miner_share_bps)
            .checked_div(Amount::from_u64(self.target_shares))
            .unwrap_or(Amount::ZERO);
        if self.miner_share_bps > 0 && !self.tail_epoch_reward.is_zero() && tail_share.is_zero() {
            return bad("a share in the tail era would pay nothing");
        }
        Ok(())
    }
}

// Documentation only: a schedule value never exceeds the ceiling 2^192, so a
// schedule's `value × bps` would even fit the naive 256-bit product. `bps_of`
// does NOT depend on this — it is exact for every `Amount`, with no bound
// (256-bit rule, DECISIONS.md 2026-11-01 and 2026-11-08).
const _: () = assert!(
    LIFETIME_EMISSION_CEILING
        .checked_mul(Amount::from_u64(BPS))
        .is_some()
);

/// `floor(value × bps / 10000)`, exact for **every** `Amount` and `bps`, with
/// no 512-bit intermediate and no overflow. Writing `value = q·BPS + r`
/// (`0 ≤ r < BPS`), `floor(value·bps/BPS) = q·bps + floor(r·bps/BPS)`: `q·bps`
/// stays ≤ `value` so it cannot overflow, and `r·bps < BPS² < 2^64`. Equal to
/// the old 512-bit `mul_div` for every input (consensus: any difference forks).
#[must_use]
pub fn bps_of(value: Amount, bps: u64) -> Amount {
    let bps = bps.min(BPS);
    let denom = Amount::from_u64(BPS);
    // `BPS != 0`, so the division is defined.
    let q = value.checked_div(denom).expect("BPS is non-zero");
    // `q·BPS ≤ value ≤ Amount::MAX`, so neither this product nor the subtraction
    // can fail, and the remainder `r = value − q·BPS` is below `BPS < 2^64`.
    let q_bps_full = q.checked_mul(denom).expect("q·BPS ≤ value ≤ Amount::MAX");
    let r = value
        .checked_sub(q_bps_full)
        .expect("q·BPS ≤ value")
        .to_u64()
        .expect("value mod BPS < BPS < 2^64");
    // `q·bps ≤ q·BPS ≤ value`, so no overflow; `r·bps < BPS² < 2^64`.
    let whole = q.checked_mul(Amount::from_u64(bps)).expect("q·bps ≤ value");
    whole
        .checked_add(Amount::from_u64(r * bps / BPS))
        .expect("floor(value·bps/BPS) ≤ value ≤ Amount::MAX")
}

/// The header hash a share's DAG proof of work is computed over. Binds the
/// chain, the epoch, the epoch's anchor and the payee, so a share can be
/// neither precomputed, replayed elsewhere, nor stolen.
#[must_use]
pub fn share_header(
    chain_tag: &[u8; 32],
    epoch: u64,
    anchor: &[u8; 32],
    miner: &Address,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(SHARE_DOMAIN);
    hasher.update(chain_tag);
    hasher.update(&epoch.to_le_bytes());
    hasher.update(anchor);
    hasher.update(miner);
    *hasher.finalize().as_bytes()
}

/// Prefix of one epoch's spent shares. The epoch leads the key so the whole
/// set is deleted when the epoch closes: a share from a closed epoch is
/// refused by the epoch check, so its record has nothing left to guard.
#[must_use]
pub fn seen_epoch_prefix(epoch: u64) -> Vec<u8> {
    let mut key = SEEN_PREFIX.to_vec();
    key.extend_from_slice(&epoch.to_be_bytes());
    key
}

/// Key of one spent share of `epoch`.
#[must_use]
pub fn seen_key(epoch: u64, header: &[u8; 32], nonce: u64) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new_derive_key("maya2c mining seen v1");
    hasher.update(header);
    hasher.update(&nonce.to_le_bytes());
    let mut key = seen_epoch_prefix(epoch);
    key.extend_from_slice(hasher.finalize().as_bytes());
    key
}

/// MINE-4: what `mine` shares of an epoch's `total` earn of its miner
/// `budget`, `⌊budget × mine / total⌋`; zero when nothing was mined. Exact in
/// 256 bits: the budget is below 2^192 ([`LIFETIME_EMISSION_CEILING`]) and
/// `mine` below 2^64, so the product cannot overflow. The floors sum to at
/// most the budget; the dust stays in the reserve.
#[must_use]
pub fn share_payout(budget: Amount, mine: u64, total: u64) -> Amount {
    budget
        .checked_mul(Amount::from_u64(mine))
        .and_then(|product| product.checked_div(Amount::from_u64(total)))
        .unwrap_or(Amount::ZERO)
}

/// A miner's work in the last epoch it mined. One per miner, whatever the
/// number of epochs: a new epoch's first share settles the older credit
/// before it credits the new work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MinerCredit {
    /// The epoch the shares were for.
    pub epoch: u64,
    /// Shares credited in it.
    pub shares: u64,
}

impl MinerCredit {
    /// `epoch ‖ shares`, both `u64` little-endian.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        [self.epoch.to_le_bytes(), self.shares.to_le_bytes()].concat()
    }

    /// Decodes [`Self::encode`].
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] on a wrong length.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        let credit = Self {
            epoch: r.read_u64()?,
            shares: r.read_u64()?,
        };
        r.finish()?;
        Ok(credit)
    }
}

/// A closed epoch's miner budget and the work it is split by. Written once at
/// the boundary, whatever the number of miners, and deleted when the last
/// credit is collected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EpochPot {
    /// The epoch's miner budget (MINE-2).
    pub budget: Amount,
    /// Shares accepted in the epoch.
    pub shares: u64,
    /// Credits not yet collected.
    pub miners: u64,
}

impl EpochPot {
    /// `budget (32 bytes LE) ‖ shares ‖ miners`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.budget.to_le_bytes().to_vec();
        out.extend_from_slice(&self.shares.to_le_bytes());
        out.extend_from_slice(&self.miners.to_le_bytes());
        out
    }

    /// Decodes [`Self::encode`].
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] on a wrong length.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        let pot = Self {
            budget: Amount::from_le_bytes(r.read_array()?),
            shares: r.read_u64()?,
            miners: r.read_u64()?,
        };
        r.finish()?;
        Ok(pot)
    }
}

/// Key of `miner`'s credit.
#[must_use]
pub fn credit_key(miner: &Address) -> Vec<u8> {
    [CREDIT_PREFIX, miner.as_slice()].concat()
}

/// Key of `epoch`'s pot. Big-endian, so pots sort by epoch.
#[must_use]
pub fn pot_key(epoch: u64) -> Vec<u8> {
    [POT_PREFIX, epoch.to_be_bytes().as_slice()].concat()
}

#[cfg(test)]
mod tests {
    // Test scope: unwrapping is how a test reports a failure (Cargo.toml lint
    // policy). Without this the lib-test target fails `lint_debt.sh`.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn record() -> MiningRecord {
        MiningGenesis {
            initial_epoch_reward: primitive_types::U256::from(1_000_000),
            halving_epochs: 10,
            tail_epoch_reward: primitive_types::U256::zero(),
            miner_share_bps: 7_000,
            target_shares: 100,
            max_shares_per_block: 4,
            initial_target: hex::encode([0x0f; 32]),
            dag: DagProfile::Testing,
        }
        .record(600)
        .unwrap()
    }

    #[test]
    fn the_reward_halves_on_schedule_and_ends() {
        let r = record();
        assert_eq!(
            r.epoch_reward(0),
            maya_ledger_math::Amount::from_u64(1_000_000)
        );
        assert_eq!(
            r.epoch_reward(9),
            maya_ledger_math::Amount::from_u64(1_000_000)
        );
        assert_eq!(
            r.epoch_reward(10),
            maya_ledger_math::Amount::from_u64(500_000)
        );
        assert_eq!(r.epoch_reward(640), maya_ledger_math::Amount::from_u64(0));
        assert_eq!(
            r.epoch_reward(u64::MAX),
            maya_ledger_math::Amount::from_u64(0)
        );
    }

    #[test]
    fn miner_and_validator_parts_sum_to_the_reward() {
        let r = record();
        for epoch in [0, 10, 33, 200] {
            assert_eq!(
                r.miner_budget(epoch).checked_add(r.validator_reward(epoch)),
                Some(r.epoch_reward(epoch))
            );
        }
        assert_eq!(
            r.miner_budget(0),
            maya_ledger_math::Amount::from_u64(700_000)
        );
    }

    #[test]
    fn share_reward_is_the_budget_over_target_shares() {
        let mut r = record();
        assert_eq!(r.share_reward(), maya_ledger_math::Amount::from_u64(7_000));
        // An estimate only: many shares landing changes nothing about it.
        r.shares = 10_000;
        assert_eq!(r.share_reward(), maya_ledger_math::Amount::from_u64(7_000));
    }

    /// MINE-01: a share's pay is its part of the work, not its place in line.
    #[test]
    fn a_payout_is_the_floor_of_budget_times_work_over_total() {
        let budget = maya_ledger_math::Amount::from_u64(700_000);
        assert_eq!(
            share_payout(budget, 1, 3),
            maya_ledger_math::Amount::from_u64(233_333)
        );
        assert_eq!(share_payout(budget, 3, 3), budget);
        assert_eq!(share_payout(budget, 0, 3), maya_ledger_math::Amount::ZERO);
        assert_eq!(share_payout(budget, 1, 0), maya_ledger_math::Amount::ZERO);
        // The largest budget a schedule may hold, times the largest count.
        let top = LIFETIME_EMISSION_CEILING.saturating_sub(maya_ledger_math::Amount::from_u64(1));
        assert_eq!(share_payout(top, u64::MAX, u64::MAX), top);
        // The floors never sum past the budget.
        let parts: Vec<_> = [1u64, 2, 4]
            .iter()
            .map(|&m| share_payout(budget, m, 7))
            .collect();
        let sum = parts
            .iter()
            .fold(maya_ledger_math::Amount::ZERO, |a, &p| a.saturating_add(p));
        assert!(sum <= budget);
    }

    #[test]
    fn credits_and_pots_round_trip_and_reject_corruption() {
        let credit = MinerCredit {
            epoch: 7,
            shares: 3,
        };
        assert_eq!(MinerCredit::decode(&credit.encode()).unwrap(), credit);
        assert!(MinerCredit::decode(&credit.encode()[..15]).is_err());
        let pot = EpochPot {
            budget: maya_ledger_math::Amount::from_u64(700_000),
            shares: 9,
            miners: 2,
        };
        assert_eq!(EpochPot::decode(&pot.encode()).unwrap(), pot);
        let mut long = pot.encode();
        long.push(0);
        assert!(EpochPot::decode(&long).is_err());
        assert!(credit_key(&[1; 32]).starts_with(b"k:mcredit:"));
        assert!(pot_key(1) < pot_key(256), "pots sort by epoch");
    }

    #[test]
    fn retarget_hardens_when_shares_overshoot_and_is_clamped() {
        let mut r = record();
        let base = U256::from_be_bytes(&r.target);
        r.shares = 200;
        assert!(U256::from_be_bytes(&r.retarget()) < base);
        r.shares = 1_000_000;
        let clamped = U256::from_be_bytes(&r.retarget());
        assert_eq!(clamped, base.mul_div_u64(1, 4).unwrap());
        // Easing is clamped at ×4, and never past the genesis target.
        let sixteenth = base.mul_div_u64(1, 16).unwrap();
        r.target = sixteenth.to_be_bytes();
        r.shares = 0;
        assert_eq!(
            U256::from_be_bytes(&r.retarget()),
            sixteenth.mul_div_u64(4, 1).unwrap()
        );
        r.target = base.mul_div_u64(1, 2).unwrap().to_be_bytes();
        assert_eq!(r.retarget(), r.target_limit);
    }

    #[test]
    fn empty_epochs_never_ease_past_the_genesis_target() {
        let mut r = record();
        for _ in 0..100 {
            r.target = r.retarget();
            assert!(U256::from_be_bytes(&r.target) <= U256::from_be_bytes(&r.target_limit));
        }
        assert_eq!(r.target, r.target_limit);
        // Overflow from a near-max limit saturates to the limit, not to zero.
        r.target_limit = [0xff; 32];
        r.target = [0xff; 32];
        assert_eq!(r.retarget(), [0xff; 32]);
    }

    #[test]
    fn record_round_trips_and_rejects_corruption() {
        let mut r = record();
        r.epoch = 7;
        r.anchor = [9; 32];
        r.shares = 3;
        r.miners = 2;
        assert_eq!(MiningRecord::decode(&r.encode()).unwrap(), r);
        let mut bytes = r.encode();
        bytes.push(0);
        assert!(MiningRecord::decode(&bytes).is_err());
        bytes = r.encode();
        bytes[0] = 1;
        assert!(MiningRecord::decode(&bytes).is_err());
    }

    fn with_tail(tail: u64, epoch_blocks: u64) -> MiningRecord {
        MiningRecord {
            tail_epoch_reward: Amount::from_u64(tail),
            epoch_blocks,
            ..record()
        }
    }

    #[test]
    fn a_tail_whose_share_rounds_to_nothing_is_refused() {
        // Pre-audit S-027: epoch 0 pays, but the tail split over the shares
        // would not.
        let mut r = with_tail(1, 600);
        r.miner_share_bps = 5_000;
        r.target_shares = 1_000;
        assert!(
            r.validate().is_err(),
            "a tail paying zero per share was accepted"
        );
        r.tail_epoch_reward = Amount::from_u64(2_000);
        assert!(r.validate().is_ok());
        r.tail_epoch_reward = Amount::ZERO;
        assert!(r.validate().is_ok(), "no tail at all stays allowed");
    }

    #[test]
    fn the_reward_halves_down_to_the_tail_and_stays_there_forever() {
        let r = with_tail(100_000, 600);
        assert_eq!(r.epoch_reward(10), Amount::from_u64(500_000));
        // 1_000_000 >> 4 = 62_500 is under the tail.
        assert_eq!(r.epoch_reward(40), Amount::from_u64(100_000));
        assert_eq!(r.epoch_reward(u64::MAX), Amount::from_u64(100_000));
    }

    #[test]
    fn lifetime_emission_is_the_sum_of_every_epoch() {
        // ~50 epochs before a u64 height runs out: small enough to add up one
        // by one and compare.
        for tail in [0, 100_000] {
            let r = with_tail(tail, u64::MAX / 50);
            let last = u64::MAX / r.epoch_blocks;
            let brute = (0..=last).fold(Amount::ZERO, |sum, e| {
                sum.checked_add(r.epoch_reward(e)).unwrap()
            });
            assert_eq!(r.lifetime_emission(), Some(brute), "tail {tail}");
        }
    }

    /// `deploy/genesis/mainnet/mining.json`, copied: the test does not read
    /// `deploy/`, which a published crate could not reach.
    fn mainnet_schedule() -> MiningGenesis {
        MiningGenesis {
            initial_epoch_reward: primitive_types::U256::from_dec_str(
                "299452429842573579739904175",
            )
            .unwrap(),
            halving_epochs: 35_064,
            tail_epoch_reward: primitive_types::U256::from_dec_str("23956194387405886379192334")
                .unwrap(),
            miner_share_bps: 7_000,
            target_shares: 3_600,
            max_shares_per_block: 16,
            initial_target: "0000ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                .into(),
            dag: DagProfile::Mainnet,
        }
    }

    #[test]
    fn the_mainnet_halving_era_pays_19_687_500_coins_then_the_tail_from_epoch_140256() {
        // Pins the figures the docs quote: 1.875 eras of 35,064 epochs at
        // 299.45 coins, one-hour epochs at one block a second. Measured by
        // summing the schedule epoch by epoch; the value is below 19,687,500
        // coins by the truncation of each halving (`shr`), which is exact.
        let r = mainnet_schedule().record(3_600).unwrap();
        let mut halving_era = Amount::ZERO;
        for epoch in 0..4 * 35_064 {
            halving_era = halving_era.checked_add(r.epoch_reward(epoch)).unwrap();
        }
        assert_eq!(halving_era.to_string(), "19687499999999999999999999910864");
        // Epoch 140255 is the last halved one (37.43 coins); 140256 is the first at the tail.
        assert_eq!(
            r.epoch_reward(140_255).to_string(),
            "37431553730321697467488021"
        );
        assert_eq!(
            r.epoch_reward(140_256).to_string(),
            "23956194387405886379192334"
        );
    }

    #[test]
    fn the_mainnet_reserve_is_the_lifetime_emission_at_3600_block_epochs() {
        // The 1.23e17-coin reserve in deploy/genesis/mainnet/README.md.
        let r = mainnet_schedule().record(3_600).unwrap();
        assert_eq!(
            r.lifetime_emission().unwrap().to_string(),
            "122753829695359067419575633036805965021648"
        );
    }

    #[test]
    fn a_schedule_past_the_ceiling_has_no_lifetime_emission() {
        // 2^130 per epoch over 2^64 one-block epochs is 2^194 > 2^192.
        let huge = Amount([0, 0, 4, 0]);
        let r = MiningRecord {
            initial_epoch_reward: huge,
            tail_epoch_reward: huge,
            epoch_blocks: 1,
            ..record()
        };
        assert_eq!(r.lifetime_emission(), None);
        // Under it, with one-block epochs (2^64 of them), it is exact.
        let r = MiningRecord {
            initial_epoch_reward: Amount::from_u64(1 << 40),
            tail_epoch_reward: Amount::from_u64(1 << 40),
            epoch_blocks: 1,
            ..record()
        };
        assert_eq!(r.lifetime_emission(), Some(Amount([0, 1 << 40, 0, 0])));
        // The ceiling is exclusive: 2^128 per epoch over 2^64 epochs is
        // exactly 2^192 and is refused; one unit less per epoch is not.
        let at = |reward: Amount| MiningRecord {
            initial_epoch_reward: reward,
            tail_epoch_reward: reward,
            epoch_blocks: 1,
            ..record()
        };
        assert_eq!(at(Amount([0, 0, 1, 0])).lifetime_emission(), None);
        assert!(
            at(Amount([u64::MAX, u64::MAX, 0, 0]))
                .lifetime_emission()
                .is_some()
        );
        assert!(MiningRecord::decode(&at(Amount([0, 0, 1, 0])).encode()).is_err());
    }

    #[test]
    fn an_era_cut_short_by_the_last_height_is_counted_to_the_epoch() {
        // ~50 epochs in all, eras of 1000: the first era never finishes.
        for tail in [0, 1] {
            let r = MiningRecord {
                halving_epochs: 1_000,
                ..with_tail(tail, u64::MAX / 50)
            };
            let epochs = u64::MAX / r.epoch_blocks + 1;
            assert_eq!(
                r.lifetime_emission(),
                r.initial_epoch_reward.checked_mul(Amount::from_u64(epochs))
            );
        }
    }

    #[test]
    fn a_tail_above_the_initial_reward_is_refused_on_decode() {
        let mut r = record();
        r.tail_epoch_reward = r
            .initial_epoch_reward
            .checked_add(Amount::from_u64(1))
            .unwrap();
        assert!(MiningRecord::decode(&r.encode()).is_err());
        // A tail that still pays every share (S-027): 1,000 × 70% / 100 shares.
        assert!(MiningRecord::decode(&with_tail(1_000, 600).encode()).is_ok());
        r.tail_epoch_reward = r.initial_epoch_reward;
        assert!(MiningRecord::decode(&r.encode()).is_ok());
    }

    #[test]
    fn genesis_refuses_bad_parameters() {
        let good = MiningGenesis {
            initial_epoch_reward: primitive_types::U256::from(1_000),
            halving_epochs: 1,
            tail_epoch_reward: primitive_types::U256::zero(),
            miner_share_bps: 7_000,
            target_shares: 1,
            max_shares_per_block: 1,
            initial_target: hex::encode([1; 32]),
            dag: DagProfile::Testing,
        };
        assert!(good.record(10).is_ok());
        // 70% of 1 unit split over one share rounds to nothing.
        assert!(
            MiningGenesis {
                initial_epoch_reward: primitive_types::U256::from(1),
                ..good.clone()
            }
            .record(10)
            .is_err()
        );
        assert!(good.record(0).is_err());
        assert!(
            MiningGenesis {
                halving_epochs: 0,
                ..good.clone()
            }
            .record(10)
            .is_err()
        );
        assert!(
            MiningGenesis {
                miner_share_bps: 10_001,
                ..good.clone()
            }
            .record(10)
            .is_err()
        );
        assert!(
            MiningGenesis {
                initial_target: hex::encode([0; 32]),
                ..good.clone()
            }
            .record(10)
            .is_err()
        );
        assert!(
            MiningGenesis {
                initial_target: "ab".into(),
                ..good
            }
            .record(10)
            .is_err()
        );
    }

    #[test]
    fn a_share_header_binds_every_field() {
        let base = share_header(&[1; 32], 5, &[2; 32], &[3; 32]);
        assert_ne!(base, share_header(&[9; 32], 5, &[2; 32], &[3; 32]));
        assert_ne!(base, share_header(&[1; 32], 6, &[2; 32], &[3; 32]));
        assert_ne!(base, share_header(&[1; 32], 5, &[9; 32], &[3; 32]));
        assert_ne!(base, share_header(&[1; 32], 5, &[2; 32], &[9; 32]));
        assert_ne!(seen_key(5, &base, 1), seen_key(5, &base, 2));
        assert!(seen_key(5, &base, 1).starts_with(&seen_epoch_prefix(5)));
    }

    /// The 2026-11-08 switch to 256-bit math is a consensus change only if
    /// an output moved: compare against the old 512-bit `mul_div` at the
    /// edges and over a seeded sweep of values up to the 2^192 ceiling.
    #[test]
    fn bps_of_matches_the_512_bit_split_for_every_in_bound_value() {
        let old = |v: Amount, bps: u64| {
            v.mul_div(Amount::from_u64(bps), Amount::from_u64(BPS))
                .unwrap()
        };
        let ceiling = LIFETIME_EMISSION_CEILING;
        let mut values = vec![
            Amount::ZERO,
            Amount::from_u64(1),
            Amount::from_u64(BPS - 1),
            Amount::from_u64(BPS),
            ceiling,
            ceiling.checked_sub(Amount::from_u64(1)).unwrap(),
        ];
        // xorshift64, fixed seed: reproducible, no dependency.
        let mut s: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        for _ in 0..20_000 {
            // A random bit length in 0..=192 so small values are covered too.
            let bits = u32::try_from(next() % 193).unwrap();
            let v = Amount([next(), next(), next(), 0]).shr(192 - bits);
            values.push(v);
        }
        let mut bps_set = vec![0, 1, 2, 5_000, 7_000, 9_999, BPS];
        bps_set.extend((0..64).map(|_| next() % (BPS + 1)));
        for &v in &values {
            assert!(v <= ceiling);
            for &bps in &bps_set {
                assert_eq!(bps_of(v, bps), old(v, bps), "value {v} bps {bps}");
            }
        }
        // Above BPS is clamped, as before.
        assert_eq!(bps_of(ceiling, u64::MAX), ceiling);
    }

    /// The split-remainder form is exact with no bound: it must stay exact for
    /// a value far past the schedule ceiling, where the old checked_mul form
    /// overflowed and fell back to returning the value unchanged. This asserts
    /// the fix, and fails on that fallback branch.
    #[test]
    fn bps_of_is_exact_past_the_schedule_ceiling() {
        // 2^255, well above LIFETIME_EMISSION_CEILING = 2^192.
        let v = Amount([0, 0, 0, 1 << 63]);
        assert_eq!(
            bps_of(v, 2).to_string(),
            "11579208923731619542357098500868790785326998466564056403945758400791312963"
        );
        // Clamping still holds out here too.
        assert_eq!(bps_of(v, u64::MAX), bps_of(v, BPS));
    }

    /// The reward split feeds consensus payouts, so pin `bps_of` through the
    /// public reward methods on the real mainnet schedule. The literals are
    /// taken from `master` before the 256-bit change, so a drift in the split
    /// math (not only in `bps_of`) is caught as the hard fork it would be.
    #[test]
    fn the_mainnet_reward_split_pins_bps_of() {
        let r = mainnet_schedule().record(3_600).unwrap();
        // miner_budget(0) = bps_of(epoch_reward(0), 7000).
        assert_eq!(r.miner_budget(0).to_string(), "209616700889801505817932922");
        // miner_budget at the tail start (epoch 140_256).
        assert_eq!(
            r.miner_budget(140_256).to_string(),
            "16769336071184120465434633"
        );
        // share_reward on the epoch-0 record: miner_budget(0) / target_shares.
        assert_eq!(r.share_reward().to_string(), "58226861358278196060536");
    }
}
