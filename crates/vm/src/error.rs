//! VM errors.

use thiserror::Error;

/// Why a contract execution did not complete normally.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum VmError {
    /// The module bytes are not valid WebAssembly, or use a disabled feature.
    ///
    /// Rejecting at load rather than at first use matters: a module that traps
    /// only on some paths would let a deployer smuggle in a feature the chain
    /// does not permit.
    #[error("invalid module: {0}")]
    InvalidModule(String),

    /// The module imports something the host does not provide.
    #[error("unresolved import: {0}")]
    UnresolvedImport(String),

    /// The module does not export the entry point the ABI requires.
    #[error("missing export `{0}`")]
    MissingExport(String),

    /// Execution ran out of gas.
    ///
    /// Distinguished from a general trap because it is the *expected* outcome
    /// of an over-long computation rather than a fault, and callers frequently
    /// need to tell the two apart.
    #[error("out of gas: limit was {limit}")]
    OutOfGas {
        /// Gas the call was allowed.
        limit: u64,
    },

    /// The guest trapped.
    #[error("trap: {0}")]
    Trap(String),

    /// A pointer or length from the guest fell outside its linear memory.
    ///
    /// The guest cannot reach host memory regardless — this is the host
    /// refusing to read from a bad offset *inside* the sandbox.
    #[error("memory access out of bounds: offset {offset}, length {length}")]
    MemoryOutOfBounds {
        /// Requested offset.
        offset: u32,
        /// Requested length.
        length: u32,
    },

    /// The guest attempted to grow memory past the configured ceiling.
    #[error("memory limit exceeded: {requested} pages, limit {limit}")]
    MemoryLimit {
        /// Pages requested.
        requested: usize,
        /// Configured ceiling.
        limit: usize,
    },

    /// A host function argument was malformed.
    #[error("invalid host call: {0}")]
    InvalidHostCall(String),

    /// A `maya_res` import broke a resource rule (`crate::resources`); the
    /// whole call is reverted.
    #[error("resource rule: {0:?}")]
    Resource(maya_contract_safety::Fault),

    /// The engine itself could not be configured.
    ///
    /// Fatal: a node that cannot build the deterministic configuration must not
    /// fall back to a permissive one.
    #[error("engine configuration failed: {0}")]
    EngineConfig(String),

    /// A storage key or value exceeded its permitted size.
    #[error("{what} of {actual} bytes exceeds the limit of {limit}")]
    SizeLimit {
        /// What was too large.
        what: &'static str,
        /// Size supplied.
        actual: usize,
        /// Permitted maximum.
        limit: usize,
    },

    /// A contract called `host_verify_zkml_proof` where no verifier is wired
    /// in: below the activation height, or under a host that has none.
    ///
    /// A trap rather than a `0`. Returning "invalid" would let a contract
    /// written for a live verifier silently treat every proof as bad on a node
    /// where the feature is off — a divergence in behaviour that looks like a
    /// divergence in data.
    #[error("zkML verification is not available at this height")]
    ZkmlUnavailable,
}

/// Convenience alias.
pub type Result<T> = core::result::Result<T, VmError>;
