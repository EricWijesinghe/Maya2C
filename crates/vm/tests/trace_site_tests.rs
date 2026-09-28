//! `Vm::execute_traced` records each host call's site, and tracing costs no
//! gas: the debugger's view must be the same call the chain would run.

#![allow(clippy::unwrap_used)]

use maya_vm::host::MemoryState;
use maya_vm::runtime::Vm;
use maya_vm::trace::Traced;

const CONTRACT: [u8; 32] = [0x31; 32];

/// Two storage writes and an event, each from a distinct call instruction.
fn module() -> Vec<u8> {
    wat::parse_str(
        r#"(module
          (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
          (import "env" "emit_event" (func $emit (param i32 i32 i32 i32)))
          (memory (export "memory") 1)
          (data (i32.const 0) "k1k2vv")
          (func (export "invoke") (param i32) (result i64)
            (call $write (i32.const 0) (i32.const 2) (i32.const 4) (i32.const 2))
            (call $write (i32.const 2) (i32.const 2) (i32.const 4) (i32.const 2))
            (call $emit (i32.const 0) (i32.const 2) (i32.const 4) (i32.const 2))
            (i64.const 0)))"#,
    )
    .unwrap()
}

#[test]
fn a_traced_call_burns_identical_gas_and_records_distinct_sites() {
    let vm = Vm::new().unwrap();
    let code = module();
    let plain = vm.execute(&code, CONTRACT, &[], 1_000_000, MemoryState::at_height(1));
    let traced = vm.execute_traced(
        &code,
        CONTRACT,
        &[],
        1_000_000,
        Traced::new(MemoryState::at_height(1)),
    );
    let (a, b) = (plain.outcome.unwrap(), traced.outcome.unwrap());
    assert_eq!(a.gas_used, b.gas_used, "tracing charges nothing");
    let (_, timeline) = traced.state.finish();
    let sites = timeline.sites();
    assert_eq!(sites.len(), timeline.steps().len());
    assert!(
        sites
            .iter()
            .all(|s| s.offset.is_some() && s.fuel_left.is_some()),
        "every step has a call site and a fuel reading: {sites:?}"
    );
    // Fuel only falls, and what is left at the last host call is at least
    // what is left at the end.
    assert!(sites.windows(2).all(|w| w[0].fuel_left >= w[1].fuel_left));
    assert!(sites.last().unwrap().fuel_left.unwrap() >= 1_000_000 - b.gas_used);
    // The two writes record their before-values with a read each; what
    // matters is that the three call instructions give three distinct sites,
    // in increasing order through the function.
    let mut distinct: Vec<usize> = sites.iter().filter_map(|s| s.offset).collect();
    distinct.dedup();
    assert_eq!(distinct.len(), 3, "{sites:?}");
    assert!(distinct.windows(2).all(|w| w[0] < w[1]));
}
