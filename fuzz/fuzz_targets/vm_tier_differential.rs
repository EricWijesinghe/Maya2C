//! Differential fuzzing of the VM's two execution tiers (V3-P3).
//!
//! `wasm-smith` turns the input into a module shaped like a contract: it
//! exports `memory` and `invoke`, may import any host function the VM
//! registers, and uses only the proposals `deterministic_config` enables.
//! The module then runs on Cranelift (cold), Cranelift (a module-cache hit)
//! and Pulley, with a gas limit and input taken from the same bytes.
//!
//! The property is the one `crate::tier` states in prose: output, gas used,
//! events, the trap or error, and every byte of final state are identical
//! in every tier. A tier is only a node-local choice while that holds; a
//! divergence here is two validators disagreeing about one call.
//!
//! Setting `MAYA_VM_DUMP_WASM=<dir>` writes every generated module into
//! `<dir>` as `<blake3>.wasm`. That is how the committed corpus in
//! `crates/vm/tests/tier_corpus/` is produced from a minimised fuzz corpus,
//! so CI replays real fuzzer finds without depending on `wasm-smith`.

#![no_main]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::OnceLock;

use libfuzzer_sys::arbitrary::{Result, Unstructured};
use libfuzzer_sys::fuzz_target;
use maya_vm::config::{MAX_MEMORY_PAGES, PAGE_SIZE};
use maya_vm::{MemoryState, Tier, Vm, VmError};

const CONTRACT: [u8; 32] = [9u8; 32];
/// Pulley is roughly 19x slower than Cranelift; a larger ceiling spends the
/// budget on a few long loops instead of many distinct modules.
const MAX_GAS: u64 = 2_000_000;
const MAX_INPUT: usize = 64;

/// Every host function, with the signature `register_host_functions` gives it.
const HOST_IMPORTS: &str = r#"(module
  (import "env" "block_height" (func (result i64)))
  (import "env" "caller" (func (param i32) (result i32)))
  (import "env" "get_balance" (func (param i32) (result i64)))
  (import "env" "get_balance_u256" (func (param i32 i32) (result i32)))
  (import "env" "storage_read" (func (param i32 i32 i32 i32) (result i32)))
  (import "env" "storage_write" (func (param i32 i32 i32 i32)))
  (import "env" "block_randomness" (func (param i32) (result i32)))
  (import "env" "oracle_read" (func (param i32 i64 i32) (result i32)))
  (import "env" "oracle_read_u256" (func (param i32 i64 i32) (result i32)))
  (import "env" "oracle_feed_age" (func (param i32) (result i64)))
  (import "env" "emit_event" (func (param i32 i32 i32 i32)))
  (import "env" "host_verify_zkml_proof" (func (param i32 i32 i32 i32 i32 i32 i32) (result i32))))"#;

/// What `Vm::invoke` looks up. `input_ptr`/`input_cap` are only read when the
/// input is non-empty.
const CONTRACT_EXPORTS: &str = r#"(module
  (memory (export "memory") 1)
  (func (export "invoke") (param i32) (result i64) unreachable)
  (func (export "input_ptr") (result i32) unreachable)
  (func (export "input_cap") (result i32) unreachable))"#;

struct Shape {
    imports: Vec<u8>,
    exports: Vec<u8>,
}

fn shape() -> &'static Shape {
    static SHAPE: OnceLock<Shape> = OnceLock::new();
    SHAPE.get_or_init(|| Shape {
        imports: wat::parse_str(HOST_IMPORTS).expect("host import WAT"),
        exports: wat::parse_str(CONTRACT_EXPORTS).expect("export WAT"),
    })
}

fn vms() -> &'static (Vm, Vm) {
    static VMS: OnceLock<(Vm, Vm)> = OnceLock::new();
    VMS.get_or_init(|| {
        (
            Vm::with_tier(Tier::Cranelift).expect("cranelift engine"),
            Vm::with_tier(Tier::Pulley).expect("pulley engine"),
        )
    })
}

/// The proposals `deterministic_config` turns off are off here too, so the
/// budget goes on modules both tiers will compile.
fn smith_config(u: &mut Unstructured<'_>) -> Result<wasm_smith::Config> {
    Ok(wasm_smith::Config {
        available_imports: Some(shape().imports.clone()),
        exports: Some(shape().exports.clone()),
        max_memory32_bytes: ((MAX_MEMORY_PAGES + 1) * PAGE_SIZE) as u64,
        max_memories: 1,
        max_tables: 1,
        max_funcs: 16,
        max_instructions: u.int_in_range(10..=400)?,
        allow_floats: true,
        bulk_memory_enabled: true,
        multi_value_enabled: true,
        sign_extension_ops_enabled: true,
        saturating_float_to_int_enabled: true,
        reference_types_enabled: false,
        simd_enabled: false,
        relaxed_simd_enabled: false,
        exceptions_enabled: false,
        gc_enabled: false,
        tail_call_enabled: false,
        threads_enabled: false,
        memory64_enabled: false,
        wide_arithmetic_enabled: false,
        extended_const_enabled: false,
        custom_page_sizes_enabled: false,
        shared_everything_threads_enabled: false,
        compact_imports_enabled: false,
        max_nesting_depth: 8,
        ..wasm_smith::Config::default()
    })
}

/// Everything a validator could observe about one call.
#[derive(Debug, PartialEq, Eq)]
struct Observed {
    outcome: std::result::Result<maya_vm::Outcome, String>,
    state: (
        std::collections::BTreeMap<([u8; 32], Vec<u8>), Vec<u8>>,
        Vec<maya_vm::Event>,
    ),
}

/// The error with its tier-independent part only. A wasm trap's `Debug`
/// carries a backtrace; the trap code is the line that names it.
fn error_class(error: &VmError) -> String {
    match error {
        // From the marker on: the "N: " cause index before it is how deep
        // the tier happened to wrap the error, not what trapped.
        VmError::Trap(text) => text
            .find("wasm trap:")
            .map_or(text.as_str(), |at| text[at..].lines().next().unwrap_or(""))
            .to_string(),
        other => format!("{other:?}"),
    }
}

fn state() -> MemoryState {
    let mut state = MemoryState::at_height(7);
    state.caller = Some([3u8; 32]);
    state.randomness = Some([5u8; 32]);
    state.set_balance([3u8; 32], 1_000_000);
    state
}

fn observe(vm: &Vm, code: &[u8], input: &[u8], gas: u64) -> Observed {
    let exec = vm.execute(code, CONTRACT, input, gas, state());
    Observed {
        outcome: exec.outcome.map_err(|e| error_class(&e)),
        state: (exec.state.storage, exec.state.events),
    }
}

fn dump(code: &[u8]) {
    static DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    if let Some(dir) = DIR.get_or_init(|| std::env::var_os("MAYA_VM_DUMP_WASM").map(Into::into)) {
        let name = format!("{}.wasm", blake3::hash(code).to_hex());
        std::fs::write(dir.join(name), code).expect("dump module");
    }
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let Ok(gas) = u.int_in_range(0..=MAX_GAS) else {
        return;
    };
    let Ok(input_len) = u.int_in_range(0..=MAX_INPUT) else {
        return;
    };
    let Ok(input) = u.bytes(input_len) else {
        return;
    };
    let input = input.to_vec();
    let Ok(config) = smith_config(&mut u) else {
        return;
    };
    let Ok(module) = wasm_smith::Module::new(config, &mut u) else {
        return;
    };
    let code = module.to_bytes();
    dump(&code);

    let (cranelift, pulley) = vms();
    let cold = observe(cranelift, &code, &input, gas);
    let cached = observe(cranelift, &code, &input, gas);
    assert_eq!(cold, cached, "cranelift cold vs cached, gas {gas}");
    let interpreted = observe(pulley, &code, &input, gas);
    assert_eq!(cold, interpreted, "cranelift vs pulley, gas {gas}");
});
