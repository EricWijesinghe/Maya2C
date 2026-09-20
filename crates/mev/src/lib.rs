//! A mempool a miner can order but cannot read.
//!
//! # The problem
//!
//! A miner choosing the order of a block sees every transaction's contents
//! first. That is not a bug in any one implementation; it is what "the miner
//! assembles the block" means. It is also the whole supply of maximal
//! extractable value: front-running a large swap, sandwiching it on both sides,
//! reordering a liquidation to win the collateral. `dex`'s uniform-price batch
//! clearing removes the *profit* from reordering swaps within a pair, which is
//! a large share of the problem and not all of it — a miner who reads a
//! transaction can still delay it, censor it, or race it with one of their own.
//!
//! # The approach
//!
//! Encrypt each transaction to a committee key with a `t`-of-`n` threshold. A
//! miner includes ciphertext, so the order is fixed while the contents are
//! unknown. After the block commits, `t` committee members publish decryption
//! shares, anyone recombines them, and the transactions execute in the order
//! that was already settled.
//!
//! The property this buys, stated exactly: **ordering is committed before
//! contents are known.** It is not confidentiality — everything is public a
//! block later — and it is not censorship resistance, since a miner can still
//! drop a ciphertext it cannot read. It removes content-dependent ordering,
//! which is the specific power MEV extraction runs on.
//!
//! # What it costs, in the order the costs bite
//!
//! **Liveness depends on the committee.** If fewer than `t` members publish
//! shares, sealed transactions cannot be opened. This crate's answer is that
//! they then **expire unopened** — the nonce does not advance, nothing moves,
//! the sender resubmits.
//!
//! The tempting alternative is a plaintext fallback: after some deadline, let
//! the sender reveal in the clear and execute anyway. That is worse, and the
//! reason is worth being precise about. A miner who wants to read a transaction
//! would simply not include the shares, wait out the deadline, and read it —
//! turning the fallback into a switch the attacker controls. A scheme whose
//! safety property can be waited out does not have that property. Expiry fails
//! toward *nothing happening*, which is recoverable; the fallback fails toward
//! *the attack succeeding*, which is not.
//!
//! Sealing is opt-in besides. A sender who needs guaranteed inclusion more than
//! they need protection sends an ordinary transaction and always could.
//!
//! **The setup is trusted.** [`Committee::generate`] holds the whole key for
//! the length of one function call. Distributed key generation removes that and
//! is not built — see the method's own documentation for what a compromised
//! setup does and does not cost.
//!
//! **It is a classical primitive.** ristretto255 falls to a quantum adversary,
//! on a chain whose signatures and transport are post-quantum precisely because
//! that adversary is assumed to arrive. This is a deliberate, bounded
//! exception: the confidentiality here has a lifetime of roughly one block, and
//! an adversary able to run Shor's algorithm against a ciphertext within one
//! block interval is an adversary who has already broken far more urgent things
//! elsewhere. Post-quantum threshold encryption exists in the literature and
//! not in a standardized, audited Rust implementation; when one lands, this is
//! the crate to replace. The rest of the chain does not depend on this
//! assumption — a break here costs MEV protection and nothing else.
//!
//! # Layout
//!
//! - [`shamir`] — splitting a scalar so that any `t` of `n` pieces rebuild it.
//! - [`committee`] — the public key, the member keys, and the setup.
//! - [`cipher`] — sealing, decryption shares and their proofs, and combining.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod cipher;
pub mod committee;
pub mod error;
pub mod shamir;

pub use cipher::{DecryptionShare, SealedPayload, ShareProof, combine, seal, verify_share};
pub use committee::{Committee, MemberKey, MemberSecret};
pub use error::MevError;
