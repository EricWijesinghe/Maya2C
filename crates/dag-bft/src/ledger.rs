//! The one state machine every mode feeds.
//!
//! A deliberately small transfer ledger: the point is not the ledger but that
//! the *same* function consumes the ordered output of every consensus mode, so
//! "the same state machine runs under every mode" is a property a test can
//! check rather than a sentence. The node's real state machine is
//! `StateDB::apply_block`; this one is its stand-in inside the simulator.

use std::collections::BTreeMap;

/// Accounts in the simulated ledger.
pub const ACCOUNTS: u64 = 64;
/// Opening balance of every account.
pub const OPENING_BALANCE: u64 = 1_000_000;

/// Balances, and how many transfers were applied or refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ledger {
    balances: BTreeMap<u64, u64>,
    applied: u64,
    refused: u64,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            balances: (0..ACCOUNTS).map(|a| (a, OPENING_BALANCE)).collect(),
            applied: 0,
            refused: 0,
        }
    }
}

impl Ledger {
    /// The opening state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies transaction `tx`: a transfer derived from its id. Checked
    /// arithmetic; an overdraft is refused and changes nothing.
    pub fn apply(&mut self, tx: u64) {
        let from = tx % ACCOUNTS;
        let to = (tx / ACCOUNTS) % ACCOUNTS;
        let amount = tx % 1_000 + 1;
        let Some(from_balance) = self.balances.get(&from).copied() else {
            self.refused += 1;
            return;
        };
        let Some(debited) = from_balance.checked_sub(amount) else {
            self.refused += 1;
            return;
        };
        self.balances.insert(from, debited);
        let credited = self
            .balances
            .get(&to)
            .copied()
            .unwrap_or(0)
            .saturating_add(amount);
        self.balances.insert(to, credited);
        self.applied += 1;
    }

    /// Total supply; transfers never change it.
    pub fn supply(&self) -> u128 {
        self.balances.values().map(|b| u128::from(*b)).sum()
    }

    /// [`Ledger::apply`] for a transaction the engine carried as bytes: the
    /// simulator submits each id as eight little-endian bytes. Anything else
    /// is refused, like an overdraft.
    pub fn apply_bytes(&mut self, tx: &[u8]) {
        match <[u8; 8]>::try_from(tx) {
            Ok(id) => self.apply(u64::from_le_bytes(id)),
            Err(_) => self.refused += 1,
        }
    }

    /// Transfers applied.
    pub fn applied(&self) -> u64 {
        self.applied
    }

    /// Root over balances in key order.
    pub fn root(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        for (a, b) in &self.balances {
            h.update(&a.to_le_bytes());
            h.update(&b.to_le_bytes());
        }
        h.update(&self.applied.to_le_bytes());
        *h.finalize().as_bytes()
    }
}
