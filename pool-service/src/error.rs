//! Pool errors.
//!
//! Split the way `explorer/src/error.rs` splits its own: by the operational
//! response, not by the module that raised it. An on-call engineer reading
//! `ledger:` reaches for the disk, and one reading `node rpc:` reaches for the
//! chain. A single `Internal(String)` would have told them neither.
//!
//! [`PoolError::Treasury`] is the variant worth pausing on. It covers every way
//! the payout path can refuse to spend — an unfunded account, a locked
//! keystore, a spend cap reached — and all of them mean *stop paying*, never
//! *retry harder*. Folding them into a generic failure would let a retry loop
//! treat "the treasury is empty" as a transient condition.

use thiserror::Error;

/// Something the pool could not do.
#[derive(Debug, Error)]
pub enum PoolError {
    /// The share ledger failed.
    ///
    /// A durable store the pool cannot write to means credits are being earned
    /// and not recorded, which is the one failure that must stop the daemon
    /// rather than degrade it.
    #[error("ledger: {0}")]
    Ledger(String),

    /// The node's JSON-RPC endpoint could not be reached or refused a request.
    ///
    /// Distinguished from a ledger failure because the responses differ: this
    /// one means work templates are stale and blocks cannot be submitted, but
    /// the shares already accepted are safe.
    #[error("node rpc: {0}")]
    NodeRpc(String),

    /// The payout path refused to spend.
    ///
    /// Never retried automatically. Every case here is a policy or custody
    /// condition that a retry cannot clear.
    #[error("treasury: {0}")]
    Treasury(String),

    /// A peer's frame or message was malformed.
    ///
    /// Carries the connection's own protocol error so a log line names what
    /// arrived, without the pool having to reproduce it.
    #[error("protocol: {0}")]
    Protocol(#[from] maya_stratum_v2::Sv2Error),

    /// The chain rejected something, or a chain primitive failed.
    #[error("chain: {0}")]
    Chain(String),

    /// Configuration was absent, malformed, or internally inconsistent.
    ///
    /// Only ever raised before the daemon starts serving. A pool that starts
    /// with an incoherent payout policy is worse than one that refuses to
    /// start, because the first pays wrong amounts and the second pays none.
    #[error("config: {0}")]
    Config(String),

    /// A worker, rig, or payout that does not exist was requested.
    #[error("not found: {0}")]
    NotFound(String),

    /// A user-supplied API parameter was malformed.
    #[error("bad request: {0}")]
    BadRequest(String),

    /// A listener could not bind, or a server could not start.
    #[error("server: {0}")]
    Server(String),
}

impl From<custom_l1_node::NodeError> for PoolError {
    fn from(error: custom_l1_node::NodeError) -> Self {
        Self::Chain(error.to_string())
    }
}

impl From<std::io::Error> for PoolError {
    fn from(error: std::io::Error) -> Self {
        Self::Server(error.to_string())
    }
}

impl From<rocksdb::Error> for PoolError {
    fn from(error: rocksdb::Error) -> Self {
        Self::Ledger(error.to_string())
    }
}

/// Convenience alias.
pub type Result<T> = core::result::Result<T, PoolError>;
