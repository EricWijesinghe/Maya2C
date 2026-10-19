//! Per-block balance changes: every account whose balance a block moved, with
//! the value before and after (Master Prompt 17, the Mesh Data API).
//!
//! A Mesh reconciler checks that the operations it is shown add up to every
//! balance it can query. Fees, burns, staking rewards and slashing all move
//! balances without a transfer output saying so, so deriving operations from
//! transactions would leave gaps. The undo journal already holds each touched
//! account's prior value and the overlay its new one; this is that pair, kept
//! for the block, so the change set is exact by construction.
//!
//! Local-only (invariant 25): it describes this node's history, is not under
//! the state root, and is never in a snapshot. Written and reverted in the same
//! batch as the undo journal, and pruned with the block body.

use primitive_types::U256;

use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::asset::{BALANCE_LEN, encode_balance};
use crate::state::undo::UndoEntry;

/// Key prefix for per-block balance changes.
pub(crate) const BALANCE_CHANGES_PREFIX: &[u8] = b"bdelta:";

/// Bytes per encoded change: address, before, after (each balance a 32-byte
/// little-endian `U256`, ADR-046).
const ENTRY_LEN: usize = 32 + BALANCE_LEN + BALANCE_LEN;

/// One account's balance across one block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BalanceChange {
    /// The account.
    pub address: Address,
    /// Balance before the block; 0 if the account did not exist.
    #[serde(with = "crate::state::amount_serde")]
    pub before: U256,
    /// Balance after the block.
    #[serde(with = "crate::state::amount_serde")]
    pub after: U256,
}

/// The key for `block_id`'s changes.
pub(crate) fn balance_changes_key(block_id: &[u8; 32]) -> Vec<u8> {
    [BALANCE_CHANGES_PREFIX, block_id.as_slice()].concat()
}

/// The accounts whose balance differs between `undo` (before) and `after`,
/// in address order.
pub(crate) fn changes<'a>(
    undo: &[UndoEntry],
    after: impl Fn(&Address) -> Option<U256> + 'a,
) -> Vec<BalanceChange> {
    let mut out: Vec<BalanceChange> = undo
        .iter()
        .filter_map(|entry| {
            let before = entry.previous.as_ref().map_or(U256::zero(), |a| a.balance);
            let now = after(&entry.address).unwrap_or_default();
            (before != now).then_some(BalanceChange {
                address: entry.address,
                before,
                after: now,
            })
        })
        .collect();
    out.sort_by_key(|c| c.address);
    out
}

/// `address`'s balance before `changes` (one block's, in address order) —
/// its `before` if the block moved it, `after_block` otherwise.
#[must_use]
pub fn before_block(changes: &[BalanceChange], address: &Address, after_block: U256) -> U256 {
    changes
        .binary_search_by(|c| c.address.cmp(address))
        .map_or(after_block, |i| changes[i].before)
}

/// Fixed-width encoding; the list is already in address order.
pub(crate) fn encode(changes: &[BalanceChange]) -> Vec<u8> {
    let mut out = Vec::with_capacity(changes.len() * ENTRY_LEN);
    for c in changes {
        out.extend_from_slice(&c.address);
        out.extend_from_slice(&encode_balance(c.before));
        out.extend_from_slice(&encode_balance(c.after));
    }
    out
}

/// Decodes [`encode`]'s output.
///
/// # Errors
///
/// [`NodeError::Storage`] for a length that is not a whole number of entries.
pub(crate) fn decode(bytes: &[u8]) -> Result<Vec<BalanceChange>> {
    if !bytes.len().is_multiple_of(ENTRY_LEN) {
        return Err(NodeError::Storage(format!(
            "balance changes of {} bytes are not whole {ENTRY_LEN}-byte entries",
            bytes.len()
        )));
    }
    Ok(bytes
        .as_chunks::<ENTRY_LEN>()
        .0
        .iter()
        .map(|e| {
            let mut address = [0u8; 32];
            address.copy_from_slice(&e[..32]);
            let word = |at: usize| U256::from_little_endian(&e[at..at + BALANCE_LEN]);
            BalanceChange {
                address,
                before: word(32),
                after: word(32 + BALANCE_LEN),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::state::account::Account;

    fn entry(byte: u8, previous: Option<u64>) -> UndoEntry {
        UndoEntry {
            address: [byte; 32],
            previous: previous.map(|balance| Account {
                balance: U256::from(balance),
                ..Account::default()
            }),
        }
    }

    #[test]
    fn only_moved_balances_are_kept_and_round_trip() {
        let undo = [entry(3, Some(10)), entry(1, None), entry(2, Some(5))];
        let after = |a: &Address| match a[0] {
            1 => Some(U256::from(7)),
            2 => Some(U256::from(5)), // nonce-only change: not a balance change
            _ => Some(U256::from(4)),
        };
        let got = changes(&undo, after);
        assert_eq!(
            got,
            vec![
                BalanceChange {
                    address: [1; 32],
                    before: U256::from(0),
                    after: U256::from(7),
                },
                BalanceChange {
                    address: [3; 32],
                    before: U256::from(10),
                    after: U256::from(4),
                },
            ]
        );
        assert_eq!(decode(&encode(&got)).unwrap(), got);
        assert_eq!(
            before_block(&got, &[3; 32], U256::from(4)),
            U256::from(10),
            "moved: its before"
        );
        assert_eq!(
            before_block(&got, &[9; 32], U256::from(4)),
            U256::from(4),
            "untouched: unchanged"
        );
        assert!(decode(&[0; 5]).is_err());
    }
}
