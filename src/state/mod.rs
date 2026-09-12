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
pub mod asset;
pub mod blocks;
pub mod channel;
pub mod commitments;
pub mod context;
pub mod contracts;
pub mod db;
pub mod dex;
pub mod dex_exec;
pub mod governance_exec;
pub mod invariant_guard;
pub mod merkle;
pub mod oracle_exec;
pub mod proof;
pub mod sealed_exec;
pub mod settlement;
pub mod shielded;
pub mod undo;
pub mod vm_exec;
pub mod zkml;

// Re-exported so that a caller wiring up a trade does not have to take a
// direct dependency on `maya-dex` to name a direction or a side. The engine is
// an implementation detail of this crate's state transition; its vocabulary is
// not.
pub use maya_dex::amm::Pool;
pub use maya_dex::book::{Order, OrderId, Side};
pub use maya_dex::types::{Direction, NATIVE_ASSET, PRICE_SCALE};

pub use account::{ACCOUNT_LEN, Account, Address};
pub use asset::{AssetId, AssetRecord, derive_asset_id};
pub use channel::{ChannelRecord, ChannelStatus};
pub use context::BlockContext;
pub use db::StateDB;
pub use dex::{OrderRecord, PoolRecord, canonical_pair, derive_lp_asset, derive_pair_id};
pub use invariant_guard::{BreakerRecord, Invariant, MODULES, Module};
pub use merkle::{PathStep, account_leaf, merkle_path, merkle_root, verify_path};
pub use proof::{AccountProof, LayerDigest, StateLayer};
pub use shielded::{MAX_SHIELDED_PER_BLOCK, ShieldedPool};
pub use undo::{UndoEntry, UndoRecord};
