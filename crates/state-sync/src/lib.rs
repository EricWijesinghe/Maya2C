//! State sync (Master Prompt 14 §2).
//!
//! A snapshot is the state at an epoch boundary, cut into chunks. Its
//! [`Manifest`] root — a BLAKE3 Merkle root over the chunk hashes — is the
//! value a block header commits to at that boundary. A joining node:
//!
//! 1. learns the root from a finalized header (never from a peer);
//! 2. asks many peers for chunks in parallel ([`Downloader`]);
//! 3. verifies **each chunk on arrival** against the root with its Merkle
//!    path, so a bad chunk is caught immediately, costs one chunk of
//!    bandwidth, and bans the peer that sent it;
//! 4. replays the blocks since the snapshot height (warp catch-up) — the
//!    node's existing `state_pruner::bootstrap_pruned` path.
//!
//! The existing node bootstrap verifies the *whole* import against
//! `header.state_root` at the end; this adds per-chunk verification so a
//! single malicious peer cannot waste the whole download. Both checks stay:
//! this root binds chunks; the state root binds meaning.
//!
//! RESEARCH: the node does not commit a snapshot root in its header yet (that
//! is a consensus change with a spec section); the downloader and the proofs
//! are complete and simulated at 100 million accounts.

#![warn(missing_docs)]

mod download;
mod manifest;
pub mod sim;
mod synthetic;

pub use download::{Downloader, Event, Request};
pub use manifest::{ChunkProof, Manifest, chunk_hash};
pub use synthetic::{ACCOUNT_BYTES, SyntheticState};
