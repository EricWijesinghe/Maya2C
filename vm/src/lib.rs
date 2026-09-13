//! # Maya VM
//!
//! A sandboxed, deterministic WebAssembly execution engine for the Maya2C chain,
//! built on wasmtime.
//!
//! ## Determinism is the whole job
//!
//! Every validator must agree on a contract call's output, its gas, and whether
//! it succeeded. Disagreement on any of those is a state divergence and a chain
//! split. Three things enforce it:
//!
//! 1. [`config::deterministic_config`] disables every non-deterministic
//!    WebAssembly proposal and pins compilation to Cranelift.
//! 2. The wasmtime dependency is pinned to an **exact** version, because fuel
//!    accounting is not stable across releases. See
//!    [`config::WASMTIME_VERSION`].
//! 3. [`host::HostState`] implementors must be pure functions of committed
//!    state. One host function that reads a clock undoes the other two.
//!
//! ## Layering
//!
//! This crate does not depend on the node crate. The node depends on *it* to
//! execute contracts, so the reverse would be a cycle. Chain state reaches the
//! VM through the [`host::HostState`] trait, which also makes the VM testable
//! against an in-memory stub.
//!
//! ## Gas
//!
//! Gas maps one-to-one onto wasmtime fuel and is a **halting bound, not a
//! price**. Exhausting it traps the call and discards its writes; nothing is
//! billed. A fee market is a separate design.

#![warn(missing_docs)]

pub mod cache;
pub mod config;
pub mod error;
pub mod host;
pub mod runtime;
pub mod zkml;

pub use config::{MAX_MEMORY_PAGES, MAX_MODULE_BYTES, WASMTIME_VERSION, deterministic_engine};
pub use error::{Result, VmError};
pub use host::{Address, ContractId, Event, HostState, MemoryState};
pub use runtime::{Execution, Outcome, Vm};
pub use zkml::ZkmlVerdict;
