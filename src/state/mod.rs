//! Persistent account state: RocksDB-backed storage, the account state
//! transition rules, and the Merkle state root.
//!
//! ## Account model
//!
//! State transitions are account-based: a transaction's `public_key` is the
//! sender, its `outputs` are credits, and its `nonce` is that sender's sequence
//! number. [`Transaction::inputs`] belongs to a UTXO model and is carried
//! through the structure but not consumed here; spending authority comes from
//! the signature plus the nonce, not from referenced outputs.
//!
//! [`Transaction::inputs`]: crate::core::Transaction::inputs

pub mod account;
pub mod channel;
pub mod context;
pub mod contracts;
pub mod db;
pub mod merkle;
pub mod settlement;
pub mod shielded;
pub mod undo;
pub mod vm_exec;

pub use account::{ACCOUNT_LEN, Account, Address};
pub use channel::{ChannelRecord, ChannelStatus};
pub use context::BlockContext;
pub use db::StateDB;
pub use merkle::{account_leaf, merkle_root};
pub use shielded::{MAX_SHIELDED_PER_BLOCK, ShieldedPool};
pub use undo::{UndoEntry, UndoRecord};
