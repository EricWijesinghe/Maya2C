//! On-chain governance: the proposal lifecycle, the vote tally, and the bounds
//! a proposal may never escape.
//!
//! # Why this is a crate and not a module
//!
//! The same argument that produced [`maya_ledger_math`] and `maya-dex`. What
//! this crate decides is which changes to the chain's own rules are
//! permissible, which makes it the last place a mistake should be hard to
//! check — and the [Kani Rust Verifier] compiles a crate together with its
//! whole dependency graph, so anything reaching RocksDB's C++ cannot be model
//! checked at all.
//!
//! So it has no dependencies, and the node calls into it rather than inlining
//! the rules. The visible cost is that nothing here hashes or stores: proposal
//! identifiers, authors, and the changes themselves are chain records, and this
//! crate sees only what it needs to decide what may happen next.
//!
//! # The one idea worth reading before the code
//!
//! **Governance must not be able to make governance unsafe.**
//!
//! A system that can change every rule can change the rules that protect the
//! process of changing rules, and the first move of a hostile majority is not
//! to steal — it is to remove the checks that would have let anyone react. So
//! the quorum floor, the approval floor, the minimum voting period, and the
//! minimum timelock live in [`limits`], are compiled into the binary, appear in
//! no [`params::ParameterKey`], and are reachable by no transaction. Changing
//! them is a release, which is to say it is a decision every node operator
//! makes individually by choosing what to run.
//!
//! Everything that *is* governable carries a hard range in [`params`], checked
//! when a proposal is made and again when it executes.
//!
//! # What self-amendment means here, precisely
//!
//! A governed rule is a `u64` in consensus state with an activation height.
//! That covers scalar limits, rates, resource bounds — and, using the same
//! table, flipping between two rules the binary already ships, which is how
//! `crypto::dag::registry` already decides whether a block's digest is
//! ArgonBlake or the DAG.
//!
//! It does **not** cover fetching native code from chain state and running it.
//! See [`params`] for why that is not a limitation to be worked around.
//!
//! [`maya_ledger_math`]: https://docs.rs/maya-ledger-math
//! [Kani Rust Verifier]: https://model-checking.github.io/kani/

// `no_std` in every build but the test harness, which needs `std` to run.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

pub mod error;
pub mod limits;
pub mod params;
pub mod proposal;
pub mod tally;

#[cfg(kani)]
pub mod proofs;

pub use error::GovernanceError;
pub use params::{Bounds, MAX_CHANGES_PER_PROPOSAL, ParameterChange, ParameterKey};
pub use proposal::{Proposal, ProposalState, Schedule};
pub use tally::{Choice, Tally};
