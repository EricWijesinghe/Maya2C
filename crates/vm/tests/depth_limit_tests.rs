//! The call-depth bound (re-freeze R-6, ADR-142 amendment 8).
//!
//! Before it, recursion stopped where wasmtime's native stack check fired,
//! and that point belongs to the validator's binary: a call recursing to
//! between two binaries' limits applied on one and failed on the other. These
//! tests pin that a call now stops at a depth computed from the wasm alone,
//! and that the native check never gets there first, for every frame shape
//! measured. CI runs them on x86-64 Linux, x86-64 Windows and aarch64, which
//! is the cross-binary part of the claim.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;

use maya_vm::config::MAX_STACK_BYTES;
use maya_vm::depth::{DEPTH_LIMIT_SLOTS, frame_slots};
use maya_vm::host::MemoryState;
use maya_vm::runtime::Vm;
use maya_vm::{Tier, VmError};

const CONTRACT: [u8; 32] = [9u8; 32];
const GAS: u64 = 200_000_000;

/// A recursion whose frames hold `locals` locals and `stack` operand values
/// live across the recursive call, so a native frame has to keep them all.
/// Each is a load: the compiler may not move a load past a call (the callee
/// can store), so it cannot recompute the value after the call instead of
/// keeping it, as it does with arithmetic on the param.
/// `invoke` recurses `depth` deep; `down` is exported for the native probe.
fn recursion(locals: usize, stack: usize, depth: u64) -> Vec<u8> {
    let mut body = String::new();
    for i in 0..locals {
        write!(
            body,
            "i32.const 0 i64.load offset={} local.set {} ",
            8 * i,
            i + 1
        )
        .unwrap();
    }
    for i in 0..stack {
        write!(body, "i32.const 0 i64.load offset={} ", 8 * (i + locals)).unwrap();
    }
    body += "local.get 0 i64.eqz if (result i64) i64.const 0 else \
             local.get 0 i64.const 1 i64.sub call $down end ";
    body += &"i64.add ".repeat(stack);
    for i in 0..locals {
        write!(body, "local.get {} i64.add ", i + 1).unwrap();
    }
    let decl = if locals == 0 {
        String::new()
    } else {
        format!("(local{})", " i64".repeat(locals))
    };
    wat::parse_str(format!(
        r#"(module (memory (export "memory") 1)
          (func $down (export "down") (param i64) (result i64) {decl} {body})
          (func (export "invoke") (param i32) (result i64)
            (drop (call $down (i64.const {depth}))) (i64.const 0)))"#
    ))
    .unwrap()
}

/// Frame shapes: (locals, live operand values).
const SHAPES: &[(usize, usize)] = &[
    (0, 0),
    (16, 0),
    (256, 0),
    (1_000, 0),
    (0, 16),
    (0, 256),
    (0, 1_000),
    (100, 100),
];

/// The deepest `depth` the bound admits for this shape: `invoke`'s frame,
/// then one `down` frame per level plus the last one (depth 0).
fn admitted(locals: usize, stack: usize) -> u64 {
    let slots = frame_slots(&recursion(locals, stack, 0)).unwrap();
    let (down, invoke) = (u64::from(slots[0]), u64::from(slots[1]));
    (u64::from(DEPTH_LIMIT_SLOTS) - invoke) / down - 1
}

fn call(vm: &Vm, code: &[u8]) -> Result<u64, VmError> {
    vm.execute(code, CONTRACT, &[], GAS, MemoryState::at_height(7))
        .outcome
        .map(|o| o.gas_used)
}

fn vms() -> [(&'static str, Vm); 2] {
    [
        ("cranelift", Vm::new().unwrap()),
        ("pulley", Vm::with_tier(Tier::Pulley).unwrap()),
    ]
}

#[test]
fn a_recursion_stops_at_the_depth_the_wasm_names_on_every_tier() {
    for (tier, vm) in vms() {
        for &(locals, stack) in SHAPES {
            let deepest = admitted(locals, stack);
            let shape = format!("{tier} ({locals} locals, {stack} live values)");
            call(&vm, &recursion(locals, stack, deepest))
                .unwrap_or_else(|e| panic!("{shape}: depth {deepest} must complete: {e}"));
            let past = call(&vm, &recursion(locals, stack, deepest + 1));
            let Err(VmError::Trap(text)) = past else {
                panic!("{shape}: depth {} must trap, got {past:?}", deepest + 1);
            };
            // The bound's own `unreachable`, not the native stack check.
            assert!(
                text.contains("unreachable") && !text.contains("stack overflow"),
                "{shape}: {text}"
            );
        }
    }
}

#[test]
fn the_minimal_frame_admits_a_useful_depth() {
    // Not a consensus fact by itself, a usability floor: Rust contracts
    // recurse, and a bound that stopped at a few dozen frames would refuse
    // ordinary code.
    assert!(admitted(0, 0) >= 1_000, "{}", admitted(0, 0));
}

/// The deepest recursion of `down` the engine runs with no bound applied:
/// where the native stack check fires on this binary.
fn native_depth(locals: usize, stack: usize) -> u64 {
    let engine = maya_vm::deterministic_engine().unwrap();
    let module = wasmtime::Module::new(&engine, recursion(locals, stack, 0)).unwrap();
    let runs = |depth: u64| {
        let mut store = wasmtime::Store::new(&engine, ());
        store.set_fuel(u64::MAX / 4).unwrap();
        let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
        let down = instance
            .get_typed_func::<i64, i64>(&mut store, "down")
            .unwrap();
        match down.call(&mut store, i64::try_from(depth).unwrap()) {
            Ok(_) => true,
            Err(e) => {
                let trap = e.downcast_ref::<wasmtime::Trap>();
                assert_eq!(trap, Some(&wasmtime::Trap::StackOverflow), "{e:?}");
                false
            }
        }
    };
    let (mut lo, mut hi) = (0u64, 1u64 << 20);
    while lo + 1 < hi {
        let mid = lo + (hi - lo) / 2;
        if runs(mid) { lo = mid } else { hi = mid }
    }
    lo
}

#[test]
fn the_native_stack_check_is_never_reached_inside_the_bound() {
    // For each shape, the native stack one frame uses (the native boundary
    // spread over its frames) times the frames the bound admits, plus one
    // for `invoke`. That must stay under a third of `max_wasm_stack` on
    // every binary CI builds, so a binary with frames up to three times this
    // one's still traps on the bound first.
    let mut report = Vec::new();
    for &(locals, stack) in SHAPES {
        let native = native_depth(locals, stack);
        let per_frame = MAX_STACK_BYTES as u64 / (native + 1);
        let used = per_frame * (admitted(locals, stack) + 2);
        let slots = frame_slots(&recursion(locals, stack, 0)).unwrap()[0];
        println!(
            "{locals:>5} locals {stack:>5} values: native depth {native}, \
             {per_frame} B/frame, {slots} slots/frame, {} B/slot, \
             admitted {}, {used} B at the bound",
            per_frame / u64::from(slots),
            admitted(locals, stack)
        );
        if used > MAX_STACK_BYTES as u64 / 3 {
            report.push(format!("({locals}, {stack}): {used} B at the bound"));
        }
    }
    assert!(report.is_empty(), "{report:#?}");
}
