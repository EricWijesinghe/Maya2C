//! What the watcher reports.

use thiserror::Error;

use crate::policy::PairingError;

/// The result of anything here.
pub type Result<T> = std::result::Result<T, WatcherError>;

/// A watcher failure.
#[derive(Debug, Error)]
pub enum WatcherError {
    /// A node was unreachable or answered with something unusable. Worth
    /// retrying on the next tick.
    #[error("rpc: {0}")]
    Rpc(String),

    /// A chain answered and said no, or reported a lock that fails a check the
    /// watcher makes before committing funds. Not worth retrying as-is.
    #[error("refused: {0}")]
    Refused(String),

    /// The journal could not be read or written.
    #[error("journal {path}: {reason}")]
    Journal {
        /// The journal file.
        path: String,
        /// What went wrong.
        reason: String,
    },

    /// A transaction could not be signed.
    #[error("signing: {0}")]
    Signing(String),

    /// The two timelocks leave too little margin.
    #[error("unsafe pairing: {0}")]
    Pairing(#[from] PairingError),

    /// A commitment already journalled. A published opening claims every lock
    /// under its commitment, so a secret is single-use.
    #[error("commitment {0} is already journalled; a swap secret is single-use")]
    DuplicateSwap(String),
}
