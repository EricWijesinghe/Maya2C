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
//! | `target` named | Cranelift's ISA flags follow the host CPU, and so do frame sizes and the stack-overflow depth |
//! | `cranelift_nan_canonicalization` | NaN bit patterns differ between CPUs |
//! | `wasm_simd` off | SIMD lowering varies by host feature detection |
//! | `wasm_threads` off | shared memory makes execution order observable |
//! | reference types + function references off | reachability is observable; `call_ref` could drop fuel accounting (RUSTSEC-2026-0315, ADR-034) |
//! | tail call, extended const, multi-memory, memory64 off | default-on attack surface no contract needs; memory64 would sidestep the page ceiling (ADR-034) |
//! | fixed memory ceiling | a host-dependent limit forks on memory-heavy calls |

use wasmtime::{Config, Engine, Strategy, WasmFeatures};

use crate::error::{Result, VmError};

/// The exact wasmtime version this chain's gas schedule is defined against.
///
/// Asserted by a test. If the dependency moves, that test fails and the change
/// is surfaced as the consensus event it is.
pub const WASMTIME_VERSION: &str = "48.0.7";

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
    config_for(Some(env!("MAYA_VM_TARGET")))
}

/// The deterministic configuration compiling for `target`, or, given `None`,
/// for the host with its CPU's ISA features inferred: what this crate did
/// before R-6, kept only so a test can tell the two apart.
fn config_for(target: Option<&str>) -> Result<Config> {
    let mut config = Config::new();

    // Metering. Without fuel a contract can simply not terminate.
    config.consume_fuel(true);

    // Cranelift, explicitly: the compilation strategy affects the fuel schedule,
    // so it must not be left to autodetection.
    config.strategy(Strategy::Cranelift);

    // The target, named: with none set, wasmtime enables every ISA feature
    // the host CPU reports (AVX, BMI, LZCNT and so on), so one binary on two
    // CPUs compiled a contract to different code with different frames, and
    // where deep recursion overflows the stack moved with the CPU (re-freeze
    // review R-6). Named, Cranelift uses the triple's baseline flags on every
    // machine. Fuel is counted on wasm operators before lowering, so gas is
    // unchanged. The binary itself (triple, OS, compiler, profile) still
    // moves the boundary: ADR-142 amendment 6, BACKLOG G4.
    if let Some(target) = target {
        // One line starting `config.`: the digest test counts those.
        let refused = |e: wasmtime::Error| VmError::EngineConfig(e.to_string());
        config.target(target).map_err(refused)?;
    }

    // Float results must not depend on the host CPU's NaN payload behaviour.
    config.cranelift_nan_canonicalization(true);

    // Non-deterministic proposals, off.
    //
    // Threads, GC and exceptions are compiled out: without the `threads` and
    // `gc` features wasmtime forces them off itself, and a feature that was
    // never compiled cannot be flipped at runtime. That does NOT hold for
    // everything whose setter is missing from this build. A missing setter
    // is not a missing feature (ADR-034), so the rest are turned off below.
    config.wasm_simd(false);
    config.wasm_relaxed_simd(false);

    // Reference types and typed function references, off. The table above
    // always said so, but wasmtime compiles their named setters only with the
    // `gc` feature, which this build leaves out, so they were silently on:
    // `call_ref` validated, and RUSTSEC-2026-0315 (`call_ref` dropping fuel
    // accounting, an exponential gas amplification) was reachable by any
    // contract. `wasm_features` sets the flags without `gc`.
    config.wasm_features(
        WasmFeatures::REFERENCE_TYPES | WasmFeatures::FUNCTION_REFERENCES,
        false,
    );

    // WebAssembly 3.0 proposals that wasmtime 48 turns on by default and no
    // contract needs: tail calls, extended constant expressions, multiple
    // memories, 64-bit memories. None has a known gas bug today. Each is
    // attack surface in a consensus VM, and each would have been one more
    // setting the determinism table did not list. Off until a contract needs
    // one and a fuel test covers it. Memory64 also keeps MAX_MEMORY_PAGES the
    // only memory ceiling.
    config.wasm_features(
        WasmFeatures::TAIL_CALL
            | WasmFeatures::EXTENDED_CONST
            | WasmFeatures::MULTI_MEMORY
            | WasmFeatures::MEMORY64,
        false,
    );

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
        1, // target named (hashed below), no host ISA inference
        1, // cranelift_nan_canonicalization
        0, // wasm_simd
        0, // wasm_relaxed_simd
        0, // reference types + function references
        0, // tail call + extended const + multi-memory + memory64
        1, // wasm_bulk_memory
        1, // wasm_multi_value
        0, // debug_info
    ]);
    hasher.update(&(MAX_STACK_BYTES as u64).to_le_bytes());
    hasher.update(&(MAX_MEMORY_PAGES as u64).to_le_bytes());
    hasher.update(&(MAX_MODULE_BYTES as u64).to_le_bytes());
    hasher.update(env!("MAYA_VM_TARGET").as_bytes());
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// Settings `config_digest` accounts for: the eleven flags in its byte
    /// array, plus the three limits, the target and the two version strings
    /// hashed after.
    const SETTINGS_COVERED: usize = 12;

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
    fn the_engine_compiles_for_its_named_target_not_for_the_host_cpu() {
        // 09 R-6: with no target named, wasmtime took this CPU's ISA features
        // and the compiled code followed the machine. Named, it is the
        // triple's baseline. Seen on any CPU with a feature past that
        // baseline, which every CI runner and validator host has: the two
        // compile one module to different code.
        // A precompiled module records the ISA flags it was built with.
        let module = wat::parse_str(
            r#"(module (func (export "f") (param i32) (result i32)
                (i32.popcnt (local.get 0))))"#,
        )
        .unwrap();
        let named = deterministic_engine().unwrap();
        let inferred = Engine::new(&config_for(None).unwrap()).unwrap();
        assert_ne!(
            named.precompile_module(&module).unwrap(),
            inferred.precompile_module(&module).unwrap(),
        );
        // And the named target is the host's, so the engine still runs here.
        assert!(crate::Vm::new().is_ok());
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
