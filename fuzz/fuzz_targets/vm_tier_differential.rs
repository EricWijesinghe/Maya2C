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
//! `crates/vm/tests/tier_corpus/` was produced from a merged fuzz corpus
//! (selected by hash order under 4 KiB, not minimised), so CI replays real
//! fuzzer finds without depending on `wasm-smith`.
//!
//! The host state is drawn from the same bytes: up to two oracle feeds (fresh,
//! stale, or too wide for the 8-byte read) and the zkML verifier's answer, so
//! the tiers are compared on the success path of those host calls too, not
//! only on "absent".

#![no_main]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::OnceLock;

use libfuzzer_sys::arbitrary::{Result, Unstructured};
use libfuzzer_sys::fuzz_target;
use maya_vm::config::{MAX_MEMORY_PAGES, PAGE_SIZE};
use maya_vm::host::OracleValue;
use maya_vm::zkml::ZKML_MODEL_ID_LEN;
use maya_vm::{
    Address, Amount, ContractId, Event, HostState, MemoryState, Tier, Vm, VmError, ZkmlVerdict,
};

const CONTRACT: [u8; 32] = [9u8; 32];
/// Pulley is roughly 19x slower than Cranelift; a larger ceiling spends the
/// budget on a few long loops instead of many distinct modules.
const MAX_GAS: u64 = 2_000_000;
const MAX_INPUT: usize = 64;
const HEIGHT: u64 = 7;
/// Id 0 is what an unwritten guest pointer reads; id 1 needs the module to
/// write it first.
const FEED_IDS: [[u8; 32]; 2] = [[0u8; 32], [1u8; 32]];

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

/// `MemoryState` plus a zkML verifier that answers what the input chose.
#[derive(Clone, Debug)]
struct FuzzHost {
    inner: MemoryState,
    zkml: ZkmlVerdict,
}

impl HostState for FuzzHost {
    fn balance_of(&self, address: &Address) -> Amount {
        self.inner.balance_of(address)
    }
    fn caller(&self) -> Option<Address> {
        self.inner.caller()
    }
    fn block_height(&self) -> u64 {
        self.inner.block_height()
    }
    fn block_randomness(&self) -> Option<[u8; 32]> {
        self.inner.block_randomness()
    }
    fn oracle_feed(&self, feed_id: &[u8; 32]) -> Option<OracleValue> {
        self.inner.oracle_feed(feed_id)
    }
    fn verify_zkml(
        &self,
        _model_id: &[u8; ZKML_MODEL_ID_LEN],
        _vk: &[u8],
        _public: &[i64],
        _proof: &[u8],
    ) -> ZkmlVerdict {
        self.zkml.clone()
    }
    fn storage_get(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        self.inner.storage_get(contract, key)
    }
    fn storage_set(&mut self, contract: &ContractId, key: Vec<u8>, value: Vec<u8>) {
        self.inner.storage_set(contract, key, value);
    }
    fn emit(&mut self, event: Event) {
        self.inner.emit(event);
    }
}

fn host(u: &mut Unstructured<'_>) -> Result<FuzzHost> {
    let mut inner = MemoryState::at_height(HEIGHT);
    inner.caller = Some([3u8; 32]);
    inner.randomness = Some([5u8; 32]);
    inner.set_balance([3u8; 32], 1_000_000);
    for id in FEED_IDS {
        if u.arbitrary()? {
            // A u64 price reads through both calls; a full 256-bit one makes
            // the 8-byte call refuse it (-3).
            let value = if u.arbitrary()? {
                Amount::from_u64(u.arbitrary()?)
            } else {
                Amount::from_le_bytes(u.arbitrary()?)
            };
            let feed = OracleValue {
                value,
                // Up to two blocks ahead of the chain: the saturating age.
                updated_height: u.int_in_range(0..=HEIGHT + 2)?,
                round: u.arbitrary()?,
                observations: u.arbitrary()?,
            };
            inner.feeds.insert(id, feed);
        }
    }
    let zkml = match u.int_in_range(0..=3u8)? {
        0 => ZkmlVerdict::Unavailable,
        1 => ZkmlVerdict::Valid,
        2 => ZkmlVerdict::Invalid,
        _ => ZkmlVerdict::Malformed("fuzz".into()),
    };
    Ok(FuzzHost { inner, zkml })
}

fn observe(vm: &Vm, code: &[u8], input: &[u8], gas: u64, host: &FuzzHost) -> Observed {
    let exec = vm.execute(code, CONTRACT, input, gas, host.clone());
    Observed {
        outcome: exec.outcome.map_err(|e| error_class(&e)),
        state: (exec.state.inner.storage, exec.state.inner.events),
    }
}

/// The dump directory, created once. A bad path stops the run here with a
/// clear message, rather than panicking inside every exec, which libFuzzer
/// would record as a crash and look like a tier divergence.
fn dump_dir() -> Option<&'static std::path::Path> {
    static DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::path::PathBuf::from(std::env::var_os("MAYA_VM_DUMP_WASM")?);
        if let Err(error) = std::fs::create_dir_all(&dir) {
            eprintln!("MAYA_VM_DUMP_WASM={}: {error}", dir.display());
            std::process::exit(2);
        }
        Some(dir)
    })
    .as_deref()
}

fn dump(code: &[u8]) {
    if let Some(dir) = dump_dir() {
        let name = format!("{}.wasm", blake3::hash(code).to_hex());
        // A failed write loses one module from the dump, not the run.
        if let Err(error) = std::fs::write(dir.join(name), code) {
            eprintln!("dump module: {error}");
        }
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
    let Ok(host) = host(&mut u) else {
        return;
    };
    let Ok(config) = smith_config(&mut u) else {
        return;
    };
    let Ok(module) = wasm_smith::Module::new(config, &mut u) else {
        return;
    };
    let code = module.to_bytes();
    dump(&code);

    let (cranelift, pulley) = vms();
    let cold = observe(cranelift, &code, &input, gas, &host);
    let cached = observe(cranelift, &code, &input, gas, &host);
    assert_eq!(
        cold, cached,
        "cranelift cold vs cached, gas {gas}, input {input:?}, host {host:?}"
    );
    let interpreted = observe(pulley, &code, &input, gas, &host);
    assert_eq!(
        cold, interpreted,
        "cranelift vs pulley, gas {gas}, input {input:?}, host {host:?}"
    );
});
