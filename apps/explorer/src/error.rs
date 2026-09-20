//! Explorer errors.

use thiserror::Error;

/// Something the explorer could not do.
#[derive(Debug, Error)]
pub enum ExplorerError {
    /// The backing store failed.
    #[error("store: {0}")]
    Store(String),

    /// The node's RPC endpoint could not be reached or refused a request.
    ///
    /// Distinguished from a store failure because the two have different
    /// operational responses: one means the database is unhealthy, the other
    /// means the chain is unreachable and indexing has simply stalled.
    #[error("node rpc: {0}")]
    NodeRpc(String),

    /// A height, hash, or address was requested that does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// A user-supplied parameter was malformed.
    #[error("bad request: {0}")]
    BadRequest(String),

    /// The HTTP server could not start.
    #[error("server: {0}")]
    Server(String),
}

impl From<sqlx::Error> for ExplorerError {
    fn from(error: sqlx::Error) -> Self {
        Self::Store(error.to_string())
    }
}

/// Convenience alias.
pub type Result<T> = core::result::Result<T, ExplorerError>;
