//! Extracting the six features from a block.
//!
//! Q16 fixed point throughout, each clamped to `±4.0`. The order is
//! consensus once the rule activates, and it matches `fee-market`'s
//! `model::Feature`:
//!
//! | Index | Feature | Definition |
//! |---|---|---|
//! | 0 | fullness | block bytes / target |
//! | 1 | mean transaction size | (block bytes / transactions) / 16 KiB |
//! | 2 | access overlap | transactions naming an account another transaction names / transactions |
//! | 3 | fuel per byte | Σ declared `gas_limit` / block bytes / 1,000 |
//! | 4 | cross-shard | transactions whose accounts fall in more than one shard / transactions |
//! | 5 | size trend | (block bytes − previous block bytes) / target, signed |

use std::collections::BTreeMap;

use maya_blockgraph::shard_of;

use crate::core::{Block, Transaction, TxKind};

/// Features per block.
pub const FEATURE_COUNT: usize = 6;

/// Fractional bits of a feature.
pub const FEATURE_FRAC_BITS: u32 = 16;

/// A feature value of 1.0.
pub const FEATURE_ONE: i64 = 1 << FEATURE_FRAC_BITS;

/// The largest magnitude a feature carries: 4.0.
pub const FEATURE_LIMIT: i64 = 4 * FEATURE_ONE;

/// The transaction size that reads as 1.0.
pub const MEAN_TX_SCALE_BYTES: u64 = 16 * 1024;

/// The declared fuel per byte that reads as 1.0.
pub const FUEL_PER_BYTE_SCALE: u64 = 1_000;

/// The six features of one block, in index order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockFeatures {
    /// Q16 values, each within `±FEATURE_LIMIT`.
    pub values: [i64; FEATURE_COUNT],
}

/// Extracts a block's features.
///
/// `size_bytes` is the block's serialized size, which a validator has already
/// measured; `previous_size_bytes` is its parent's. `target_block_bytes` is the
/// fee market's target, and a zero reads every size-relative feature as 0.
#[must_use]
pub fn extract(
    block: &Block,
    size_bytes: u64,
    previous_size_bytes: u64,
    target_block_bytes: u64,
) -> BlockFeatures {
    let transactions = &block.transactions;
    let count = transactions.len() as u64;
    let accounts: Vec<Vec<[u8; 32]>> = transactions.iter().map(accounts_named).collect();

    let declared_fuel = transactions
        .iter()
        .map(|tx| match &tx.kind {
            TxKind::CallContract(call) => u128::from(call.gas_limit),
            // A deliberate default, not a forgotten arm: a call is the only
            // kind that declares fuel today. A new kind that declares fuel must
            // be added above *before* the rule activates, because the committed
            // weights were trained on this definition — changing it afterwards
            // is a hard fork (docs/neural-gas.md).
            _ => 0,
        })
        .sum::<u128>();
    let size_delta = i128::from(size_bytes) - i128::from(previous_size_bytes);

    BlockFeatures {
        values: [
            ratio(u128::from(size_bytes), u128::from(target_block_bytes)),
            ratio(
                u128::from(size_bytes),
                u128::from(count) * u128::from(MEAN_TX_SCALE_BYTES),
            ),
            ratio(u128::from(overlapping(&accounts)), u128::from(count)),
            ratio(
                declared_fuel,
                u128::from(size_bytes) * u128::from(FUEL_PER_BYTE_SCALE),
            ),
            ratio(u128::from(cross_shard(&accounts)), u128::from(count)),
            signed_ratio(size_delta, i128::from(target_block_bytes)),
        ],
    }
}

/// The accounts a transaction names statically: its sender and every output
/// recipient, deduplicated.
fn accounts_named(tx: &Transaction) -> Vec<[u8; 32]> {
    let mut accounts = Vec::with_capacity(1 + tx.outputs.len());
    accounts.push(tx.sender());
    accounts.extend(tx.outputs.iter().map(|output| output.recipient));
    accounts.sort_unstable();
    accounts.dedup();
    accounts
}

/// Transactions naming at least one account another transaction also names.
fn overlapping(accounts: &[Vec<[u8; 32]>]) -> u64 {
    let mut named_by: BTreeMap<[u8; 32], u32> = BTreeMap::new();
    for set in accounts {
        for account in set {
            *named_by.entry(*account).or_insert(0) += 1;
        }
    }
    accounts
        .iter()
        .filter(|set| set.iter().any(|account| named_by[account] > 1))
        .count() as u64
}

/// Transactions whose accounts fall in more than one shard.
fn cross_shard(accounts: &[Vec<[u8; 32]>]) -> u64 {
    accounts
        .iter()
        .filter(|set| {
            set.split_first().is_some_and(|(first, rest)| {
                let shard = shard_of(first);
                rest.iter().any(|account| shard_of(account) != shard)
            })
        })
        .count() as u64
}

/// `numerator / denominator` in Q16, clamped to `[0, FEATURE_LIMIT]`; 0 for a
/// zero denominator.
fn ratio(numerator: u128, denominator: u128) -> i64 {
    if denominator == 0 {
        return 0;
    }
    let scaled = (numerator << FEATURE_FRAC_BITS) / denominator;
    // Clamped to FEATURE_LIMIT, which fits i64.
    scaled.min(FEATURE_LIMIT as u128) as i64
}

/// `numerator / denominator` in Q16, clamped to `±FEATURE_LIMIT`; 0 for a
/// zero denominator.
fn signed_ratio(numerator: i128, denominator: i128) -> i64 {
    if denominator == 0 {
        return 0;
    }
    let scaled = (numerator << FEATURE_FRAC_BITS) / denominator;
    // Clamped to ±FEATURE_LIMIT, which fits i64.
    scaled.clamp(-i128::from(FEATURE_LIMIT), i128::from(FEATURE_LIMIT)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_clamp_and_tolerate_zero() {
        assert_eq!(ratio(1, 0), 0);
        assert_eq!(ratio(1, 2), FEATURE_ONE / 2);
        assert_eq!(ratio(u128::from(u64::MAX), 1), FEATURE_LIMIT);
        assert_eq!(signed_ratio(-10, 1), -FEATURE_LIMIT);
        assert_eq!(signed_ratio(-1, 4), -FEATURE_ONE / 4);
        assert_eq!(signed_ratio(5, 0), 0);
    }
}
