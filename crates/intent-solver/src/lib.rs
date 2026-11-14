//! Intent solver market with HTLC settlement (P-30, ADR-139).
//!
//! A maker signs an [`Intent`] — "`buy_amount` of asset A for at most
//! `max_sell` of asset B, before `deadline_height`" — with the hybrid
//! ML-DSA-65 + SLH-DSA-SHA2-128s pair every account already holds. Solvers
//! quote off chain; the [`IntentBook`] keeps the quotes in a deterministic
//! order and picks the best one. [`plan_settlement`] turns the winner into two
//! hash locks with staggered timeouts, and [`LegState`] tracks each lock with
//! the chain's own claim/refund rule.
//!
//! **Nothing here is consensus.** No new transaction form: settlement uses the
//! `HtlcLock`/`HtlcClaim`/`HtlcRefund` forms the chain has had since genesis
//! (hash locks, ADR-012). Matching happens off chain and the chain never sees
//! an intent, so a bad book can waste a maker's time but never move funds the
//! HTLC rule would not.
//!
//! Money is `U256` with checked arithmetic only: no floats, no 512-bit
//! intermediates. Heights, never timestamps (invariant 9).

pub mod book;
pub mod error;
pub mod intent;
pub mod settle;

pub use book::{IntentBook, MAX_INTENTS, MAX_QUOTES_PER_INTENT, MAX_SPENT_NONCES, Quote};
pub use error::{IntentError, Result};
pub use intent::{AssetId, Intent, IntentId, MAX_RECIPIENT_BYTES, SignedIntent};
pub use settle::{Action, Leg, LegState, Party, SettlementPlan, Step, plan_settlement};
