//! The trading engine for `custom-l1-node`: a constant-product AMM and a
//! price-time limit order book, with no I/O and no chain types.
//!
//! # Why this is a crate and not a module
//!
//! The same argument that produced [`maya_ledger_math`]. Every function here
//! decides how much value moves, and a rounding error in one of them is a mint
//! or a burn rather than a cosmetic defect. That is the class of code worth
//! proving, and the [Kani Rust Verifier] compiles a crate *together with its
//! whole dependency graph* — which rules out anything that reaches `RocksDB`'s
//! C++ or the `ark-*` stack.
//!
//! So this crate has no dependencies, and the node calls into it rather than
//! inlining the arithmetic. What `proofs` (compiled only under Kani) establishes is a property of the
//! code that runs when a block is applied, not of a restatement of it.
//!
//! The visible cost of that boundary is that nothing here hashes. Pair
//! identifiers, order identifiers, and LP asset identifiers arrive as opaque
//! 32-byte values; deriving them is the node's job.
//!
//! # The two venues
//!
//! [`amm`] is a constant-product pool: `x * y >= k`, fees retained in the
//! reserves. [`book`] and [`matching`] are a limit order book cleared by
//! price-time priority at the resting order's price.
//!
//! They are two independent venues on the same pair, not one routed venue. A
//! swap reaches the pool; a limit order rests in the book; the block-end
//! matching pass crosses the book against itself. Routing a single order across
//! both — taking book liquidity up to the pool's marginal price and the
//! remainder from the curve — is a real feature and is deliberately *not*
//! implemented here, because a half-built router that silently picks the worse
//! venue is worse than no router. See `docs/dex.md`.
//!
//! # Determinism
//!
//! Every function is a pure function of its inputs. There is no floating point,
//! no hashing, no iteration order that depends on an address's bit pattern, and
//! no time source. Two nodes handed the same inputs produce the same trades, or
//! the chain forks.
//!
//! # Rounding
//!
//! One rule, applied without exception: **when a division is inexact, the
//! remainder goes to the pool.** A trader receiving output gets a floor; a
//! trader supplying input pays a ceiling. The alternative is not "fairer", it
//! is a slow leak that a bot can drain in a loop.
//!
//! [`maya_ledger_math`]: https://docs.rs/maya-ledger-math
//! [Kani Rust Verifier]: https://model-checking.github.io/kani/

// `no_std` in every build but the test harness, which needs `std` to run.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

pub mod amm;
pub mod batch;
pub mod book;
pub mod error;
pub mod fees;
pub mod matching;
pub mod math;
pub mod types;

#[cfg(kani)]
pub mod proofs;

pub use amm::{LiquidityDelta, Pool, SwapOutcome};
pub use batch::{BatchOutcome, ClearedTrade, SwapIntent, clear_batch};
pub use book::{Book, Order, OrderId, Side};
pub use error::DexError;
pub use fees::FeeSchedule;
pub use matching::{Fill, MatchLimits, MatchOutcome, match_book};
pub use types::{AssetId, Direction, PRICE_SCALE, PairId};
