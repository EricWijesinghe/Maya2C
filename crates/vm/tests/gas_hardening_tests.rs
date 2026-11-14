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
        matches!(
            &outcome,
            Err(VmError::SizeLimit {
                what: "event topic",
                ..
            })
        ),
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
    // Exactly the page price: the modules differ in one immediate byte.
    assert_eq!(declared - one, 63 * gas::MEMORY_PAGE_FUEL);
    assert_eq!(grown - one, 63 * gas::MEMORY_PAGE_FUEL);
    // Growth past the module's own maximum is refused by wasmtime and never
    // held, so it is not charged.
    let capped = |grow: u32| {
        let code = wasm(&format!(
            r#"(module (memory (export "memory") 1 2)
                (func (export "invoke") (param i32) (result i64)
                  (drop (memory.grow (i32.const {grow})))
                  (i64.const 0)))"#
        ));
        Vm::new()
            .unwrap()
            .execute(&code, CONTRACT, &[], 50_000_000, MemoryState::at_height(1))
            .outcome
            .unwrap()
            .gas_used
    };
    assert_eq!(capped(63), capped(0));
}

#[test]
fn a_call_cannot_hold_more_memory_than_its_gas_pays_for() {
    let code = wasm(
        r#"(module (memory (export "memory") 256)
            (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#,
    );
    let vm = Vm::new().unwrap();
    let limit = gas::cold_call_fuel(&code).unwrap() + 10_000;
    let outcome = vm
        .execute(&code, CONTRACT, &[], limit, MemoryState::at_height(1))
        .outcome;
    assert!(
        matches!(outcome, Err(VmError::MemoryLimit { limit: 1, .. })),
        "{outcome:?}"
    );
}

#[test]
fn a_failed_call_reports_the_gas_it_used() {
    let vm = Vm::new().unwrap();
    let trap = wasm(
        r#"(module (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64) unreachable))"#,
    );
    let spin = wasm(
        r#"(module (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (loop $l (br $l)) (i64.const 0)))"#,
    );
    let limit = 50_000_000;
    let trapped = vm.execute(&trap, CONTRACT, &[], limit, MemoryState::at_height(1));
    assert!(trapped.outcome.is_err());
    // Its upfront price, its page and the unit or so wasmtime meters on the
    // way in: nowhere near the limit it named.
    let base = gas::cold_call_fuel(&trap).unwrap() + gas::MEMORY_PAGE_FUEL;
    assert!(
        (base..base + 16).contains(&trapped.gas_used),
        "{} against {base}",
        trapped.gas_used
    );
    let spun = vm.execute(&spin, CONTRACT, &[], limit, MemoryState::at_height(1));
    assert!(matches!(spun.outcome, Err(VmError::OutOfGas { .. })));
    // Its whole limit, and the page it held on top: memory is charged after
    // the compute, so a call that ran out did both (PR #549 review).
    assert_eq!(spun.gas_used, limit + gas::MEMORY_PAGE_FUEL);
    // Refused before anything ran: nothing used.
    let short = vm.execute(&trap, CONTRACT, &[], 10, MemoryState::at_height(1));
    assert_eq!(short.gas_used, 0);
}

#[test]
fn a_failed_call_reports_the_memory_it_touched_on_top_of_its_limit() {
    // Grow to every page the gas pays for, touch each one, then spin out of
    // gas. Before the review fix the figure was capped at the limit, hiding
    // up to 256 pages of page faults and zeroing (PR #549 review, MEDIUM).
    let code = wasm(
        r#"(module (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (local $p i32)
              (drop (memory.grow (i32.const 255)))
              (loop $touch
                (i32.store8 (local.get $p) (i32.const 1))
                (local.set $p (i32.add (local.get $p) (i32.const 4096)))
                (br_if $touch (i32.lt_u (local.get $p) (i32.const 16777216))))
              (loop $l (br $l))
              (i64.const 0)))"#,
    );
    let vm = Vm::new().unwrap();
    let limit = 50_000_000;
    let run = vm.execute(&code, CONTRACT, &[], limit, MemoryState::at_height(1));
    assert!(
        matches!(run.outcome, Err(VmError::OutOfGas { .. })),
        "{:?}",
        run.outcome
    );
    assert_eq!(run.gas_used, limit + 256 * gas::MEMORY_PAGE_FUEL);
}

#[test]
fn a_call_that_recurses_forever_reports_its_whole_limit() {
    // Where the native stack overflows depends on frame size, so on the ISA
    // and the compiler's flags: the fuel used by then is not the same on
    // every validator. The figure must not depend on it (PR #549 review,
    // HIGH): the whole limit, and every page the gas could have paid for.
    let code = wasm(
        r#"(module (memory (export "memory") 1)
            (func $down (param i64) (result i64)
              (call $down (i64.add (local.get 0) (i64.const 1))))
            (func (export "invoke") (param i32) (result i64)
              (call $down (i64.const 0))))"#,
    );
    let vm = Vm::new().unwrap();
    for limit in [10_000_000u64, 50_000_000] {
        let run = vm.execute(&code, CONTRACT, &[], limit, MemoryState::at_height(1));
        assert!(
            matches!(&run.outcome, Err(VmError::Trap(text)) if text.contains("stack")),
            "{:?}",
            run.outcome
        );
        let pages =
            ((limit - gas::cold_call_fuel(&code).unwrap()) / gas::MEMORY_PAGE_FUEL).min(256);
        assert_eq!(run.gas_used, limit + pages * gas::MEMORY_PAGE_FUEL);
    }
}

#[test]
fn a_module_with_a_wide_function_type_is_refused_at_deploy() {
    // The review's compile bomb, scaled down: a type naming more values than
    // MAX_TYPE_ARITY (16) is refused by the scan, before the compile. Before
    // the fix this validated, and 1,500 blocks of [1000] -> [1000] compiled
    // in 8.2 s for a price of 49.6 M fuel.
    let vm = Vm::new().unwrap();
    let wide = wasm(&format!(
        r#"(module (type $w (func (param {0}) (result {0})))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#,
        "i32 ".repeat(9)
    ));
    assert!(
        matches!(vm.validate(&wide), Err(VmError::SizeLimit { .. })),
        "{:?}",
        vm.validate(&wide)
    );
    assert!(gas::cold_call_fuel(&wide).is_err());
    // Within the bound, a typed block pays for the values it carries.
    let typed = |blocks: usize| {
        wasm(&format!(
            r#"(module (type $h (func (param {0}) (result {0})))
                (memory (export "memory") 1)
                (func (export "invoke") (param i32) (result i64)
                  {1} {2} {3} (i64.const 0)))"#,
            "i32 ".repeat(8),
            "(i32.const 1) ".repeat(8),
            "block (type $h) end ".repeat(blocks),
            "drop ".repeat(8),
        ))
    };
    let (one, two) = (typed(1), typed(2));
    assert!(vm.validate(&one).is_ok() && vm.validate(&two).is_ok());
    let step = gas::cold_call_fuel(&two).unwrap() - gas::cold_call_fuel(&one).unwrap();
    // One more block: its bytes, its control and its 16 carried values.
    let bytes = (two.len() - one.len()) as u64;
    assert_eq!(
        step,
        bytes * gas::CODE_BYTE_FUEL + gas::CONTROL_FUEL + 16 * gas::ARITY_FUEL
    );
    // A typed loop pays more per value: its params are fed again at every
    // back edge (2,000 loops of 16 params compiled in 823 ms, release).
    let looped = wasm(&format!(
        r#"(module (type $h (func (param {0}) (result {0})))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              {1} block (type $h) end loop (type $h) end {2} (i64.const 0)))"#,
        "i32 ".repeat(8),
        "(i32.const 1) ".repeat(8),
        "drop ".repeat(8),
    ));
    assert_eq!(looped.len(), two.len());
    assert_eq!(
        gas::cold_call_fuel(&looped).unwrap() - gas::cold_call_fuel(&one).unwrap(),
        bytes * gas::CODE_BYTE_FUEL + gas::CONTROL_FUEL + 16 * gas::LOOP_ARITY_FUEL
    );
}

#[test]
fn a_module_whose_compile_costs_more_than_any_cap_allows_is_refused() {
    // Every deployed module must stay callable under the lowest cap a vote
    // can set, so its cold price may not exceed the largest module's length
    // price.
    let functions = usize::try_from(gas::MAX_COLD_CALL_FUEL / gas::FUNCTION_FUEL).unwrap() + 1;
    let code = wasm(&format!("(module {})", "(func)".repeat(functions)));
    assert!(code.len() < maya_vm::MAX_MODULE_BYTES);
    let vm = Vm::new().unwrap();
    assert!(
        matches!(
            vm.validate(&code),
            Err(VmError::SizeLimit {
                what: "module compile price",
                ..
            })
        ),
        "{:?}",
        vm.validate(&code)
    );
    // And one too many control operators in one function, at any price.
    let branchy = wasm(&format!(
        "(module (func {}))",
        "(block)".repeat(maya_vm::shape::MAX_CONTROL_PER_FUNCTION + 1)
    ));
    assert!(matches!(
        vm.validate(&branchy),
        Err(VmError::SizeLimit {
            what: "function control operators",
            ..
        })
    ));
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
