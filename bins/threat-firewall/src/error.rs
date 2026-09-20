//! What the worker reports.

use thiserror::Error;

/// The result of anything here.
pub type Result<T> = std::result::Result<T, FirewallError>;

/// A worker failure.
#[derive(Debug, Error)]
pub enum FirewallError {
    /// The node was unreachable or answered with something unusable. Worth
    /// retrying on the next tick.
    #[error("rpc: {0}")]
    Rpc(String),

    /// A firewall command failed. The rule is left out of the enforced set so
    /// the next tick tries again.
    #[error("sink: {0}")]
    Sink(String),

    /// The worker was configured with something it refuses to run with.
    #[error("config: {0}")]
    Config(String),
}
