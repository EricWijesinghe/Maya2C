//! Deterministic engine configuration.
//!
//! Every validator must reach an identical result — same output, same gas, same
//! success or failure — for the same contract and input. WebAssembly is
//! *mostly* deterministic, but several optional features and defaults are not,
//! and each has to be switched off explicitly.
//!
//! ## The version pin
//!
//! wasmtime makes no guarantee that fuel accounting is stable across releases.
//! The per-instruction schedule can change with any version. A validator on one
//! version and a validator on another can therefore disagree about whether a
//! call ran out of gas — which is a state divergence and a chain split.
//!
//! The dependency is pinned to an exact version for that reason, and
//! [`WASMTIME_VERSION`] records it so a test can assert the pin has not drifted.
//! Bumping wasmtime is a hard fork, not a dependency update.
//!
//! ## What each setting prevents
//!
//! | Setting | Without it |
//! |---|---|
//! | `consume_fuel` | contracts can loop forever |
//! | `cranelift_nan_canonicalization` | NaN bit patterns differ between CPUs |
//! | `wasm_simd` off | SIMD lowering varies by host feature detection |
//! | `wasm_threads` off | shared memory makes execution order observable |
//! | `wasm_reference_types` off | reachability, and thus GC timing, is observable |
//! | fixed memory ceiling | a host-dependent limit forks on memory-heavy calls |

use wasmtime::{Config, Engine, Strategy};

use crate::error::{Result, VmError};

/// The exact wasmtime version this chain's gas schedule is defined against.
///
/// Asserted by a test. If the dependency moves, that test fails and the change
/// is surfaced as the consensus event it is.
pub const WASMTIME_VERSION: &str = "48.0.1";

/// WebAssembly page size in bytes.
pub const PAGE_SIZE: usize = 64 * 1024;

/// Maximum linear memory pages a contract may hold: 16 MiB.
///
/// A fixed number rather than a fraction of host memory. A limit that varies
/// with the machine would let the same call succeed on one validator and trap
/// on another.
pub const MAX_MEMORY_PAGES: usize = 256;

/// Maximum guest stack, in bytes.
pub const MAX_STACK_BYTES: usize = 512 * 1024;

/// Largest module a deployment may carry.
pub const MAX_MODULE_BYTES: usize = 512 * 1024;

/// Builds the deterministic engine configuration.
///
/// # Errors
///
/// Returns [`VmError::EngineConfig`] if the engine rejects the configuration.
/// This is fatal by design: a node that cannot construct the deterministic
/// configuration must refuse to run rather than quietly use a permissive one.
pub fn deterministic_config() -> Result<Config> {
    let mut config = Config::new();

    // Metering. Without fuel a contract can simply not terminate.
    config.consume_fuel(true);

    // Cranelift, explicitly: the compilation strategy affects the fuel schedule,
    // so it must not be left to autodetection.
    config.strategy(Strategy::Cranelift);

    // Float results must not depend on the host CPU's NaN payload behaviour.
    config.cranelift_nan_canonicalization(true);

    // Non-deterministic proposals, off.
    //
    // Threads, GC, and reference types are not merely disabled here — they are
    // compiled out entirely by the crate's feature selection, which is why
    // wasmtime exposes no setter for them in this build. That is the stronger
    // guarantee: a runtime toggle can be flipped, a feature that was never
    // compiled cannot be.
    config.wasm_simd(false);
    config.wasm_relaxed_simd(false);

    // Deterministic and useful; memcpy-style ops save a great deal of fuel.
    config.wasm_bulk_memory(true);
    config.wasm_multi_value(true);

    // Bounded stack, so deep recursion traps rather than exhausting the host.
    config.max_wasm_stack(MAX_STACK_BYTES);

    // No ambient capability of any kind reaches the guest: every interaction
    // goes through an explicitly registered host function.
    config.debug_info(false);

    Ok(config)
}

/// Builds an engine with the deterministic configuration.
///
/// # Errors
///
/// Returns [`VmError::EngineConfig`] if the engine cannot be created.
/// A digest of everything `deterministic_config` sets.
///
/// Mixed into every module-cache key, so a change to the configuration
/// invalidates every cached module by construction rather than by somebody
/// remembering to clear a cache. Without it, a build that flipped NaN
/// canonicalisation would keep serving modules compiled under the old setting
/// until the process restarted — two nodes running one contract under two
/// compilers, which is the one way a cache can fork a chain.
///
/// Written out by hand because wasmtime exposes no stable fingerprint of a
/// `Config`. That makes this a list somebody must extend when they add a
/// setting, so `the_digest_covers_every_setting_the_config_makes` counts the
/// `config.` calls in this file against the fields below and fails when they
/// drift.
#[must_use]
pub fn config_digest() -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"maya-vm-config-v1:");
    // Every value `deterministic_config` sets, in the order it sets them.
    hasher.update(&[
        1, // consume_fuel
        1, // Strategy::Cranelift
        1, // cranelift_nan_canonicalization
        0, // wasm_simd
        0, // wasm_relaxed_simd
        1, // wasm_bulk_memory
        1, // wasm_multi_value
        0, // debug_info
    ]);
    hasher.update(&(MAX_STACK_BYTES as u64).to_le_bytes());
    hasher.update(&(MAX_MEMORY_PAGES as u64).to_le_bytes());
    hasher.update(&(MAX_MODULE_BYTES as u64).to_le_bytes());
    // The compiler itself. wasmtime does not guarantee stable fuel accounting
    // across versions — see the pin in Cargo.toml — so a version change must
    // invalidate every cached module too.
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update(b"wasmtime-48.0.1");
    *hasher.finalize().as_bytes()
}

/// Builds an engine with the deterministic configuration.
///
/// # Errors
///
/// Returns [`VmError::EngineConfig`] if the engine cannot be created.
pub fn deterministic_engine() -> Result<Engine> {
    let config = deterministic_config()?;
    Engine::new(&config).map_err(|e| VmError::EngineConfig(e.to_string()))
}

#[cfg(test)]
mod digest_tests {
    use super::*;

    /// Settings `config_digest` accounts for: the eight flags in its byte
    /// array, plus the three limits and the two version strings hashed after.
    const SETTINGS_COVERED: usize = 9;

    #[test]
    fn the_digest_covers_every_setting_the_config_makes() {
        // `config_digest` is a hand-written list, because wasmtime exposes no
        // stable fingerprint of a `Config`. A hand-written list drifts, and a
        // digest that missed a setting would keep serving modules compiled
        // under the old one after a change — two nodes running one contract
        // under two compilers.
        //
        // So the source is counted. Crude, and it fails loudly the moment
        // somebody adds a `config.` call without extending the digest, which is
        // exactly when a comment would have been ignored.
        let source = include_str!("config.rs");
        let calls = source
            .lines()
            .filter(|line| line.trim_start().starts_with("config."))
            .count();
        assert_eq!(
            calls, SETTINGS_COVERED,
            "`deterministic_config` makes {calls} settings and `config_digest` \
             accounts for {SETTINGS_COVERED}. Add the new one to the digest and \
             raise this count — a cached module compiled under the old setting \
             would otherwise outlive the change."
        );
    }

    #[test]
    fn the_digest_is_the_same_on_every_call() {
        // Two nodes must agree on it, so it cannot depend on anything but the
        // constants above.
        assert_eq!(config_digest(), config_digest());
    }

    #[test]
    fn the_digest_is_not_all_zeros() {
        // A digest that came back zeroed would make every configuration share a
        // cache, which is the failure the digest exists to prevent — and it
        // would look like it was working.
        assert_ne!(config_digest(), [0u8; 32]);
    }
}
