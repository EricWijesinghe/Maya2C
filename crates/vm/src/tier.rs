//! Execution tiers (Master Prompt 5 §2): the same deterministic configuration
//! compiled to native code by Cranelift, or to Pulley bytecode run by
//! wasmtime's portable interpreter.
//!
//! # Why gas is identical across tiers
//!
//! Fuel is not measured by the machine that runs the code. Cranelift inserts
//! the fuel decrements while lowering *wasm operators*, before it chooses a
//! target, so a native build and a Pulley build of one module decrement the
//! same counter by the same amounts at the same wasm-level points.
//! `tests/tier_differential_tests.rs` checks that claim on every contract in
//! the test corpus rather than trusting this paragraph: gas, output, events
//! and error class must match exactly between tiers and between a cold and a
//! cached compile.
//!
//! # What each tier is for
//!
//! | Tier | Use |
//! |---|---|
//! | [`Tier::Cranelift`] | the node's tier: native code, cached by code hash |
//! | [`Tier::Pulley`] | hosts Cranelift cannot target, and a cross-check of the first |
//!
//! The "cached precompiled modules" tier of the brief is not a third engine:
//! it is [`crate::cache::ModuleCache`] in front of either, and a hit changes
//! compile time only.
//!
//! A tier is a node-local choice, never consensus, *because* gas is identical;
//! the tier is still mixed into the module-cache key so a cache can never hand
//! Pulley bytecode to a native engine.

use wasmtime::Engine;

use crate::config::{config_digest, deterministic_config};
use crate::error::{Result, VmError};

/// A compilation target for the deterministic configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tier {
    /// Native code for the host.
    Cranelift,
    /// Pulley bytecode, interpreted. Portable; roughly an order of magnitude
    /// slower (measured in `reports/05-vm.md`).
    Pulley,
}

impl Tier {
    /// Every tier.
    pub const ALL: [Self; 2] = [Self::Cranelift, Self::Pulley];

    /// Name used in logs and reports.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cranelift => "cranelift",
            Self::Pulley => "pulley",
        }
    }
}

/// An engine for `tier`, and the module-cache digest that goes with it.
///
/// # Errors
///
/// [`VmError::EngineConfig`] if wasmtime rejects the target.
pub fn tier_engine(tier: Tier) -> Result<(Engine, [u8; 32])> {
    let mut config = deterministic_config()?;
    if tier == Tier::Pulley {
        // 64-bit Pulley regardless of host, so one bytecode serves every
        // machine; the host's own pointer width does not enter it.
        config
            .target("pulley64")
            .map_err(|e| VmError::EngineConfig(e.to_string()))?;
    }
    let engine = Engine::new(&config).map_err(|e| VmError::EngineConfig(e.to_string()))?;
    let mut digest = blake3::Hasher::new();
    digest.update(&config_digest());
    digest.update(tier.name().as_bytes());
    Ok((engine, *digest.finalize().as_bytes()))
}
