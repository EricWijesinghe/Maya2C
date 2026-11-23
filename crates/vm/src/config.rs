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
//! | `memory_init_cow` off | data-segment gas depends on the host OS and page size (2 on Linux, 4,099 on Windows for one 16-byte segment) |
//! | `target` named | Cranelift's ISA flags follow the host CPU, and so do frame sizes and the stack-overflow depth |
//! | x86-64-v2 flags on x86-64 | the bare baseline lowers float rounding to libcalls and `popcnt` to a sequence: several times the wall time per unit of gas |
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
pub const WASMTIME_VERSION: &str = "48.0.10";

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

/// The Cranelift ISA flags every engine on this build's architecture turns
/// on: on x86-64 the x86-64-v2 level (SSE3 to SSE4.2, POPCNT, CMPXCHG16B),
/// elsewhere none.
///
/// The bare x86-64 baseline has no SSE4.1, so Cranelift lowers wasm's
/// `f32`/`f64` `floor`, `ceil`, `nearest` and `trunc` to calls into the host,
/// and `popcnt` to a bit-twiddling sequence. Fuel is one per operator either
/// way, so a block of those cost several times the wall time per unit of gas
/// of the work the block-time derivation measured (ADR-142 amendment 7, where
/// the figures are). Named, every validator compiles the same code still:
/// the level is fixed, not inferred. A CPU without it is refused when the
/// engine is built ([`deterministic_engine`]): every x86-64 CPU sold since
/// 2009 has it. aarch64's baseline already rounds and counts bits natively.
pub const ISA_FLAGS: &[&str] = if cfg!(target_arch = "x86_64") {
    &[
        "has_sse3",
        "has_ssse3",
        "has_sse41",
        "has_sse42",
        "has_popcnt",
        "has_cmpxchg16b",
    ]
} else {
    &[]
};

/// Builds the deterministic engine configuration.
///
/// # Errors
///
/// Returns [`VmError::EngineConfig`] if the engine rejects the configuration.
/// This is fatal by design: a node that cannot construct the deterministic
/// configuration must refuse to run rather than quietly use a permissive one.
pub fn deterministic_config() -> Result<Config> {
    config_for(Some(env!("MAYA_VM_TARGET")), ISA_FLAGS)
}

/// The deterministic configuration compiling for `target` with `isa_flags`
/// on, or, given `None`, for the host with its CPU's ISA features inferred:
/// what this crate did before R-6, kept only so a test can tell them apart.
/// The Pulley tier names its own target and no native flags.
pub(crate) fn config_for(target: Option<&str>, isa_flags: &[&str]) -> Result<Config> {
    let mut config = Config::new();

    // Metering. Without fuel a contract can simply not terminate.
    config.consume_fuel(true);

    // Data segments are copied by a fuel-metered startup function. With the
    // copy-on-write memory image on, whether that copy runs, and how many
    // bytes it copies (the image is padded to the compiler's page size),
    // depends on the host: Linux mapped the image and charged nothing,
    // Windows copied a 4 KiB page on Cranelift and 64 KiB on Pulley. Off, every
    // host copies exactly the segments' bytes, so their gas is the module's.
    // Pinned by `a_data_segment_costs_the_same_gas_on_every_host_and_tier`.
    config.memory_init_cow(false);

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
        for flag in isa_flags {
            // SAFETY: each flag is an ISA feature Cranelift knows (an unknown
            // name fails `Engine::new`), and wasmtime checks the host CPU
            // has every one enabled before it compiles or loads code
            // (`check_compatible_with_native_host`), so no instruction the
            // CPU lacks is ever executed. `deterministic_engine` makes that
            // check run at start-up; pinned by
            // `an_engine_refuses_a_cpu_without_its_isa_level`.
            unsafe {
                config.cranelift_flag_enable(flag);
            }
        }
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
        0, // memory_init_cow
        1, // Strategy::Cranelift
        1, // target named (hashed below), no host ISA inference
        1, // ISA_FLAGS (hashed below)
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
    for flag in ISA_FLAGS {
        hasher.update(flag.as_bytes());
        hasher.update(b";");
    }
    // The compiler itself. wasmtime does not guarantee stable fuel accounting
    // across versions — see the pin in Cargo.toml — so a version change must
    // invalidate every cached module too.
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update(WASMTIME_VERSION.as_bytes());
    // The call-depth rewrite every module compiles through (re-freeze R-6):
    // a module cached under another rewrite or budget is other code.
    hasher.update(crate::depth::REWRITE_VERSION);
    hasher.update(&crate::depth::DEPTH_LIMIT_SLOTS.to_le_bytes());
    hasher.update(&crate::depth::FRAME_SLOTS.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// Builds an engine with the deterministic configuration.
///
/// # Errors
///
/// Returns [`VmError::EngineConfig`] if the engine cannot be created.
pub fn deterministic_engine() -> Result<Engine> {
    checked_engine(&deterministic_config()?)
}

/// An engine for `config` that has compiled once, so a CPU without one of its
/// ISA flags is refused here, at start-up, rather than on the first contract
/// call (where it would read as a failure of that call).
pub(crate) fn checked_engine(config: &Config) -> Result<Engine> {
    let refused = |e: wasmtime::Error| VmError::EngineConfig(format!("{e:#}"));
    let engine = Engine::new(config).map_err(refused)?;
    // The smallest module: wasmtime checks the host against the flags on
    // every compile, so this one runs the check.
    wasmtime::Module::new(&engine, b"\0asm\x01\0\0\0").map_err(refused)?;
    Ok(engine)
}

#[cfg(test)]
mod digest_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]
    use super::*;

    /// Settings `config_digest` accounts for: the `config.` calls (one per flag in its byte
    /// array, plus the three limits, the target and the two version strings
    /// hashed after), counted as source lines.
    const SETTINGS_COVERED: usize = 14;

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
        let inferred = Engine::new(&config_for(None, &[]).unwrap()).unwrap();
        assert_ne!(
            named.precompile_module(&module).unwrap(),
            inferred.precompile_module(&module).unwrap(),
        );
        // And the named target is the host's, so the engine still runs here.
        assert!(crate::Vm::new().is_ok());
    }

    /// `(f64.floor x)` and `(i64.popcnt y)` in a loop: the two operators the
    /// bare x86-64 baseline does not lower to one instruction.
    const ROUNDING_AND_POPCNT: &str = r#"(module
        (func (export "f") (param f64 i64) (result f64 i64)
          (f64.floor (local.get 0)) (i64.popcnt (local.get 1))))"#;

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn the_engine_compiles_at_the_x86_64_v2_level_not_the_bare_baseline() {
        // Review of ADR-142 amendment 6: naming the target alone left
        // Cranelift on the triple's baseline, where float rounding is a
        // libcall and popcnt a sequence. Red with `ISA_FLAGS` empty: the two
        // engines precompiled this module to identical bytes.
        let module = wat::parse_str(ROUNDING_AND_POPCNT).unwrap();
        let levelled = deterministic_engine().unwrap();
        let baseline =
            Engine::new(&config_for(Some(env!("MAYA_VM_TARGET")), &[]).unwrap()).unwrap();
        assert_ne!(
            levelled.precompile_module(&module).unwrap(),
            baseline.precompile_module(&module).unwrap(),
        );
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn an_engine_refuses_a_cpu_without_its_isa_level() {
        // `checked_engine` must refuse at start-up a CPU missing a flag the
        // config turns on. No CPU lacks the x86-64-v2 level here, so a flag
        // past it stands in: whichever of these this CPU lacks.
        for (flag, feature) in [
            (
                "has_avx512vbmi",
                std::arch::is_x86_feature_detected!("avx512vbmi"),
            ),
            (
                "has_avx512f",
                std::arch::is_x86_feature_detected!("avx512f"),
            ),
            ("has_avx2", std::arch::is_x86_feature_detected!("avx2")),
        ] {
            let mut flags = ISA_FLAGS.to_vec();
            flags.push(flag);
            let built = checked_engine(&config_for(Some(env!("MAYA_VM_TARGET")), &flags).unwrap());
            assert_eq!(built.is_ok(), feature, "{flag}: {:?}", built.err());
        }
    }

    /// Wall time per unit of fuel of `body` run in a loop under `config`.
    fn ns_per_fuel(config: &Config, locals: &str, body: &str, result: &str) -> f64 {
        const ITERATIONS: u32 = 20_000_000;
        let module = wat::parse_str(format!(
            r#"(module (func (export "f") (result i64) (local $i i32) {locals}
                (block $done (loop $again
                  (br_if $done (i32.ge_u (local.get $i) (i32.const {ITERATIONS})))
                  {body}
                  (local.set $i (i32.add (local.get $i) (i32.const 1)))
                  (br $again)))
                {result}))"#
        ))
        .unwrap();
        let engine = Engine::new(config).unwrap();
        let module = wasmtime::Module::new(&engine, module).unwrap();
        let mut best = f64::MAX;
        for _ in 0..3 {
            let mut store = wasmtime::Store::new(&engine, ());
            store.set_fuel(u64::MAX / 2).unwrap();
            let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
            let f = instance.get_typed_func::<(), i64>(&mut store, "f").unwrap();
            let started = std::time::Instant::now();
            std::hint::black_box(f.call(&mut store, ()).unwrap());
            let ns = started.elapsed().as_nanos() as f64;
            let fuel = (u64::MAX / 2 - store.get_fuel().unwrap()) as f64;
            best = best.min(ns / fuel);
        }
        best
    }

    #[test]
    #[ignore = "timing: run with --ignored --nocapture and record the figures in ADR-142"]
    fn measure_float_rounding_and_popcnt_against_the_calibrated_compute() {
        // Each body feeds its result back, so Cranelift cannot drop the op.
        let f64_op = |op: &str| {
            (
                "(local $x f64)".to_string(),
                format!("(local.set $x (f64.{op} (f64.add (local.get $x) (f64.const 1.25))))"),
                "(i64.reinterpret_f64 (local.get $x))".to_string(),
            )
        };
        let f32_op = |op: &str| {
            (
                "(local $x f32)".to_string(),
                format!("(local.set $x (f32.{op} (f32.add (local.get $x) (f32.const 1.25))))"),
                "(i64.extend_i32_u (i32.reinterpret_f32 (local.get $x)))".to_string(),
            )
        };
        let int_op = |ty: &str, op: &str| {
            let counted = if ty == "i32" {
                format!(
                    "(i64.extend_i32_u (i32.{op} (i32.xor (i32.wrap_i64 (local.get $y)) (local.get $i))))"
                )
            } else {
                format!("(i64.{op} (i64.xor (local.get $y) (i64.extend_i32_u (local.get $i))))")
            };
            (
                "(local $y i64)".to_string(),
                format!("(local.set $y (i64.add (local.get $y) {counted}))"),
                "(local.get $y)".to_string(),
            )
        };
        let shapes = [
            ("i64.div_u (the calibrated compute)".to_string(), (
                String::new(),
                "(drop (i64.div_u (i64.const -1) (i64.extend_i32_u (i32.add (local.get $i) (i32.const 3)))))".to_string(),
                "(i64.const 0)".to_string(),
            )),
            ("f64.floor".into(), f64_op("floor")),
            ("f64.ceil".into(), f64_op("ceil")),
            ("f64.nearest".into(), f64_op("nearest")),
            ("f64.trunc".into(), f64_op("trunc")),
            ("f32.floor".into(), f32_op("floor")),
            ("f32.nearest".into(), f32_op("nearest")),
            ("i64.popcnt".into(), int_op("i64", "popcnt")),
            ("i32.popcnt".into(), int_op("i32", "popcnt")),
            ("i64.clz".into(), int_op("i64", "clz")),
            ("i64.ctz".into(), int_op("i64", "ctz")),
        ];
        let target = env!("MAYA_VM_TARGET");
        let configs = [
            ("host-inferred", config_for(None, &[]).unwrap()),
            ("baseline", config_for(Some(target), &[]).unwrap()),
            ("ISA_FLAGS", deterministic_config().unwrap()),
        ];
        for (label, (locals, body, result)) in &shapes {
            let figures: Vec<String> = configs
                .iter()
                .map(|(name, config)| {
                    format!("{name} {:.3}", ns_per_fuel(config, locals, body, result))
                })
                .collect();
            eprintln!("ISA-TIMING {label:<36} ns/fuel: {}", figures.join("  "));
        }
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
