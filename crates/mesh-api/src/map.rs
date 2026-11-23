//! Node shapes to Mesh shapes. Pure functions, so the rules are tested
//! without a server.
//!
//! **One operation per balance a block moved.** A block's signed transactions
//! are listed with no operations, and one extra transaction per block,
//! `block-balance-changes:<block id>`, carries a `BALANCE_CHANGE` for every
//! account whose balance changed. Fees, burns, staking rewards and slashing
//! move balances without an output saying so; taking the changes from the
//! node's before/after record (`state::balance_changes`) instead of from
//! transaction outputs is what lets a reconciler's sums close.

use crate::node::{Change, NodeBlock};
use crate::types::{
    AccountIdentifier, Amount, Block, BlockIdentifier, Currency, OP_BALANCE_CHANGE, Operation,
    OperationIdentifier, STATUS_SUCCESS, Transaction, TransactionIdentifier,
};

/// The native coin's ticker.
pub const SYMBOL: &str = "MAYA";
/// Milliseconds per second.
const MS_PER_S: u64 = 1_000;
/// The synthetic transaction's id prefix.
pub const CHANGES_TX_PREFIX: &str = "block-balance-changes:";

/// The native coin.
#[must_use]
pub fn currency() -> Currency {
    Currency {
        symbol: SYMBOL.to_string(),
        decimals: DECIMALS,
    }
}

/// [`maya_amount::COIN_DECIMALS`] as Mesh's `u32`. A reconciler that reads 0
/// here shows every balance 10^24 times too large.
const DECIMALS: u32 = 24;
const _: () = assert!(DECIMALS as usize == maya_amount::COIN_DECIMALS);

/// `after - before` as a signed decimal string. Both are 256-bit, so the
/// magnitude is taken in the order that cannot underflow and the sign is
/// written separately.
#[must_use]
pub fn delta(change: &Change) -> String {
    if change.after >= change.before {
        change.after.saturating_sub(change.before).to_string()
    } else {
        format!("-{}", change.before.saturating_sub(change.after))
    }
}

fn operation(index: u64, change: &Change) -> Operation {
    Operation {
        operation_identifier: OperationIdentifier { index },
        kind: OP_BALANCE_CHANGE.to_string(),
        status: STATUS_SUCCESS.to_string(),
        account: AccountIdentifier {
            address: change.address.clone(),
        },
        amount: Amount {
            value: delta(change),
            currency: currency(),
        },
    }
}

/// The Mesh block for `block` and its balance `changes` (empty for genesis,
/// whose allocations are the reconciler's bootstrap balances).
#[must_use]
pub fn block(block: &NodeBlock, changes: &[Change]) -> Block {
    let id = BlockIdentifier {
        index: block.height,
        hash: block.header.id.clone(),
    };
    // Genesis names itself as its parent, as the specification asks.
    let parent = if block.height == 0 {
        id.clone()
    } else {
        BlockIdentifier {
            index: block.height - 1,
            hash: block.header.prev_hash.clone(),
        }
    };
    let mut transactions: Vec<Transaction> = block
        .transactions
        .iter()
        .map(|tx| Transaction {
            transaction_identifier: TransactionIdentifier {
                hash: tx.txid.clone(),
            },
            operations: Vec::new(),
        })
        .collect();
    let ops: Vec<Operation> = (0u64..)
        .zip(changes.iter().filter(|c| c.before != c.after))
        .map(|(i, c)| operation(i, c))
        .collect();
    if !ops.is_empty() {
        transactions.push(Transaction {
            transaction_identifier: TransactionIdentifier {
                hash: format!("{CHANGES_TX_PREFIX}{}", block.header.id),
            },
            operations: ops,
        });
    }
    Block {
        block_identifier: id,
        parent_block_identifier: parent,
        timestamp: block.header.timestamp.saturating_mul(MS_PER_S),
        transactions,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::node::{Header, Tx};

    fn node_block(height: u64) -> NodeBlock {
        NodeBlock {
            height,
            header: Header {
                id: "bb".into(),
                prev_hash: "aa".into(),
                timestamp: 1_790_000_000,
            },
            transactions: vec![Tx { txid: "t1".into() }],
        }
    }

    fn change(address: &str, before: u64, after: u64) -> Change {
        Change {
            address: address.into(),
            before: maya_amount::Amount::from_u64(before),
            after: maya_amount::Amount::from_u64(after),
        }
    }

    /// 2^256 - 1, the largest balance the node can hold.
    const MAX_DEC: &str =
        "115792089237316195423570985008687907853269984665640564039457584007913129639935";

    #[test]
    fn every_moved_balance_is_one_operation_and_they_sum_to_the_net_change() {
        let changes = [
            change("01", 100, 40),
            change("02", 0, 55),
            change("03", 5, 5),
        ];
        let b = block(&node_block(7), &changes);
        assert_eq!(
            b.parent_block_identifier,
            BlockIdentifier {
                index: 6,
                hash: "aa".into()
            }
        );
        assert_eq!(b.timestamp, 1_790_000_000_000);
        assert_eq!(b.transactions.len(), 2);
        assert!(b.transactions[0].operations.is_empty());
        let ops = &b.transactions[1].operations;
        assert_eq!(ops.len(), 2, "an unchanged balance is not an operation");
        let values: Vec<&str> = ops.iter().map(|o| o.amount.value.as_str()).collect();
        assert_eq!(values, ["-60", "55"]);
        // 5 burned: the sum is the net change, which a reconciler checks
        // against balances, not against zero.
        let net: i128 = ops
            .iter()
            .map(|o| o.amount.value.parse::<i128>().unwrap())
            .sum();
        assert_eq!(net, -5);
    }

    #[test]
    fn genesis_is_its_own_parent_and_extremes_do_not_overflow() {
        let b = block(&node_block(0), &[]);
        assert_eq!(b.parent_block_identifier, b.block_identifier);
        assert_eq!(b.transactions.len(), 1);
        assert_eq!(delta(&change("x", u64::MAX, 0)), format!("-{}", u64::MAX));
        assert_eq!(delta(&change("x", 0, u64::MAX)), u64::MAX.to_string());
        // Beyond u64 and i128: the full 256-bit range in both directions.
        let max = Change {
            address: "x".into(),
            before: maya_amount::Amount([u64::MAX; 4]),
            after: maya_amount::Amount::from_u64(0),
        };
        assert_eq!(delta(&max), format!("-{MAX_DEC}"));
        let rise = Change {
            before: max.after,
            after: max.before,
            ..max
        };
        assert_eq!(delta(&rise), MAX_DEC);
    }

    #[test]
    fn the_currency_carries_the_chains_24_decimals() {
        // Mesh reads `value` in base units and divides by 10^decimals.
        assert_eq!(currency().decimals, 24);
        assert_eq!(currency().symbol, "MAYA");
    }
}
