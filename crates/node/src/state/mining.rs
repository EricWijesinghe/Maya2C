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

/// The retarget moves at most this factor per epoch, either way.
pub const MAX_RETARGET_FACTOR: u64 = 4;

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
    /// Units issued in epoch 0, before any halving.
    pub initial_epoch_reward: u64,
    /// Epochs between halvings.
    pub halving_epochs: u64,
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
            initial_epoch_reward: self.initial_epoch_reward,
            halving_epochs: self.halving_epochs,
            miner_share_bps: self.miner_share_bps,
            target_shares: self.target_shares,
            max_shares_per_block: self.max_shares_per_block,
            dag: self.dag,
            epoch_blocks,
            epoch: 0,
            anchor: [0; 32],
            target,
            shares: 0,
            paid: 0,
        };
        record.validate()?;
        Ok(record)
    }
}

/// The mining state: fixed parameters plus the current epoch's progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MiningRecord {
    /// As [`MiningGenesis::initial_epoch_reward`].
    pub initial_epoch_reward: u64,
    /// As [`MiningGenesis::halving_epochs`].
    pub halving_epochs: u64,
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
    pub paid: u64,
}

impl MiningRecord {
    /// The whole reward of `epoch`: the initial reward halved once per
    /// `halving_epochs`, reaching zero after 64 halvings.
    #[must_use]
    pub fn epoch_reward(&self, epoch: u64) -> u64 {
        let halvings = epoch / self.halving_epochs;
        if halvings >= 64 {
            0
        } else {
            self.initial_epoch_reward >> halvings
        }
    }

    /// Miners' part of `epoch`'s reward.
    #[must_use]
    pub fn miner_budget(&self, epoch: u64) -> u64 {
        bps_of(self.epoch_reward(epoch), self.miner_share_bps)
    }

    /// What one share earns, capped by what is left of the epoch's budget.
    /// Zero once the budget is spent: the share still lands, it just pays
    /// nothing, and the remainder stays in the reserve.
    #[must_use]
    pub fn share_reward(&self) -> u64 {
        let budget = self.miner_budget(self.epoch);
        let per_share = budget / self.target_shares;
        per_share.min(budget.saturating_sub(self.paid))
    }

    /// The committee's part of `epoch`'s reward.
    #[must_use]
    pub fn validator_reward(&self, epoch: u64) -> u64 {
        self.epoch_reward(epoch) - self.miner_budget(epoch)
    }

    /// The next epoch's share target: more shares than wanted makes shares
    /// harder. Clamped to a factor of [`MAX_RETARGET_FACTOR`] either way, and
    /// never zero.
    #[must_use]
    pub fn retarget(&self) -> [u8; 32] {
        let low = (self.target_shares / MAX_RETARGET_FACTOR).max(1);
        let high = self.target_shares.saturating_mul(MAX_RETARGET_FACTOR);
        let observed = self.shares.clamp(low, high);
        let previous = U256::from_be_bytes(&self.target);
        // Overflow means the next target is past 256 bits, which is easier
        // than any digest: saturate to the easiest target rather than wrap to
        // a hard one. Only reachable from a target already near the top.
        let next = previous
            .mul_div_u64(self.target_shares, observed)
            .unwrap_or(U256::from_be_bytes(&[0xff; 32]));
        if next.is_zero() {
            U256::ONE.to_be_bytes()
        } else {
            next.to_be_bytes()
        }
    }

    /// Fixed-width encoding, version 1.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + 8 * 9 + 1 + 64);
        out.push(1);
        for value in [
            self.initial_epoch_reward,
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
        for value in [self.shares, self.paid] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out
    }

    /// Decodes [`Self::encode`].
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] on a wrong version, profile or length.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        if r.read_u8()? != 1 {
            return Err(NodeError::Decode("mining record version".into()));
        }
        let record = Self {
            initial_epoch_reward: r.read_u64()?,
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
            paid: r.read_u64()?,
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
        if U256::from_be_bytes(&self.target).is_zero() {
            return bad("target is zero");
        }
        // A share that rounds to nothing would have miners working for free
        // from block 1.
        if self.miner_share_bps > 0 && self.miner_budget(0) / self.target_shares == 0 {
            return bad("a share in epoch 0 would pay nothing");
        }
        Ok(())
    }
}

/// `value × bps / 10000`, in 128 bits so it cannot overflow.
#[must_use]
pub fn bps_of(value: u64, bps: u64) -> u64 {
    let scaled = u128::from(value) * u128::from(bps.min(BPS)) / u128::from(BPS);
    // At most `value`, so it fits.
    u64::try_from(scaled).unwrap_or(value)
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

/// Key of one spent share.
#[must_use]
pub fn seen_key(header: &[u8; 32], nonce: u64) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new_derive_key("maya2c mining seen v1");
    hasher.update(header);
    hasher.update(&nonce.to_le_bytes());
    let mut key = SEEN_PREFIX.to_vec();
    key.extend_from_slice(hasher.finalize().as_bytes());
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> MiningRecord {
        MiningGenesis {
            initial_epoch_reward: 1_000_000,
            halving_epochs: 10,
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
        assert_eq!(r.epoch_reward(0), 1_000_000);
        assert_eq!(r.epoch_reward(9), 1_000_000);
        assert_eq!(r.epoch_reward(10), 500_000);
        assert_eq!(r.epoch_reward(640), 0);
        assert_eq!(r.epoch_reward(u64::MAX), 0);
    }

    #[test]
    fn miner_and_validator_parts_sum_to_the_reward() {
        let r = record();
        for epoch in [0, 10, 33, 200] {
            assert_eq!(
                r.miner_budget(epoch) + r.validator_reward(epoch),
                r.epoch_reward(epoch)
            );
        }
        assert_eq!(r.miner_budget(0), 700_000);
    }

    #[test]
    fn share_reward_stops_at_the_epoch_budget() {
        let mut r = record();
        assert_eq!(r.share_reward(), 7_000);
        r.paid = 700_000 - 3;
        assert_eq!(r.share_reward(), 3);
        r.paid = 700_000;
        assert_eq!(r.share_reward(), 0);
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
        r.shares = 0;
        assert_eq!(
            U256::from_be_bytes(&r.retarget()),
            base.mul_div_u64(4, 1).unwrap()
        );
    }

    #[test]
    fn record_round_trips_and_rejects_corruption() {
        let mut r = record();
        r.epoch = 7;
        r.anchor = [9; 32];
        r.shares = 3;
        r.paid = 21_000;
        assert_eq!(MiningRecord::decode(&r.encode()).unwrap(), r);
        let mut bytes = r.encode();
        bytes.push(0);
        assert!(MiningRecord::decode(&bytes).is_err());
        bytes = r.encode();
        bytes[0] = 2;
        assert!(MiningRecord::decode(&bytes).is_err());
    }

    #[test]
    fn genesis_refuses_bad_parameters() {
        let good = MiningGenesis {
            initial_epoch_reward: 1_000,
            halving_epochs: 1,
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
                initial_epoch_reward: 1,
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
        assert_ne!(seen_key(&base, 1), seen_key(&base, 2));
    }
}
