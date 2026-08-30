//! Core chain data structures.

pub mod block;
pub mod codec;
pub mod payload;
pub mod transaction;

pub use block::{Block, BlockHeader, HEADER_LEN};
pub use payload::{
    ChannelClosure, ChannelId, ChannelOpen, ContractCall, ContractDeploy, RevocationProof, TxKind,
};
pub use transaction::{Transaction, TxInput, TxOutput};
