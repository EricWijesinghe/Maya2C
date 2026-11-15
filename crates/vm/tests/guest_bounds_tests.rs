//! Guest-range and size checks on the host surface.
//!
//! A host call refuses an out-of-range or oversized request before it copies
//! any guest bytes, and reports the same first error it always did.

use maya_vm::error::VmError;
use maya_vm::host::MemoryState;
use maya_vm::runtime::Vm;

const CONTRACT: [u8; 32] = [7; 32];

fn wasm(text: &str) -> Vec<u8> {
    wat::parse_str(text).expect("valid WAT")
}

#[test]
fn an_event_topic_over_the_limit_is_refused() {
    let vm = Vm::new().expect("vm");
    let module = wasm(
        r#"(module
            (import "env" "emit_event" (func $emit (param i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (call $emit (i32.const 0) (i32.const 129) (i32.const 0) (i32.const 0))
              (i64.const 0)))"#,
    );

    let outcome = vm
        .execute(&module, CONTRACT, &[], 1_000_000, MemoryState::default())
        .outcome;

    assert!(matches!(
        outcome,
        Err(VmError::SizeLimit {
            what: "event topic",
            actual: 129,
            limit: 128,
        })
    ));
}

#[test]
fn an_event_topic_at_the_limit_is_accepted() {
    let vm = Vm::new().expect("vm");
    let module = wasm(
        r#"(module
            (import "env" "emit_event" (func $emit (param i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (call $emit (i32.const 0) (i32.const 128) (i32.const 0) (i32.const 0))
              (i64.const 0)))"#,
    );

    let execution = vm.execute(&module, CONTRACT, &[], 1_000_000, MemoryState::default());

    assert!(execution.outcome.is_ok());
    assert_eq!(execution.state.events.len(), 1);
}

#[test]
fn an_oversized_storage_key_is_refused_by_size() {
    let vm = Vm::new().expect("vm");
    let module = wasm(
        r#"(module
            (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (call $write (i32.const 0) (i32.const 200) (i32.const 256) (i32.const 4))
              (i64.const 0)))"#,
    );

    let outcome = vm
        .execute(&module, CONTRACT, &[], 1_000_000, MemoryState::default())
        .outcome;

    assert!(matches!(
        outcome,
        Err(VmError::SizeLimit {
            what: "storage key",
            actual: 200,
            limit: 128,
        })
    ));
}

#[test]
fn an_oversized_storage_read_key_is_refused_by_size() {
    let vm = Vm::new().expect("vm");
    let module = wasm(
        r#"(module
            (import "env" "storage_read" (func $read (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (drop (call $read (i32.const 0) (i32.const 200) (i32.const 256) (i32.const 4)))
              (i64.const 0)))"#,
    );

    let outcome = vm
        .execute(&module, CONTRACT, &[], 1_000_000, MemoryState::default())
        .outcome;

    assert!(matches!(
        outcome,
        Err(VmError::SizeLimit {
            what: "storage key",
            actual: 200,
            limit: 128,
        })
    ));
}

#[test]
fn an_oversized_storage_value_is_refused_by_size() {
    let vm = Vm::new().expect("vm");
    let module = wasm(
        r#"(module
            (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
            (memory (export "memory") 2)
            (func (export "invoke") (param i32) (result i64)
              (call $write (i32.const 0) (i32.const 1) (i32.const 256) (i32.const 4097))
              (i64.const 0)))"#,
    );

    let outcome = vm
        .execute(&module, CONTRACT, &[], 1_000_000, MemoryState::default())
        .outcome;

    assert!(matches!(
        outcome,
        Err(VmError::SizeLimit {
            what: "storage value",
            actual: 4097,
            limit: 4096,
        })
    ));
}

#[test]
fn an_out_of_bounds_value_is_reported_before_an_oversized_key() {
    // Error precedence is part of the host's observable behaviour: the range
    // error comes before the size error, as it did when both were read first.
    let vm = Vm::new().expect("vm");
    let module = wasm(
        r#"(module
            (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (call $write (i32.const 0) (i32.const 200) (i32.const 65536) (i32.const 4))
              (i64.const 0)))"#,
    );

    let outcome = vm
        .execute(&module, CONTRACT, &[], 1_000_000, MemoryState::default())
        .outcome;

    assert!(matches!(outcome, Err(VmError::MemoryOutOfBounds { .. })));
}
