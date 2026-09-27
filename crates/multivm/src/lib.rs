//! Multi-VM Phase A (Master Prompt 5 §3): foreign engines over one account
//! state, with one unit of cost.
//!
//! - [`evm`] hosts revm: EVM bytecode deployed and called by Maya accounts.
//! - [`sbf`] hosts solana-sbpf: Solana SBF programs interpreted against the
//!   same accounts.
//! - [`state`] is the **unified state mapping**: every engine sees a Maya
//!   account through one derivation (`evm_address`) and one balance.
//! - [`gas`] is the **gas conversion**: EVM gas and SBF compute units priced
//!   in Maya VM fuel by fixed, measured ratios.
//!
//! # Status
//!
//! RESEARCH. Nothing in block execution calls this crate; ADR-023 records what
//! "EVM compatibility" would and would not mean before any of it does. The
//! engines are the real ones (`revm` 43, `solana-sbpf` 0.25), not models.

pub mod evm;
pub mod gas;
pub mod sbf;
pub mod state;

/// Why an engine refused or failed.
#[derive(Debug, thiserror::Error)]
pub enum MultiVmError {
    /// The engine reported an error before or during execution.
    #[error("{engine}: {message}")]
    Engine {
        /// Which engine.
        engine: &'static str,
        /// Its message.
        message: String,
    },
    /// Execution ran but did not succeed (revert, halt, program error).
    #[error("{engine} execution failed: {message}")]
    Failed {
        /// Which engine.
        engine: &'static str,
        /// What happened.
        message: String,
    },
}
