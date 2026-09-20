//! Stateless transfer execution over a keyed sparse Merkle vector commitment.
//!
//! A stateless node holds a header chain and nothing else. To check a block it
//! takes a **witness** — the accounts the block touches, and every sibling
//! digest on their paths — verifies it against the parent header's state root,
//! runs the transfers against it, and recomputes the root the block's header
//! must declare. No database is read: this crate cannot reach one.
//!
//! # Pieces
//!
//! - [`compress`]: the node function. [`compress::Blake3`] is what the node
//!   commits with; [`lattice::RingSis`] is the research backend over
//!   `R_q = Z_q[X]/(X^n + 1)`.
//! - [`sparse`]: the tree shape, [`sparse::root`] and [`sparse::open`].
//! - [`partial`]: the decoded witness, verification, reads and writes.
//! - [`transition`]: one transfer, by the node's rules.
//!
//! # What the brief asked for and what this is
//!
//! The brief asked for a lattice polynomial commitment with sub-kilobyte
//! openings. No published lattice vector commitment with transparent setup
//! and standard assumptions opens in under a kilobyte at 128-bit security; the
//! 2024 constructions report kilobytes to hundreds of kilobytes. A Ring-SIS
//! hash tree opens in `448 × depth` bytes. So the lattice backend is here,
//! tested, and dark, and the node commits with BLAKE3 at `33 × depth` — about
//! 0.7 KB per key at a million accounts. `docs/stateless.md` has the figures
//! and what they rest on.
//!
//! # What a witness cannot do
//!
//! Only transfers. A contract reads storage no witness can predict, a DEX
//! batch reads every pool it clears, and the end-of-block passes read state
//! the block never names. The node decides eligibility; this crate executes
//! what it is given.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod compress;
pub mod error;
pub mod lattice;
pub mod params;
pub mod partial;
pub mod sparse;
pub mod transition;

pub use compress::{Blake3, Compress, Key, Value};
pub use error::{Defect, Error, Result, Violation};
pub use lattice::RingSis;
pub use partial::{Node, PartialTree, VerifiedTree};
pub use transition::{AccountState, apply_transfer};
