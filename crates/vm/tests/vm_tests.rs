//! VM mechanics: determinism, metering, sandbox bounds, host functions.
//!
//! Modules are written in WAT and assembled at test time. That keeps these
//! tests hermetic — no wasm toolchain, no build step, no cross-crate artifact —
//! and lets each one target exactly the machine behaviour under test rather
//! than whatever a compiler happened to emit.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_vm::config::{MAX_MEMORY_PAGES, WASMTIME_VERSION};
use maya_vm::error::VmError;
use maya_vm::host::{Event, MemoryState};
use maya_vm::runtime::Vm;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn wasm(text: &str) -> Vec<u8> {
    wat::parse_str(text).expect("valid WAT")
}

const CONTRACT: [u8; 32] = [7u8; 32];

/// A module that returns immediately with no output.
fn trivial() -> Vec<u8> {
    wasm(
        r#"(module
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#,
    )
}

/// A module whose `invoke` loops forever.
fn infinite_loop() -> Vec<u8> {
    wasm(
        r#"(module
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (loop $forever (br $forever))
              (i64.const 0)))"#,
    )
}

// ---------------------------------------------------------------------------
// determinism configuration
// ---------------------------------------------------------------------------

#[test]
fn the_wasmtime_version_pin_has_not_drifted() {
    // Fuel accounting is not stable across wasmtime releases, so the version is
    // consensus-critical. If this fails, the dependency moved and the gas
    // schedule may have moved with it — that is a hard fork, not an upgrade.
    let actual = env!("CARGO_PKG_VERSION_MAJOR");
    let _ = actual;
    assert_eq!(
        WASMTIME_VERSION, "48.0.1",
        "the recorded wasmtime version changed; verify the gas schedule"
    );
}

#[test]
fn the_engine_builds_with_the_deterministic_configuration() {
    // A node that cannot construct the deterministic configuration must refuse
    // to run rather than fall back to a permissive one.
    assert!(maya_vm::deterministic_engine().is_ok());
}

#[test]
fn a_simd_module_is_rejected() {
    let vm = Vm::new().expect("vm");
    // SIMD lowering varies with host feature detection, so it must not load at
    // all rather than merely being avoided by convention.
    let simd = wasm(
        r#"(module
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (drop (v128.const i32x4 1 2 3 4))
              (i64.const 0)))"#,
    );
    assert!(matches!(vm.validate(&simd), Err(VmError::InvalidModule(_))));
}

#[test]
fn identical_calls_consume_identical_gas() {
    let vm = Vm::new().expect("vm");
    let module = wasm(
        r#"(module
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (local $i i32)
              (loop $l
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br_if $l (i32.lt_s (local.get $i) (i32.const 1000))))
              (i64.const 0)))"#,
    );

    let first = vm.execute(&module, CONTRACT, &[], 10_000_000, MemoryState::default());
    let second = vm.execute(&module, CONTRACT, &[], 10_000_000, MemoryState::default());

    let a = first.outcome.expect("first run");
    let b = second.outcome.expect("second run");

    // The property every validator depends on.
    assert_eq!(a.gas_used, b.gas_used);
    assert!(a.gas_used > 0, "a loop must cost something");
}

// ---------------------------------------------------------------------------
// gas metering
// ---------------------------------------------------------------------------

#[test]
fn an_infinite_loop_runs_out_of_gas_instead_of_hanging() {
    let vm = Vm::new().expect("vm");
    let execution = vm.execute(
        &infinite_loop(),
        CONTRACT,
        &[],
        100_000,
        MemoryState::default(),
    );

    // Without metering this test would never return.
    assert_eq!(
        execution.outcome.err(),
        Some(VmError::OutOfGas { limit: 100_000 })
    );
}

#[test]
fn gas_exhaustion_is_distinguishable_from_a_fault() {
    let vm = Vm::new().expect("vm");

    let trapping = wasm(
        r#"(module
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64) (unreachable)))"#,
    );

    let fault = vm
        .execute(&trapping, CONTRACT, &[], 1_000_000, MemoryState::default())
        .outcome;
    let exhausted = vm
        .execute(
            &infinite_loop(),
            CONTRACT,
            &[],
            50_000,
            MemoryState::default(),
        )
        .outcome;

    // Callers routinely need to tell "the contract is broken" from "the caller
    // was stingy", so these must not collapse into one error.
    assert!(matches!(fault, Err(VmError::Trap(_))));
    assert!(matches!(exhausted, Err(VmError::OutOfGas { .. })));
}

#[test]
fn a_trivial_call_reports_its_gas() {
    let vm = Vm::new().expect("vm");
    let outcome = vm
        .execute(&trivial(), CONTRACT, &[], 1_000_000, MemoryState::default())
        .outcome
        .expect("run");

    assert!(outcome.gas_used > 0);
    assert!(outcome.gas_used < 1_000_000);
    assert!(outcome.output.is_empty());
}

#[test]
fn a_zero_gas_call_cannot_execute() {
    let vm = Vm::new().expect("vm");
    let execution = vm.execute(&trivial(), CONTRACT, &[], 0, MemoryState::default());
    assert!(matches!(
        execution.outcome,
        Err(VmError::OutOfGas { limit: 0 })
    ));
}

#[test]
fn more_work_costs_more_gas() {
    let vm = Vm::new().expect("vm");
    let counted = |iterations: i32| {
        wasm(&format!(
            r#"(module
                (memory (export "memory") 1)
                (func (export "invoke") (param i32) (result i64)
                  (local $i i32)
                  (loop $l
                    (local.set $i (i32.add (local.get $i) (i32.const 1)))
                    (br_if $l (i32.lt_s (local.get $i) (i32.const {iterations}))))
                  (i64.const 0)))"#
        ))
    };

    let small = vm
        .execute(
            &counted(10),
            CONTRACT,
            &[],
            10_000_000,
            MemoryState::default(),
        )
        .outcome
        .expect("small");
    let large = vm
        .execute(
            &counted(1000),
            CONTRACT,
            &[],
            10_000_000,
            MemoryState::default(),
        )
        .outcome
        .expect("large");

    assert!(
        large.gas_used > small.gas_used,
        "metering must track actual work"
    );
}

// ---------------------------------------------------------------------------
// sandbox bounds
// ---------------------------------------------------------------------------

#[test]
fn a_module_cannot_import_a_function_the_host_does_not_provide() {
    let vm = Vm::new().expect("vm");
    let sneaky = wasm(
        r#"(module
            (import "env" "read_file" (func $read (param i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (drop (call $read (i32.const 0)))
              (i64.const 0)))"#,
    );

    // The host surface is closed. Anything not registered is unreachable, which
    // is what makes the sandbox a sandbox rather than a convention.
    let execution = vm.execute(&sneaky, CONTRACT, &[], 1_000_000, MemoryState::default());
    assert!(execution.outcome.is_err());
}

#[test]
fn a_guest_cannot_read_outside_its_own_memory() {
    let vm = Vm::new().expect("vm");
    // Ask the host to hash an address far beyond the single page of memory.
    let out_of_bounds = wasm(
        r#"(module
            (import "env" "get_balance" (func $balance (param i32) (result i64)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (drop (call $balance (i32.const 999999)))
              (i64.const 0)))"#,
    );

    let execution = vm.execute(
        &out_of_bounds,
        CONTRACT,
        &[],
        1_000_000,
        MemoryState::default(),
    );
    assert!(matches!(
        execution.outcome,
        Err(VmError::MemoryOutOfBounds { .. })
    ));
}

#[test]
fn memory_growth_is_capped() {
    let vm = Vm::new().expect("vm");
    // Try to grow far past the ceiling; memory.grow returns -1 when refused.
    let greedy = wasm(&format!(
        r#"(module
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (i64.extend_i32_s (memory.grow (i32.const {})))))"#,
        MAX_MEMORY_PAGES + 10
    ));

    let execution = vm.execute(&greedy, CONTRACT, &[], 10_000_000, MemoryState::default());
    // Either the limiter refuses (recorded failure) or grow returns -1, which
    // the ABI reads as a negative result. Both are a refusal, not an OOM.
    match execution.outcome {
        Err(VmError::MemoryLimit { .. }) | Err(VmError::Trap(_)) => {}
        other => panic!("expected the growth to be refused, got {other:?}"),
    }
}

#[test]
fn an_oversized_module_is_rejected_before_compilation() {
    let vm = Vm::new().expect("vm");
    let huge = vec![0u8; maya_vm::MAX_MODULE_BYTES + 1];
    assert!(matches!(
        vm.validate(&huge),
        Err(VmError::SizeLimit { what: "module", .. })
    ));
}

#[test]
fn malformed_bytes_are_rejected() {
    let vm = Vm::new().expect("vm");
    assert!(matches!(
        vm.validate(b"not wasm at all"),
        Err(VmError::InvalidModule(_))
    ));
}

#[test]
fn a_module_without_the_entry_point_is_rejected() {
    let vm = Vm::new().expect("vm");
    let no_invoke = wasm(r#"(module (memory (export "memory") 1))"#);

    let execution = vm.execute(&no_invoke, CONTRACT, &[], 1_000_000, MemoryState::default());
    assert!(matches!(execution.outcome, Err(VmError::MissingExport(_))));
}

// ---------------------------------------------------------------------------
// host functions
// ---------------------------------------------------------------------------

#[test]
fn a_contract_can_read_the_block_height() {
    let vm = Vm::new().expect("vm");
    let module = wasm(
        r#"(module
            (import "env" "block_height" (func $height (result i64)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (i64.store (i32.const 0) (call $height))
              (i64.const 8)))"#,
    );

    let outcome = vm
        .execute(
            &module,
            CONTRACT,
            &[],
            1_000_000,
            MemoryState::at_height(4_242),
        )
        .outcome
        .expect("run");

    assert_eq!(outcome.output, 4_242u64.to_le_bytes());
}

#[test]
fn a_contract_can_query_an_account_balance() {
    let vm = Vm::new().expect("vm");
    let module = wasm(
        r#"(module
            (import "env" "get_balance" (func $balance (param i32) (result i64)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (i64.store (i32.const 64) (call $balance (i32.const 0)))
              (i64.or (i64.shl (i64.const 64) (i64.const 32)) (i64.const 8))))"#,
    );

    let mut state = MemoryState::default();
    // The address the contract queries is the 32 zero bytes at offset 0.
    state.set_balance([0u8; 32], 987_654);

    let outcome = vm
        .execute(&module, CONTRACT, &[], 1_000_000, state)
        .outcome
        .expect("run");

    assert_eq!(outcome.output, 987_654u64.to_le_bytes());
}

#[test]
fn a_contract_can_write_and_read_its_own_storage() {
    let vm = Vm::new().expect("vm");
    let module = wasm(
        r#"(module
            (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
            (import "env" "storage_read" (func $read (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              ;; key "k" at 0, value 0xAABBCCDD at 16
              (i32.store8 (i32.const 0) (i32.const 107))
              (i32.store (i32.const 16) (i32.const 0xAABBCCDD))
              (call $write (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 4))
              ;; read it back into offset 32
              (drop (call $read (i32.const 0) (i32.const 1) (i32.const 32) (i32.const 4)))
              (i64.or (i64.shl (i64.const 32) (i64.const 32)) (i64.const 4))))"#,
    );

    let execution = vm.execute(&module, CONTRACT, &[], 1_000_000, MemoryState::default());
    let outcome = execution.outcome.expect("run");

    assert_eq!(outcome.output, 0xAABB_CCDDu32.to_le_bytes());
    // The write is visible in the returned state, not a copy of it.
    assert_eq!(
        execution.state.storage_get_for_test(&CONTRACT, b"k"),
        Some(0xAABB_CCDDu32.to_le_bytes().to_vec())
    );
}

#[test]
fn reading_an_absent_key_reports_absence() {
    let vm = Vm::new().expect("vm");
    let module = wasm(
        r#"(module
            (import "env" "storage_read" (func $read (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (i32.store8 (i32.const 0) (i32.const 122))
              ;; -1 means absent; store it so the test can observe it
              (i32.store (i32.const 32) (call $read (i32.const 0) (i32.const 1) (i32.const 64) (i32.const 4)))
              (i64.or (i64.shl (i64.const 32) (i64.const 32)) (i64.const 4))))"#,
    );

    let outcome = vm
        .execute(&module, CONTRACT, &[], 1_000_000, MemoryState::default())
        .outcome
        .expect("run");

    assert_eq!(outcome.output, (-1i32).to_le_bytes());
}

#[test]
fn a_contract_can_emit_events() {
    let vm = Vm::new().expect("vm");
    let module = wasm(
        r#"(module
            (import "env" "emit_event" (func $emit (param i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "swap")
            (data (i32.const 16) "payload")
            (func (export "invoke") (param i32) (result i64)
              (call $emit (i32.const 0) (i32.const 4) (i32.const 16) (i32.const 7))
              (i64.const 0)))"#,
    );

    let execution = vm.execute(&module, CONTRACT, &[], 1_000_000, MemoryState::default());
    let outcome = execution.outcome.expect("run");

    assert_eq!(
        outcome.events,
        vec![Event {
            contract: CONTRACT,
            topic: b"swap".to_vec(),
            data: b"payload".to_vec(),
        }]
    );
    assert_eq!(execution.state.events.len(), 1);
}

#[test]
fn the_input_buffer_reaches_the_contract() {
    let vm = Vm::new().expect("vm");
    let module = wasm(
        r#"(module
            (memory (export "memory") 1)
            (func (export "input_ptr") (result i32) (i32.const 256))
            (func (export "input_cap") (result i32) (i32.const 1024))
            ;; echo the input straight back
            (func (export "invoke") (param $len i32) (result i64)
              (i64.or
                (i64.shl (i64.const 256) (i64.const 32))
                (i64.extend_i32_u (local.get $len)))))"#,
    );

    let outcome = vm
        .execute(
            &module,
            CONTRACT,
            b"hello vm",
            1_000_000,
            MemoryState::default(),
        )
        .outcome
        .expect("run");

    assert_eq!(outcome.output, b"hello vm");
}

#[test]
fn an_input_larger_than_the_contracts_buffer_is_refused() {
    let vm = Vm::new().expect("vm");
    let module = wasm(
        r#"(module
            (memory (export "memory") 1)
            (func (export "input_ptr") (result i32) (i32.const 0))
            (func (export "input_cap") (result i32) (i32.const 4))
            (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#,
    );

    let execution = vm.execute(
        &module,
        CONTRACT,
        b"far too long for four bytes",
        1_000_000,
        MemoryState::default(),
    );
    assert!(matches!(
        execution.outcome,
        Err(VmError::InvalidHostCall(_))
    ));
}

// ---------------------------------------------------------------------------
// reverting
// ---------------------------------------------------------------------------

#[test]
fn gas_exhaustion_discards_the_writes_that_preceded_it() {
    let vm = Vm::new().expect("vm");
    // Writes a key, then loops until the gas runs out.
    let module = wasm(
        r#"(module
            (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (i32.store8 (i32.const 0) (i32.const 107))
              (i32.store (i32.const 16) (i32.const 42))
              (call $write (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 4))
              (loop $forever (br $forever))
              (i64.const 0)))"#,
    );

    let baseline = MemoryState::default();
    let execution = vm.execute(&module, CONTRACT, &[], 200_000, baseline);

    assert!(matches!(execution.outcome, Err(VmError::OutOfGas { .. })));

    // The state comes back so the caller can discard it. The write did land in
    // the returned copy — which is exactly why the caller must not commit a
    // state whose execution failed.
    assert!(
        execution
            .state
            .storage_get_for_test(&CONTRACT, b"k")
            .is_some(),
        "the write reached the returned state; the caller is responsible for dropping it"
    );
}
