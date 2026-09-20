//! What one scheduling tick looked like, per leaf.
//!
//! Three signals, each chosen because it decides something:
//!
//! - **Density** ([`TickSample::load`]): transactions touching each leaf. The
//!   split trigger.
//! - **Straddling** ([`TickSample::straddling`]): transactions touching *both*
//!   halves of a leaf. Splitting a leaf whose load mostly straddles turns
//!   intra-shard conflicts into cross-shard ones and buys no parallelism, so it
//!   vetoes the split.
//! - **Cross-shard delay** ([`TickSample::waves`], [`TickSample::cross_shard`]):
//!   how many sequential waves the tick needed, and how many transactions
//!   spanned leaves. A wave is the unit of waiting.
//!
//! Memory pressure is supplied by the caller in permille. It is this node's own
//! reading, which is only acceptable because the map is not consensus (see
//! [`crate::shard_manager`]).

use alloc::vec::Vec;

use crate::error::{GraphError, Result};
use crate::schedule::{Access, schedule};
use crate::shard::SHARD_COUNT;

use super::map::ShardMap;

/// Scale of every ratio in this module.
pub const PERMILLE: u16 = 1_000;

/// Half-flags: the address fell in the lower half of its leaf.
const LOWER: u8 = 1;
/// Half-flags: the address fell in the upper half of its leaf.
const UPPER: u8 = 2;

/// Measurements of one tick against the map that scheduled it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TickSample {
    load: Vec<u32>,
    straddling: Vec<u32>,
    cross_shard: u32,
    waves: u32,
    transactions: u32,
    memory_pressure_permille: u16,
}

impl TickSample {
    /// Measures `transactions`, each given as the addresses it touches.
    ///
    /// # Errors
    ///
    /// [`GraphError::PressureOutOfRange`] above [`PERMILLE`] — refused, not
    /// clamped, because a clamped reading is one nobody took;
    /// [`GraphError::EmptyAccessSet`] for a transaction naming no address.
    pub fn measure(
        map: &ShardMap,
        transactions: &[&[[u8; 32]]],
        memory_pressure_permille: u16,
    ) -> Result<Self> {
        if memory_pressure_permille > PERMILLE {
            return Err(GraphError::PressureOutOfRange);
        }
        let count = map.shard_count();
        let mut load = alloc::vec![0u32; count];
        let mut straddling = alloc::vec![0u32; count];
        let mut cross_shard = 0u32;
        let mut halves = [0u8; SHARD_COUNT];
        let mut accesses = Vec::with_capacity(transactions.len());

        for addresses in transactions {
            let mut mask = 0u64;
            for address in *addresses {
                let shard = map.locate(address);
                mask |= 1u64 << shard.index();
                if let Some(upper) = map.prefix(shard).and_then(|leaf| leaf.half_of(address)) {
                    halves[shard.index()] |= if upper { UPPER } else { LOWER };
                }
            }
            let access = Access::from_mask(mask)?;
            if mask.count_ones() > 1 {
                cross_shard = cross_shard.saturating_add(1);
            }
            let mut remaining = mask;
            while remaining != 0 {
                let index = remaining.trailing_zeros() as usize;
                remaining &= remaining - 1;
                load[index] = load[index].saturating_add(1);
                if halves[index] == LOWER | UPPER {
                    straddling[index] = straddling[index].saturating_add(1);
                }
                halves[index] = 0;
            }
            accesses.push(access);
        }

        Ok(Self {
            load,
            straddling,
            cross_shard,
            waves: u32::try_from(schedule(&accesses)?.len()).unwrap_or(u32::MAX),
            transactions: u32::try_from(transactions.len()).unwrap_or(u32::MAX),
            memory_pressure_permille,
        })
    }

    /// Leaves the sample was taken over.
    #[must_use]
    pub fn shard_count(&self) -> usize {
        self.load.len()
    }

    /// Transactions touching each leaf.
    #[must_use]
    pub fn load(&self) -> &[u32] {
        &self.load
    }

    /// Transactions touching both halves of each leaf.
    #[must_use]
    pub fn straddling(&self) -> &[u32] {
        &self.straddling
    }

    /// Transactions touching more than one leaf.
    #[must_use]
    pub const fn cross_shard(&self) -> u32 {
        self.cross_shard
    }

    /// Sequential waves the tick needed. One means nothing waited.
    #[must_use]
    pub const fn waves(&self) -> u32 {
        self.waves
    }

    /// Transactions in the tick.
    #[must_use]
    pub const fn transactions(&self) -> u32 {
        self.transactions
    }

    /// The caller's memory reading.
    #[must_use]
    pub const fn memory_pressure_permille(&self) -> u16 {
        self.memory_pressure_permille
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn address(first: u32) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[..4].copy_from_slice(&first.to_be_bytes());
        out
    }

    #[test]
    fn load_straddling_and_cross_shard_are_counted_per_leaf() {
        let map = ShardMap::uniform(1).expect("two");
        let lower_low = address(0x1000_0000);
        let lower_high = address(0x5000_0000);
        let upper = address(0x9000_0000);
        let local: &[[u8; 32]] = &[lower_low];
        let straddle: &[[u8; 32]] = &[lower_low, lower_high];
        let cross: &[[u8; 32]] = &[lower_high, upper];

        let sample = TickSample::measure(&map, &[local, straddle, cross], 10).expect("sample");

        assert_eq!(sample.load(), &[3, 1]);
        assert_eq!(sample.straddling(), &[1, 0]);
        assert_eq!(sample.cross_shard(), 1);
        assert_eq!(sample.waves(), 3, "all three touch the lower leaf");
        assert_eq!(sample.transactions(), 3);
    }

    #[test]
    fn a_pressure_reading_above_one_is_refused_not_clamped() {
        let map = ShardMap::uniform(2).expect("four");
        assert_eq!(
            TickSample::measure(&map, &[], 1_001),
            Err(GraphError::PressureOutOfRange)
        );
        let empty: &[[u8; 32]] = &[];
        assert_eq!(
            TickSample::measure(&map, &[empty], 0),
            Err(GraphError::EmptyAccessSet)
        );
    }
}
