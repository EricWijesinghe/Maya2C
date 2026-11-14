//! Review findings on ADR-142, pinned (PR #549 review, 2026-11-14).
//!
//! Each test is a way the first cut of the gas rules let work run below its
//! price: a refused host call that did not end the call, an event topic with
//! no size bound, linear memory nobody paid for, and a module shape that
//! compiled far slower per byte than its price.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_vm::{MemoryState, Vm, VmError, gas};

const CONTRACT: [u8; 32] = [4u8; 32];

fn wasm(text: &str) -> Vec<u8> {
    wat::parse_str(text).expect("valid WAT")
}

/// Runs `body` then writes `k` = `v`: if the write landed, the call kept
/// running after `body` was refused.
fn refused_then_write(imports: &str, body: &str) -> (Result<u64, VmError>, bool) {
    let code = wasm(&format!(
        r#"(module
            (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
            {imports}
            (memory (export "memory") 1)
            (data (i32.const 0) "kv")
            (func (export "invoke") (param i32) (result i64)
              {body}
              (call $write (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1))
              (i64.const 0)))"#
    ));
    let vm = Vm::new().unwrap();
    let execution = vm.execute(&code, CONTRACT, &[], 50_000_000, MemoryState::at_height(1));
    let wrote = execution
        .state
        .storage_get_for_test(&CONTRACT, b"k")
        .is_some();
    (execution.outcome.map(|o| o.gas_used), wrote)
}

#[test]
fn a_refused_host_call_ends_the_call() {
    // Before: the refusal was recorded and the guest kept running, so a loop
    // of refused calls bought host work at one unit of fuel each.
    let cases = [
        (
            "oversized storage_read key",
            r#"(import "env" "storage_read" (func $f (param i32 i32 i32 i32) (result i32)))"#,
            "(drop (call $f (i32.const 0) (i32.const 200) (i32.const 0) (i32.const 0)))",
            "storage key",
        ),
        (
            "oversized storage_write value",
            r#"(import "env" "storage_write" (func $f (param i32 i32 i32 i32)))"#,
            "(call $f (i32.const 0) (i32.const 1) (i32.const 0) (i32.const 5000))",
            "storage value",
        ),
        (
            "oversized event payload",
            r#"(import "env" "emit_event" (func $f (param i32 i32 i32 i32)))"#,
            "(call $f (i32.const 0) (i32.const 1) (i32.const 0) (i32.const 5000))",
            "event payload",
        ),
        (
            "event past the count cap",
            r#"(import "env" "emit_event" (func $f (param i32 i32 i32 i32)))"#,
            "(local $i i32)
             (loop $l
               (call $f (i32.const 0) (i32.const 1) (i32.const 0) (i32.const 1))
               (local.set $i (i32.add (local.get $i) (i32.const 1)))
               (br_if $l (i32.lt_u (local.get $i) (i32.const 65))))",
            "event count",
        ),
    ];
    for (label, import, body, what) in cases {
        let (outcome, wrote) = refused_then_write(import, body);
        assert!(
            matches!(&outcome, Err(VmError::SizeLimit { what: w, .. }) if *w == what),
            "{label}: {outcome:?}"
        );
        assert!(!wrote, "{label}: the call kept running after the refusal");
    }
}

#[test]
fn an_event_topic_over_the_cap_is_refused() {
    // Before: only the payload was size-checked, so a topic could be the
    // guest's whole memory, held twice by the host.
    let (outcome, wrote) = refused_then_write(
        r#"(import "env" "emit_event" (func $f (param i32 i32 i32 i32)))"#,
        "(call $f (i32.const 0) (i32.const 1000) (i32.const 0) (i32.const 1))",
    );
    assert!(
        matches!(&outcome, Err(VmError::SizeLimit { what: "event topic", .. })),
        "{outcome:?}"
    );
    assert!(!wrote);
}

fn gas_with_memory(pages: u32, grow: u32) -> Result<u64, VmError> {
    let code = wasm(&format!(
        r#"(module (memory (export "memory") {pages})
            (func (export "invoke") (param i32) (result i64)
              (drop (memory.grow (i32.const {grow})))
              (i64.const 0)))"#
    ));
    let vm = Vm::new().unwrap();
    vm.execute(&code, CONTRACT, &[], 50_000_000, MemoryState::at_height(1))
        .outcome
        .map(|o| o.gas_used)
}

#[test]
fn a_call_pays_for_the_linear_memory_it_holds() {
    // Before: memory cost nothing beyond the `memory.grow` operator, and
    // touching a fresh page measured ~14 ns per unit of fuel.
    let one = gas_with_memory(1, 0).unwrap();
    let declared = gas_with_memory(64, 0).unwrap();
    let grown = gas_with_memory(1, 63).unwrap();
    assert!(declared > one + 63 * 1_000, "{one} vs {declared}");
    assert!(grown > one + 63 * 1_000, "{one} vs {grown}");
}

#[test]
fn a_call_cannot_hold_more_memory_than_its_gas_pays_for() {
    let code = wasm(
        r#"(module (memory (export "memory") 256)
            (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#,
    );
    let vm = Vm::new().unwrap();
    let limit = gas::call_fuel(code.len(), false) + 10_000;
    let outcome = vm
        .execute(&code, CONTRACT, &[], limit, MemoryState::at_height(1))
        .outcome;
    assert!(outcome.is_err(), "{outcome:?}");
}

/// A binary module: `functions` bodies of `locals` i64 locals each.
fn locals_module(functions: u32, locals: u32) -> Vec<u8> {
    let mut out = b"\0asm\x01\0\0\0".to_vec();
    let leb = |mut v: u64, out: &mut Vec<u8>| loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    };
    let section = |id: u8, payload: Vec<u8>, out: &mut Vec<u8>| {
        out.push(id);
        leb(payload.len() as u64, out);
        out.extend(payload);
    };
    section(1, vec![1, 0x60, 0, 0], &mut out);
    let mut funcs = Vec::new();
    leb(u64::from(functions), &mut funcs);
    funcs.extend(std::iter::repeat_n(0u8, functions as usize));
    section(3, funcs, &mut out);
    let mut body = vec![1];
    leb(u64::from(locals), &mut body);
    body.extend([0x7e, 0x0b]);
    let mut code = Vec::new();
    leb(u64::from(functions), &mut code);
    for _ in 0..functions {
        leb(body.len() as u64, &mut code);
        code.extend(&body);
    }
    section(10, code, &mut out);
    out
}

#[test]
fn a_module_with_too_many_locals_is_refused_before_it_compiles() {
    // Measured by the review: 200 functions of 50,000 locals each, 1,662
    // bytes, compiled in ~980 ms against a price of ~0.8 ms.
    let vm = Vm::new().unwrap();
    let outcome = vm.validate(&locals_module(200, 50_000));
    assert!(
        matches!(outcome, Err(VmError::SizeLimit { .. })),
        "{outcome:?}"
    );
}
