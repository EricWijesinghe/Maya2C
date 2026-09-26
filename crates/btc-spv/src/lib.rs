//! Bitcoin SPV header tracking (Master Prompt 6 §3).
//!
//! Enough of Bitcoin's consensus to decide, from headers alone, which chain
//! has the most work and how deeply a transaction is buried in it:
//! the 80-byte header and its double-SHA-256, the compact target encoding
//! bit-exact with Bitcoin Core, the 2016-block retarget with its 4x clamp,
//! median-time-past, most-chainwork fork choice with reorgs, and merkle
//! inclusion proofs that refuse the 64-byte-transaction and duplicate-node
//! tricks.
//!
//! # Trust model
//!
//! An SPV client trusts that the most-work chain is valid — it does not
//! check scripts or UTXOs. A deposit is therefore as safe as the work an
//! attacker would have to out-pace: `tests/spv_tests.rs` shows a six-block
//! reorg displacing a deposit that was five blocks deep, and the same deposit
//! surviving once it is buried deeper than the attacker's chain. The
//! confirmation depth is a policy the bridge chooses and states; it is not a
//! proof. `docs/crosschain.md` has the table.
//!
//! RESEARCH: nothing in the node calls it. It is the header half of a Bitcoin
//! light client; the lock/mint path that would use it is Master Prompt 25.

#![warn(missing_docs)]

mod chain;
mod header;
mod merkle;
mod u256;

pub use chain::{HeaderChain, HeaderError, Params};
pub use header::{
    BlockHash, HEADER_LEN, Header, compact_from_target, sha256d, target_from_compact,
    work_from_target,
};
pub use merkle::{MerkleProof, merkle_root, prove};
pub use u256::U256;
