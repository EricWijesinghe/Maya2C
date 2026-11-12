//! One error type for the crate. The messages are fixed strings: the HTTP and
//! MCP layers return them to remote callers, so they carry no internals.

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("field out of bounds")]
    Bounds,
    #[error("signature does not verify")]
    Signature,
    #[error("expired")]
    Expired,
    #[error("signing key does not match the one named")]
    KeyMismatch,
    #[error("signing failed")]
    Signing,
    #[error("a newer card for this agent is already published")]
    Stale,
    #[error("registry is full")]
    Full,
    #[error("quote is already settled")]
    Replay,
    #[error("transfer does not match the quote or receipt")]
    Mismatch,
    #[error("quote is not addressed to this payee")]
    NotPayee,
    #[error("quote is not addressed to this payer")]
    NotPayer,
}

pub type Result<T> = core::result::Result<T, Error>;
