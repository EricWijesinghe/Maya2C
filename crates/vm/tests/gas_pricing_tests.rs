//! Priced native work (ADR-142): a call pays for its module, every host call
//! pays a base, and storage and events pay by the byte, all before the work.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_vm::gas::{
    CALL_BASE_FUEL, CODE_BYTE_FUEL, EVENT_BYTE_FUEL, HOST_CALL_FUEL, STORAGE_READ_BYTE_FUEL,
    STORAGE_WRITE_BYTE_FUEL, WARM_CODE_BYTE_FUEL, call_fuel,
};
use maya_vm::{MemoryState, Vm, VmError};

const CONTRACT: [u8; 32] = [3u8; 32];

fn wasm(text: &str) -> Vec<u8> {
    wat::parse_str(text).expect("valid WAT")
}

fn gas_of(vm: &Vm, code: &[u8], state: MemoryState) -> u64 {
    vm.execute(code, CONTRACT, &[], 1_000_000_000, state)
        .outcome
        .expect("runs")
        .gas_used
}

const EMPTY: &str = r#"(module (memory (export "memory") 1)
    (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#;

#[test]
fn a_call_pays_its_base_and_every_module_byte() {
    let vm = Vm::new().unwrap();
    let code = wasm(EMPTY);
    let used = gas_of(&vm, &code, MemoryState::at_height(1));
    // The body's own fuel as wasmtime 48 meters it: `i64.const` and the
    // implicit return, two units.
    assert_eq!(used, call_fuel(code.len(), false) + 2);
    assert_eq!(
        call_fuel(code.len(), false),
        CALL_BASE_FUEL + CODE_BYTE_FUEL * code.len() as u64
    );
}

#[test]
fn a_call_that_cannot_pay_its_base_runs_out_of_gas_before_compiling() {
    let vm = Vm::new().unwrap();
    let code = wasm(EMPTY);
    let short = call_fuel(code.len(), false) - 1;
    let outcome = vm
        .execute(&code, CONTRACT, &[], short, MemoryState::at_height(1))
        .outcome;
    assert_eq!(outcome, Err(VmError::OutOfGas { limit: short }));
    // Refused before the compile, so nothing was cached.
    assert_eq!(vm.cache().stats(), (0, 0));
}

#[test]
fn a_warm_call_pays_instantiation_but_not_the_compile() {
    let vm = Vm::new().unwrap();
    let code = wasm(EMPTY);
    let compiled = vm.compile(&code).unwrap();
    let run = |warm| {
        vm.execute_compiled(
            &compiled,
            warm,
            CONTRACT,
            &[],
            1_000_000_000,
            MemoryState::at_height(1),
        )
        .outcome
        .unwrap()
        .gas_used
    };
    let (cold, warm) = (run(false), run(true));
    assert_eq!(
        cold - warm,
        (CODE_BYTE_FUEL - WARM_CODE_BYTE_FUEL) * code.len() as u64
    );
    // `execute` is a cold call: the same price as an explicit cold one.
    assert_eq!(gas_of(&vm, &code, MemoryState::at_height(1)), cold);
}

/// A module that writes one `len`-byte value under a 4-byte key.
fn writer(len: u32) -> Vec<u8> {
    wasm(&format!(
        r#"(module
            (import "env" "storage_write" (func $w (param i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (call $w (i32.const 0) (i32.const 4) (i32.const 64) (i32.const {len}))
              (i64.const 0)))"#
    ))
}

#[test]
fn a_storage_heavy_call_pays_in_proportion_to_its_bytes() {
    let vm = Vm::new().unwrap();
    let small = writer(100);
    let large = writer(4_000);
    // Same instructions and same module length: only the value differs.
    assert_eq!(small.len(), large.len());
    let delta = gas_of(&vm, &large, MemoryState::at_height(1))
        - gas_of(&vm, &small, MemoryState::at_height(1));
    assert_eq!(delta, 3_900 * STORAGE_WRITE_BYTE_FUEL);
}

/// A module that reads the 4-byte key `kkkk` into a 4 KiB buffer.
const READER: &str = r#"(module
    (import "env" "storage_read" (func $r (param i32 i32 i32 i32) (result i32)))
    (memory (export "memory") 1)
    (data (i32.const 0) "kkkk")
    (func (export "invoke") (param i32) (result i64)
      (drop (call $r (i32.const 0) (i32.const 4) (i32.const 64) (i32.const 4096)))
      (i64.const 0)))"#;

#[test]
fn a_storage_read_pays_for_the_value_it_returns() {
    let vm = Vm::new().unwrap();
    let code = wasm(READER);
    let with_value = |len: usize| {
        let mut state = MemoryState::at_height(1);
        state
            .storage
            .insert((CONTRACT, b"kkkk".to_vec()), vec![1; len]);
        gas_of(&vm, &code, state)
    };
    assert_eq!(
        with_value(4_000) - with_value(100),
        3_900 * STORAGE_READ_BYTE_FUEL
    );
    // A miss pays for the key and nothing else.
    let miss = gas_of(&vm, &code, MemoryState::at_height(1));
    assert_eq!(with_value(100) - miss, 100 * STORAGE_READ_BYTE_FUEL);
}

#[test]
fn every_host_call_pays_the_host_base() {
    let vm = Vm::new().unwrap();
    let calls = |n: usize| {
        wasm(&format!(
            r#"(module
                (import "env" "block_height" (func $h (result i64)))
                (memory (export "memory") 1)
                (func (export "invoke") (param i32) (result i64)
                  {}
                  (i64.const 0)))"#,
            "(drop (call $h))".repeat(n)
        ))
    };
    let (one, two) = (calls(1), calls(2));
    let delta =
        gas_of(&vm, &two, MemoryState::at_height(1)) - gas_of(&vm, &one, MemoryState::at_height(1));
    let extra_bytes = (two.len() - one.len()) as u64;
    // The host base, the `call` operator, and the longer module (`drop` is free).
    assert_eq!(delta, HOST_CALL_FUEL + 1 + extra_bytes * CODE_BYTE_FUEL);
}

#[test]
fn event_bytes_are_charged_before_they_are_copied() {
    // A 1 MiB topic: unbounded and unmetered before ADR-142.
    let code = wasm(
        r#"(module
            (import "env" "emit_event" (func $e (param i32 i32 i32 i32)))
            (memory (export "memory") 17)
            (func (export "invoke") (param i32) (result i64)
              (call $e (i32.const 0) (i32.const 1048576) (i32.const 0) (i32.const 8))
              (i64.const 0)))"#,
    );
    let vm = Vm::new().unwrap();
    let base = call_fuel(code.len(), false);
    let outcome = vm
        .execute(
            &code,
            CONTRACT,
            &[],
            base + 100_000,
            MemoryState::at_height(1),
        )
        .outcome;
    assert!(
        matches!(outcome, Err(VmError::OutOfGas { .. })),
        "{outcome:?}"
    );
    let used = gas_of(&vm, &code, MemoryState::at_height(1));
    assert!(used >= base + HOST_CALL_FUEL + (1_048_576 + 8) * EVENT_BYTE_FUEL);
}
