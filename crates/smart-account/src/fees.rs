//! Fees that do not get in the way (Master Prompt 22 §3).
//!
//! Three ways to pay, one guarantee: the quoted maximum is never exceeded.
//!
//! - **Native**: up to `max_fee` of the native token.
//! - **Token**: an approved token converted at an oracle price with a safety
//!   margin in the payer's disfavour (so a price move between quote and
//!   inclusion is absorbed by the margin, not by the protocol).
//! - **Sponsored**: an app's paymaster pays, within its own per-account daily
//!   cap and total budget — the abuse protection.

use std::collections::BTreeMap;

use crate::AccountId;

/// Safety margin applied to token-fee conversion, basis points.
pub const TOKEN_MARGIN_BPS: u64 = 500;

/// How an op pays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeeSpec {
    /// Native token, at most `max_fee`.
    Native {
        /// Ceiling.
        max_fee: u64,
    },
    /// An approved token, at most `max_amount` of it.
    Token {
        /// Token id.
        token: u32,
        /// Ceiling in token units.
        max_amount: u64,
    },
    /// Paid by a paymaster account.
    Sponsored {
        /// The sponsor.
        paymaster: AccountId,
    },
}

impl FeeSpec {
    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Native { max_fee } => {
                out.push(1);
                out.extend_from_slice(&max_fee.to_le_bytes());
            }
            Self::Token { token, max_amount } => {
                out.push(2);
                out.extend_from_slice(&token.to_le_bytes());
                out.extend_from_slice(&max_amount.to_le_bytes());
            }
            Self::Sponsored { paymaster } => {
                out.push(3);
                out.extend_from_slice(paymaster);
            }
        }
    }
}

/// An oracle price: native units per token unit, as a ratio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenPrice {
    /// Numerator (native units).
    pub native: u64,
    /// Denominator (token units).
    pub token: u64,
}

impl TokenPrice {
    /// Token units needed to cover `native_fee`, rounded up, plus the margin.
    pub fn token_cost(&self, native_fee: u64) -> Option<u64> {
        if self.native == 0 {
            return None;
        }
        let base = (u128::from(native_fee) * u128::from(self.token)).div_ceil(u128::from(self.native));
        let with_margin = (base * u128::from(10_000 + TOKEN_MARGIN_BPS)).div_ceil(10_000);
        u64::try_from(with_margin).ok()
    }
}

/// A sponsor's rules and running totals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paymaster {
    /// Accounts it sponsors (empty = anyone).
    pub sponsored: Vec<AccountId>,
    /// Most it pays for one account per policy day.
    pub per_account_daily: u64,
    /// Budget left, native units.
    pub budget: u64,
    /// `(account, day)` → spent.
    pub spent: BTreeMap<(AccountId, u64), u64>,
}

impl Paymaster {
    /// Charges `fee` for `account` on `day`, or refuses.
    pub fn charge(&mut self, account: &AccountId, day: u64, fee: u64) -> bool {
        if !self.sponsored.is_empty() && !self.sponsored.contains(account) {
            return false;
        }
        let spent = self.spent.get(&(*account, day)).copied().unwrap_or(0);
        let Some(next) = spent.checked_add(fee) else { return false };
        if next > self.per_account_daily || fee > self.budget {
            return false;
        }
        self.budget -= fee;
        self.spent.insert((*account, day), next);
        true
    }
}
