//! State root over accounts — `spec/03-state.md` ROOT-1 .. ROOT-4.

use std::collections::BTreeMap;

use crate::Address;
use crate::stf::Account;

/// ROOT-1: `blake3(0x00 ‖ address ‖ balance_le64 ‖ nonce_le64)`.
#[must_use]
pub fn account_leaf(address: &Address, account: &Account) -> [u8; 32] {
    let mut bytes = vec![0x00];
    bytes.extend_from_slice(address);
    bytes.extend_from_slice(&account.balance.to_le_bytes());
    bytes.extend_from_slice(&account.nonce.to_le_bytes());
    *blake3::hash(&bytes).as_bytes()
}

/// ROOT-2: `blake3(0x01 ‖ left ‖ right)`.
#[must_use]
pub fn internal_node(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut bytes = vec![0x01];
    bytes.extend_from_slice(left);
    bytes.extend_from_slice(right);
    *blake3::hash(&bytes).as_bytes()
}

/// ROOT-3: pair left to right; an unpaired last node is promoted unchanged;
/// repeat until one node remains. No leaves: 32 zero bytes.
#[must_use]
pub fn merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return [0; 32];
    }
    let mut level = leaves.to_vec();
    while level.len() > 1 {
        let mut next = Vec::new();
        let mut i = 0;
        while i < level.len() {
            if i + 1 < level.len() {
                next.push(internal_node(&level[i], &level[i + 1]));
            } else {
                next.push(level[i]);
            }
            i += 2;
        }
        level = next;
    }
    level[0]
}

/// ROOT-4: leaves in ascending address order. With no other state layer
/// present (no channels, notes, trading or governance records), the state root
/// is this accounts root.
#[must_use]
pub fn accounts_root(accounts: &BTreeMap<Address, Account>) -> [u8; 32] {
    let leaves: Vec<[u8; 32]> = accounts
        .iter()
        .map(|(a, acc)| account_leaf(a, acc))
        .collect();
    merkle_root(&leaves)
}
