//! Hash-time-locked contracts: hash locks (REAL) and lattice locks (HTLC-L,
//! RESEARCH).
//!
//! [`Lock`] is either a 256-bit digest of a 32-byte preimage — SHA3-256,
//! BLAKE3, or SHA-256, the one Bitcoin and Ethereum can check — or a
//! Module-LWE commitment. Hash locks are already post-quantum (Grover needs
//! about 2^128 sequential evaluations) and are live on the node from genesis;
//! everything below about the lattice lock is the RESEARCH family, dark until
//! its activation height. ADR-012.
//!
//! A lattice lock escrows value under a commitment `t = A·s + e` and an expiry height.
//! Before the expiry, whoever publishes a short `(s, e)` that opens `t` pays the
//! lock's named recipient; from the expiry on, a refund repays the sender.
//! Two locks on two chains under **one** commitment are an atomic swap: the
//! claim that pays one side publishes the opening that pays the other.
//!
//! ## What this replaces, and what it does not
//!
//! The classical HTLC locks under `SHA-256(preimage)`. The case for replacing
//! it is not that a quantum computer finds SHA-256 preimages — Grover's
//! algorithm needs about 2^128 sequential evaluations, which is not a threat
//! anyone plans around, and a SHA3 hashlock is already post-quantum. The case
//! is uniformity: a claim here rests on Module-LWE at ML-DSA-65's parameters,
//! the same assumption every signature on this chain already rests on, rather
//! than adding a second one. See [`params`].
//!
//! What it does **not** do is swap with Bitcoin. An atomic swap needs both
//! chains to check the same predicate, and a Bitcoin script cannot check this
//! one. A classical hashlock on one side and a lattice lock on the other are
//! two unrelated secrets, and nothing makes revealing one reveal the other.
//! HTLC-L is a swap between chains that run this verifier. `docs/htlc-lattice.md`.
//!
//! ## Chain-free
//!
//! Nothing here knows what a block is. The node turns [`LockRecord`]s into state
//! and [`timelock`] outcomes into no-ops; the watcher in `htlc-watcher` reads
//! the same records over RPC.

pub mod commitment;
pub mod error;
pub mod lock;
pub mod matrix;
pub mod opening;
pub mod params;
pub mod record;
pub mod secret;
pub mod timelock;

#[cfg(kani)]
mod proofs;

pub use commitment::{Commitment, CommitmentId, centered};
pub use error::{Error, Result};
pub use lock::{HashFunction, Lock, PREIMAGE_BYTES, Preimage, SwapSecret, Unlock};
pub use matrix::Matrix;
pub use opening::Opening;
pub use params::{COMMITMENT_BYTES, DIGEST_BYTES, ETA, OPENING_BYTES, SEED_BYTES};
pub use record::{Address, LockRecord, Settlement};
pub use secret::LatticeSecret;
pub use timelock::{
    ClaimOutcome, LockStatus, RefundOutcome, claim_window_open, decide_claim, decide_refund,
};
