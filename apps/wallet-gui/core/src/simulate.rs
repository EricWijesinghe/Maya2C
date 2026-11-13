//! Sim-firewall, local form: before signing, check a transfer against the
//! sender's account as the node reports it and say what would change.
//!
//! The node has no `simulate_transaction` RPC (nothing in
//! `crates/node/src/rpc` simulates, dry-runs or estimates), so this is a
//! pre-check of balance, nonce and fee, not a simulation: it does not run the
//! state transition and it trusts the one node it read from. A full
//! simulation needs that read-only node RPC, a v3 candidate (CATALOG
//! `sim-firewall`).

use primitive_types::U256;
use serde::{Deserialize, Serialize};

/// What the transfer would change, and why it should not be signed, if so.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Precheck {
    /// The sender's balance the node reported.
    #[serde(with = "custom_l1_node::state::amount_serde")]
    pub balance_before: U256,
    /// The balance once the transfer applies; equal to `balance_before` when
    /// it is refused, because a refused transfer changes nothing.
    #[serde(with = "custom_l1_node::state::amount_serde")]
    pub balance_after: U256,
    /// The account's next nonce the node reported.
    pub nonce_before: u64,
    /// The next nonce once the transfer applies.
    pub nonce_after: u64,
    /// Reasons not to sign. Non-empty means the wallet refuses.
    pub refusals: Vec<String>,
    /// Surprises the person should read before signing.
    pub warnings: Vec<String>,
    /// Always false: see the module docs.
    pub full_simulation: bool,
}

impl Precheck {
    /// Whether nothing stops the signature. Not "safe": one node's word on the
    /// balance and nonce, with no view of pending mempool debits.
    #[must_use]
    pub fn passes(&self) -> bool {
        self.refusals.is_empty()
    }
}

/// Checks a transfer of `amount` plus `fee` at `nonce` against an account
/// holding `balance` whose next nonce is `account_nonce`. `to_self` is a
/// transfer back to the sender, which only spends the fee.
#[must_use]
pub fn precheck_transfer(
    balance: U256,
    account_nonce: u64,
    amount: U256,
    fee: U256,
    nonce: u64,
    to_self: bool,
) -> Precheck {
    let mut refusals = Vec::new();
    let mut warnings = Vec::new();
    if nonce < account_nonce {
        refusals.push(format!(
            "nonce {nonce} is already used; the account's next nonce is {account_nonce}"
        ));
    } else if nonce > account_nonce {
        warnings.push(format!(
            "nonce {nonce} skips ahead of {account_nonce}; it waits until the transfers before it land"
        ));
    }
    let next_nonce = nonce.checked_add(1);
    if next_nonce.is_none() {
        refusals.push("the nonce has no successor; the account cannot send again".to_string());
    }
    let spent = if to_self { Some(fee) } else { amount.checked_add(fee) };
    let after = match spent {
        None => {
            refusals.push("the amount plus the fee does not fit in 256 bits".to_string());
            None
        }
        Some(spent) => {
            let after = balance.checked_sub(spent);
            if after.is_none() {
                refusals.push("the balance does not cover the amount plus the fee".to_string());
            }
            after
        }
    };
    if amount.is_zero() {
        warnings.push("the amount is zero: only the fee moves".to_string());
    }
    if to_self {
        warnings.push("the recipient is this account: only the fee is spent".to_string());
    }
    let refused = !refusals.is_empty();
    let balance_after = if refused { balance } else { after.unwrap_or(balance) };
    if !refused && balance_after.is_zero() {
        warnings.push("this empties the account".to_string());
    }
    Precheck {
        balance_before: balance,
        balance_after,
        nonce_before: account_nonce,
        nonce_after: next_nonce.filter(|_| !refused).unwrap_or(account_nonce),
        refusals,
        warnings,
        full_simulation: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: u64) -> U256 {
        U256::from(v)
    }

    #[test]
    fn ordinary_send_shows_the_balance_change() {
        // Arrange / Act
        let check = precheck_transfer(n(100), 4, n(30), n(2), 4, false);
        // Assert
        assert!(check.passes());
        assert_eq!(check.balance_after, n(68));
        assert_eq!(check.nonce_after, 5);
        assert!(check.warnings.is_empty());
        assert!(!check.full_simulation);
    }

    #[test]
    fn insufficient_balance_is_refused_and_changes_nothing() {
        // Arrange / Act: fee pushes it one unit over.
        let check = precheck_transfer(n(100), 0, n(99), n(2), 0, false);
        // Assert
        assert!(!check.passes());
        assert_eq!(check.balance_after, n(100));
        assert_eq!(check.nonce_after, 0);
    }

    #[test]
    fn exact_balance_passes_with_an_empty_account_warning() {
        let check = precheck_transfer(n(100), 0, n(98), n(2), 0, false);
        assert!(check.passes());
        assert_eq!(check.balance_after, U256::zero());
        assert!(check.warnings.iter().any(|w| w.contains("empties")));
    }

    #[test]
    fn used_nonce_is_refused() {
        let check = precheck_transfer(n(100), 5, n(1), n(1), 4, false);
        assert!(!check.passes());
        assert!(check.refusals[0].contains("already used"));
    }

    #[test]
    fn future_nonce_warns() {
        let check = precheck_transfer(n(100), 5, n(1), n(1), 7, false);
        assert!(check.passes());
        assert!(check.warnings[0].contains("skips ahead"));
    }

    #[test]
    fn amount_plus_fee_overflow_is_refused_not_wrapped() {
        let check = precheck_transfer(U256::MAX, 0, U256::MAX, n(1), 0, false);
        assert!(!check.passes());
        assert!(check.refusals[0].contains("256 bits"));
        assert_eq!(check.balance_after, U256::MAX);
    }

    #[test]
    fn last_nonce_is_refused_not_saturated() {
        let check = precheck_transfer(n(10), u64::MAX, n(1), n(1), u64::MAX, false);
        assert!(!check.passes());
        assert_eq!(check.nonce_after, u64::MAX);
    }

    #[test]
    fn send_to_self_spends_only_the_fee() {
        let check = precheck_transfer(n(10), 0, n(10), n(1), 0, true);
        assert!(check.passes());
        assert_eq!(check.balance_after, n(9));
        assert!(check.warnings.iter().any(|w| w.contains("this account")));
    }
}
