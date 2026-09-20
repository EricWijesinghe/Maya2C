//! Lattice proof-of-work verification for `custom-l1-node`: does this vector
//! lie in the block's lattice, and is it short enough?
//!
//! # Status
//!
//! **This crate is a research branch. Nothing in the node calls it, and the
//! consensus rule it would serve is not wired to any activation height.** It
//! exists so that the verification half of a lattice proof of work can be
//! written, proved, and reviewed independently of the question of whether such
//! a proof of work is *sound*, which it is not yet known to be. See
//! `docs/lattice-pow.md` for what is unresolved; the short version is in
//! [Why this is not yet a consensus rule](#why-this-is-not-yet-a-consensus-rule).
//!
//! # Why this is a crate and not a module
//!
//! The same argument that produced [`maya_ledger_math`] and [`maya_dex`]. Every
//! function here decides whether a block is admissible, and a bug in one of
//! them either accepts a block nobody paid for or rejects one somebody did.
//! That is the class of code worth proving, and the [Kani Rust Verifier]
//! compiles a crate *together with its whole dependency graph* — which rules
//! out anything reaching RocksDB's C++ or the `ark-*` stack.
//!
//! The visible cost of that boundary is that nothing here hashes. The basis
//! coefficients arrive as plain integers; expanding them from a block header is
//! the node's job, in `custom-l1-node`'s `crypto::lattice`.
//!
//! # The lattice
//!
//! A Goldstein–Mayer random `q`-ary lattice in Hermite normal form — the family
//! the Darmstadt SVP challenges are drawn from. For dimension `n` and modulus
//! `q`, the basis rows are
//!
//! ```text
//! b_0 = ( q,   0, 0, …, 0 )
//! b_i = ( x_i, 0, …, 1, …, 0 )      1 <= i < n, the 1 in column i
//! ```
//!
//! so the whole lattice is named by `q` and the `n - 1` coefficients `x_i`, and
//! `det(L) = q`. That compactness is the point: a basis that would otherwise be
//! `n²` integers on the wire is `n - 1`.
//!
//! # Membership is a divisibility test, not a matrix solve
//!
//! Writing `v = Σ c_i b_i` gives `v_i = c_i` for `i >= 1` and
//! `v_0 = c_0·q + Σ_{i>=1} c_i·x_i`. Eliminating `c` leaves
//!
//! ```text
//! v ∈ L   ⟺   v_0 ≡ Σ_{i=1}^{n-1} x_i · v_i   (mod q)
//! ```
//!
//! This is why the miner sends `v` and not its coefficient vector: the
//! coefficients are recoverable, so carrying them would be redundant bytes a
//! miner could vary to grind. Verification is one pass of modular arithmetic in
//! exact integers — no floating point, no reduction, no Gram–Schmidt.
//!
//! That asymmetry is the whole reason this shape of proof of work is checkable
//! at all: finding a short `v` is the miner's exponential problem, and
//! confirming one is `O(n)` additions.
//!
//! # Determinism
//!
//! Every function is a pure function of its inputs. There is no floating point,
//! no hashing, no allocation-order dependence, and no time source.
//!
//! This matters more here than it does elsewhere in the tree. Lattice
//! *reduction* — LLL, BKZ — runs on floating-point Gram–Schmidt and is not
//! reproducible across platforms, compilers, or library versions. If validation
//! re-ran reduction, the chain would fork on a rounding difference. It does
//! not: the validator never reduces anything. It checks a congruence and a sum
//! of squares, both in exact integer arithmetic, and both bounded by
//! [`MAX_COORDINATE`] so that neither can overflow.
//!
//! # Why this is not yet a consensus rule
//!
//! Recorded here rather than in a design document nobody opens, because the
//! next person to read this crate will otherwise assume the hard part is done.
//!
//! Lattice reduction is **not progress-free**. Nakamoto consensus assumes each
//! mining attempt is an independent trial, so a miner who has worked for nine
//! minutes is no closer than one starting now. BKZ is monotone optimisation:
//! elapsed work strictly improves the basis. A large miner therefore finishes
//! reductions a small miner must abandon, reward becomes superlinear in miner
//! size, and cumulative work stops measuring expected trials — which is the
//! quantity `consensus::difficulty` compares branches by.
//!
//! Nothing in this crate fixes that, and nothing in this crate pretends to.
//! What it provides is the half of the problem that *is* well-posed.
//!
//! [`maya_ledger_math`]: https://docs.rs/maya-ledger-math
//! [`maya_dex`]: https://docs.rs/maya-dex
//! [Kani Rust Verifier]: https://model-checking.github.io/kani/

// `no_std` in every build but the test harness, which needs `std` to run.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

pub mod basis;
pub mod error;
pub mod params;
pub mod verify;

#[cfg(kani)]
pub mod proofs;

pub use basis::Basis;
pub use error::{LatticeError, Result};
pub use params::{LatticeParams, MAX_DIMENSION, MIN_DIMENSION};
pub use verify::{MAX_COORDINATE, norm_squared, verify_solution};
