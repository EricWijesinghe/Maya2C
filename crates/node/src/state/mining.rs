//! Mining rewards (ADR-044): the record, the schedule, and the share digest.
//!
//! Miners never order blocks; DAG-BFT does. A miner earns by putting a
//! [`MiningShare`](crate::core::mining_payload::MiningShare) on chain whose DAG
//! proof of work meets the epoch's share target. Everything here is integer
//! arithmetic over committed state: no floats, no wall-clock (invariant 20).
//!
//! Coins are never minted. Genesis allocates the unissued supply to
//! [`MINING_RESERVE`], an address no key derives, and rewards are transfers out
//! of it, so the conservation equation (no native mint, ever) is unchanged.
//!
//! | Key | Holds |
//! |---|---|
//! | `k:mine` | the [`MiningRecord`] |
//! | `k:mseen:<digest>` | one spent share, so it cannot be paid twice |
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

// Both must stay inside the committed staking layer.
const _: () = assert!(MINING_KEY[0] == b'k' && MINING_KEY[1] == b':');
const _: () = assert!(SEEN_PREFIX[0] == b'k' && SEEN_PREFIX[1] == b':');

/// Holds the unissued supply. ASCII `maya2c/mining/reserve/v1`, zero-padded:
/// not a hash of any key pair, so no signature can spend from it.
pub const MINING_RESERVE: Address = [
    0x6d, 0x61, 0x79, 0x61, 0x32, 0x63, 0x2f, 0x6d, 0x69, 0x6e, 0x69, 0x6e, 0x67, 0x2f, 0x72, 0x65,
    0x73, 0x65, 0x72, 0x76, 0x65, 0x2f, 0x76, 0x31, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Basis points in a whole.
pub const BPS: u64 = 10_000;

/// Mining record format; see [`MiningRecord::encode`].
const RECORD_VERSION: u8 = 4;

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
            paid: Amount::ZERO,
            spent_after: 0,
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
    /// Hash of the block that opened `epoch`; zero in epoch 0. Work cannot be
    /// done before this block is final.
    pub anchor: [u8; 32],
    /// The share target, big-endian. A digest at or below it is a share.
    pub target: [u8; 32],
    /// Shares accepted in `epoch`.
    pub shares: u64,
    /// Units paid to miners in `epoch`.
    pub paid: Amount,
    /// Blocks into `epoch` after which its miner budget was spent; 0 while
    /// some is left. Shares past the budget are refused, so `shares` stops
    /// near `target_shares` and cannot show how far demand ran over it; how
    /// early the budget ran out does (security review of ADR-044).
    pub spent_after: u64,
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

    /// What one share earns, capped by what is left of the epoch's budget.
    /// Zero once the budget is spent: the share still lands, it just pays
    /// nothing, and the remainder stays in the reserve.
    #[must_use]
    pub fn share_reward(&self) -> Amount {
        let budget = self.miner_budget(self.epoch);
        // `validate` keeps `target_shares` positive.
        let per_share = budget
            .checked_div(Amount::from_u64(self.target_shares))
            .unwrap_or(Amount::ZERO);
        per_share.min(budget.saturating_sub(self.paid))
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

    /// Shares the epoch would have seen had the budget lasted it out:
    /// `shares` scaled by how early the budget was spent. Integer only.
    fn demand(&self) -> u64 {
        if self.spent_after == 0 {
            return self.shares;
        }
        let scaled =
            u128::from(self.shares) * u128::from(self.epoch_blocks) / u128::from(self.spent_after);
        u64::try_from(scaled).unwrap_or(u64::MAX)
    }

    /// Fixed-width encoding, version 4: adds the tail reward. Version 3 had no
    /// tail, version 2 held amounts as `u64` and version 1 lacked
    /// `spent_after`; none was on a public chain with mining active.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + 64 + 8 * 8 + 32 + 1 + 96);
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
        out.extend_from_slice(&self.paid.to_le_bytes());
        out.extend_from_slice(&self.spent_after.to_le_bytes());
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
            paid: Amount::from_le_bytes(r.read_array()?),
            spent_after: r.read_u64()?,
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
        if self.spent_after > self.epoch_blocks {
            return bad("spent_after past the end of the epoch");
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
        Ok(())
    }
}

/// `value × bps / 10000`, through a 512-bit product so it cannot overflow.
#[must_use]
pub fn bps_of(value: Amount, bps: u64) -> Amount {
    // At most `value`, so the quotient fits; the fallback is unreachable.
    value
        .mul_div(Amount::from_u64(bps.min(BPS)), Amount::from_u64(BPS))
        .unwrap_or(value)
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
    fn share_reward_stops_at_the_epoch_budget() {
        let mut r = record();
        assert_eq!(r.share_reward(), maya_ledger_math::Amount::from_u64(7_000));
        r.paid = maya_ledger_math::Amount::from_u64(700_000 - 3);
        assert_eq!(r.share_reward(), maya_ledger_math::Amount::from_u64(3));
        r.paid = maya_ledger_math::Amount::from_u64(700_000);
        assert_eq!(r.share_reward(), maya_ledger_math::Amount::from_u64(0));
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

    /// Shares past the budget are refused, so `shares` stops near
    /// `target_shares`; how early the budget ran out is what shows demand.
    #[test]
    fn a_budget_spent_early_hardens_the_target_by_how_early() {
        let mut r = record();
        let base = U256::from_be_bytes(&r.target);
        r.shares = 100;
        r.spent_after = 300; // half the 600-block epoch: twice the demand
        assert_eq!(
            U256::from_be_bytes(&r.retarget()),
            base.mul_div_u64(1, 2).unwrap()
        );
        r.spent_after = 1; // drained in one block: clamped at x4
        assert_eq!(
            U256::from_be_bytes(&r.retarget()),
            base.mul_div_u64(1, 4).unwrap()
        );
        r.spent_after = 0; // budget left over: shares alone, as before
        assert_eq!(r.retarget(), r.target);
    }

    #[test]
    fn a_spent_after_past_the_epoch_is_refused_on_decode() {
        let mut r = record();
        r.spent_after = 601;
        assert!(MiningRecord::decode(&r.encode()).is_err());
        r.spent_after = 600;
        assert_eq!(MiningRecord::decode(&r.encode()).unwrap(), r);
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
        r.paid = maya_ledger_math::Amount::from_u64(21_000);
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
        assert!(MiningRecord::decode(&with_tail(1, 600).encode()).is_ok());
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
}
