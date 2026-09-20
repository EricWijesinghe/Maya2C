//! Recent hash power, as a number the chain can hold.
//!
//! ## The problem this solves
//!
//! This chain has no block reward. `bins/pool-service/src/config.rs` states it in
//! the code: `apply_block_checked` credits no subsidy and fees burn to the fee
//! sink. There is no coinbase, and [`crate::core::BlockHeader`] carries
//! `prev_hash`, `state_root`, `timestamp`, `nonce`, and `difficulty_target` —
//! nothing that says who mined it.
//!
//! So before this module there was **no on-chain record of who did any work**,
//! and "voting power based on hash power" had no data source at all.
//!
//! ## Why a claim transaction rather than a header field
//!
//! Adding a miner field to the header would change the proof-of-work preimage,
//! and therefore the miner, the pool protocol, the Stratum implementation, and
//! the CUDA kernel — `core::block::NONCE_RANGE` exists precisely because those
//! four have to agree byte for byte about the header's layout.
//!
//! A claim transaction changes none of them. At most one is admitted per block
//! and it credits that block's work to an address the *miner* names, because
//! the miner is the party that chose what went in the block. That is the
//! correct answer to "who did this work" and it needs no new consensus field.
//!
//! The honest consequence: hash-power voting is **miner-directed**, not
//! pool-participant-directed. A pool's miners contribute the hashes; the pool
//! operator decides who is credited. That is a real property of this design and
//! not one the mechanism can fix — attributing work to the individual hashers
//! would require the share data the pool holds off chain.
//!
//! ## Why credit decays
//!
//! Voting power should reflect hash power *now*. A miner who ran a farm for a
//! month two years ago is not securing the chain today, and an all-time
//! accumulator would let them govern it forever.
//!
//! Decay is by halving, not by a multiplier: a value loses half its weight
//! every [`WORK_HALF_LIFE_BLOCKS`]. Halving is exact in integer arithmetic —
//! it is a shift — where a fractional decay would need a rounding rule, and a
//! rounding rule in voting weight is a bias in whichever direction it falls.

use crate::core::codec::ByteReader;
use crate::error::Result;

/// Blocks over which work credit loses half its weight.
///
/// 5,760 — about a day at the 15-second target in
/// `consensus::difficulty::TARGET_BLOCK_TIME`. A week of absence leaves about
/// one part in 128 of a miner's weight, which is the intended shape: leaving is
/// gradual rather than instant, and returning is quick.
pub const WORK_HALF_LIFE_BLOCKS: u64 = 5_760;

/// Halvings past which a credit is treated as gone.
///
/// 127, because a `u128` shifted 128 places is undefined rather than zero.
/// Saturating here rather than at the shift is what keeps a record that has sat
/// untouched for years from being an arithmetic edge case.
const MAX_HALVINGS: u64 = 127;

/// Encoded size of a [`WorkRecord`].
pub const WORK_RECORD_LEN: usize = 16 + 8;

/// One address's accumulated, decaying work credit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkRecord {
    /// Credit as of `last_height`, before any further decay.
    pub credit: u128,
    /// Height the credit was last brought up to date at.
    pub last_height: u64,
}

impl WorkRecord {
    /// The credit as it stands at `height`.
    ///
    /// Decay is applied on read rather than on a schedule, so no block has to
    /// walk every address to age their credit. The record stores what it was
    /// worth at `last_height` and every reader derives the rest.
    #[must_use]
    pub const fn value_at(&self, height: u64) -> u128 {
        // A height below the record's own means a reorg is in flight. Reporting
        // the undecayed value is the conservative reading: it is what the
        // credit was worth at a height the chain has actually seen.
        let elapsed = match height.checked_sub(self.last_height) {
            Some(elapsed) => elapsed,
            None => return self.credit,
        };

        let halvings = elapsed / WORK_HALF_LIFE_BLOCKS;
        if halvings > MAX_HALVINGS {
            return 0;
        }
        self.credit >> halvings
    }

    /// Adds `work` to the credit, first ageing what was already there.
    ///
    /// Returns a new record rather than mutating, so a caller that computes a
    /// credit and then rejects the claim cannot leave a half-applied one
    /// behind.
    #[must_use]
    pub const fn credited(&self, work: u128, height: u64) -> Self {
        let aged = self.value_at(height);
        Self {
            // Saturating: a credit at `u128::MAX` is already more voting weight
            // than the tally will accept, and wrapping it would hand the
            // wrapper a weight of nearly zero — or, worse, a weight of nearly
            // everything.
            credit: aged.saturating_add(work),
            last_height: height,
        }
    }

    /// Encodes the record.
    #[must_use]
    pub fn encode(&self) -> [u8; WORK_RECORD_LEN] {
        let mut buf = [0u8; WORK_RECORD_LEN];
        buf[..16].copy_from_slice(&self.credit.to_le_bytes());
        buf[16..].copy_from_slice(&self.last_height.to_le_bytes());
        buf
    }

    /// Decodes a stored record.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`](crate::error::NodeError::Decode) if the record is not exactly
    /// [`WORK_RECORD_LEN`] bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let low = reader.read_u64()?;
        let high = reader.read_u64()?;
        let last_height = reader.read_u64()?;
        reader.finish()?;
        Ok(Self {
            credit: u128::from(low) | (u128::from(high) << 64),
            last_height,
        })
    }
}

/// Narrows a 256-bit work value to the `u128` the credit is held in.
///
/// Saturating. At the difficulties this chain can reach, a single block's work
/// is nowhere near `2^128`; the saturation exists because `work_from_target`
/// admits a target of zero, and a value that wrapped would credit a miner
/// almost nothing for the hardest block ever mined.
#[must_use]
pub fn narrow_work(work: &crate::consensus::uint::U256) -> u128 {
    let bytes = work.to_be_bytes();
    // Anything in the high sixteen bytes is already past `u128::MAX`.
    if bytes[..16].iter().any(|byte| *byte != 0) {
        return u128::MAX;
    }
    let mut low = [0u8; 16];
    low.copy_from_slice(&bytes[16..]);
    u128::from_be_bytes(low)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_halves_over_one_half_life() {
        let record = WorkRecord {
            credit: 1_024,
            last_height: 0,
        };

        assert_eq!(record.value_at(0), 1_024);
        assert_eq!(record.value_at(WORK_HALF_LIFE_BLOCKS - 1), 1_024);
        assert_eq!(record.value_at(WORK_HALF_LIFE_BLOCKS), 512);
        assert_eq!(record.value_at(WORK_HALF_LIFE_BLOCKS * 2), 256);
        assert_eq!(record.value_at(WORK_HALF_LIFE_BLOCKS * 10), 1);
    }

    #[test]
    fn a_credit_left_for_long_enough_reaches_zero_rather_than_wrapping() {
        // A `u128` shifted 128 places is undefined rather than zero, and a
        // record that has sat untouched for years must not be an arithmetic
        // edge case.
        let record = WorkRecord {
            credit: u128::MAX,
            last_height: 0,
        };
        assert_eq!(record.value_at(WORK_HALF_LIFE_BLOCKS * 200), 0);
        assert_eq!(record.value_at(u64::MAX), 0);
    }

    #[test]
    fn crediting_ages_what_was_there_before_adding() {
        // Otherwise a miner could park credit and top it up forever without it
        // ever decaying — an all-time accumulator wearing a decay's clothes.
        let record = WorkRecord {
            credit: 1_000,
            last_height: 0,
        };
        let updated = record.credited(1_000, WORK_HALF_LIFE_BLOCKS);

        assert_eq!(updated.credit, 1_500, "500 aged plus 1000 new");
        assert_eq!(updated.last_height, WORK_HALF_LIFE_BLOCKS);
    }

    #[test]
    fn a_record_ahead_of_the_chain_reads_as_undecayed() {
        // A reorg in flight. Reporting the undecayed value is what the credit
        // was worth at a height the chain has actually seen.
        let record = WorkRecord {
            credit: 1_000,
            last_height: 100,
        };
        assert_eq!(record.value_at(50), 1_000);
    }

    #[test]
    fn a_record_round_trips_through_its_encoding() {
        let record = WorkRecord {
            credit: u128::MAX - 7,
            last_height: 123_456,
        };
        assert_eq!(WorkRecord::decode(&record.encode()), Ok(record));
        assert!(WorkRecord::decode(&record.encode()[..16]).is_err());
    }

    #[test]
    fn work_beyond_a_u128_saturates_rather_than_wrapping() {
        use crate::consensus::uint::U256;

        // Wrapping would credit a miner almost nothing for the hardest block
        // ever mined, which is precisely backwards.
        assert_eq!(narrow_work(&U256::MAX), u128::MAX);
        assert_eq!(narrow_work(&U256::ONE), 1);
        assert_eq!(narrow_work(&U256::ZERO), 0);
    }
}
