//! Why the GPU half could not answer. None of these is a verdict.

/// Failures of a lattice backend.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// wgpu found no adapter.
    #[error("no GPU adapter available")]
    NoAdapter,
    /// The device could not be created, or was lost.
    #[error("GPU device: {0}")]
    Device(String),
    /// A dispatch did not finish inside the bounded wait.
    #[error("GPU dispatch did not finish inside the bounded wait")]
    Timeout,
    /// The output did not carry this batch's nonce and indices (threat model G-9).
    #[error("refused a stale or misplaced GPU result: {0}")]
    StaleBuffer(&'static str),
    /// More items than one dispatch may carry.
    #[error("batch of {0} exceeds the per-dispatch bound")]
    BatchTooLarge(usize),
}
