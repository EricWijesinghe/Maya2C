//! Why the miner could not start or could not run.

/// Failures a GPU miner can report.
#[derive(Debug, thiserror::Error)]
pub enum MinerError {
    /// wgpu found no adapter, or `--device` named one that does not exist.
    #[error("no GPU adapter available")]
    NoAdapter,

    /// The dataset needs more storage bindings than the shader declares.
    ///
    /// Reported rather than worked around. Silently mining a truncated dataset
    /// would produce digests that differ from the node's, and the miner would
    /// look like it was working until the first solution was rejected.
    #[error("dataset needs {needed} storage bindings; the shader declares {available}")]
    DatasetTooLarge {
        /// Bindings the dataset would require on this adapter.
        needed: u64,
        /// Bindings the shader has.
        available: u64,
    },

    /// The device could not be created.
    #[error("creating GPU device: {0}")]
    Device(String),

    /// The result buffer could not be read back.
    #[error("reading back results: {0}")]
    Readback(String),
}
