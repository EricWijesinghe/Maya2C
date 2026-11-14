//! What the intent market refuses.

use thiserror::Error;

/// The result of anything here.
pub type Result<T> = std::result::Result<T, IntentError>;

/// A refusal. Every one is a value, never a panic: the book and the planner
/// run on input a stranger wrote.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum IntentError {
    /// The intent's fields contradict themselves.
    #[error("malformed intent: {0}")]
    Malformed(&'static str),
    /// Signed for another network.
    #[error("intent is for another network")]
    WrongNetwork,
    /// The public key does not belong to the named maker.
    #[error("public key does not match the maker address")]
    MakerMismatch,
    /// The hybrid signature failed (either half).
    #[error("intent signature does not verify")]
    BadSignature,
    /// The maker already spent this nonce on an intent.
    #[error("intent replayed: this maker's nonce is already used")]
    Replay,
    /// At or past the deadline height.
    #[error("intent expired")]
    Expired,
    /// No open intent has this id.
    #[error("unknown intent")]
    UnknownIntent,
    /// The intent is open but nobody has quoted it.
    #[error("no quote for this intent")]
    NoQuote,
    /// The quote costs the maker more than its limit.
    #[error("quote is worse than the intent's limit")]
    WorseThanLimit,
    /// The same solver already quoted at least as well.
    #[error("quote does not improve this solver's earlier quote")]
    NotAnImprovement,
    /// 256-bit amount or 64-bit height arithmetic would overflow.
    #[error("arithmetic overflow")]
    Overflow,
    /// The timelocks leave no room for both locks and both claims.
    #[error("no room between now and the deadline for the settlement margin")]
    NoRoom,
    /// A leg on another chain needs a SHA-256 lock.
    #[error("a leg on another chain needs a SHA-256 hash lock")]
    HashNotPortable,
    /// A bound on the in-memory book was reached.
    #[error("intent book is full")]
    Full,
}
