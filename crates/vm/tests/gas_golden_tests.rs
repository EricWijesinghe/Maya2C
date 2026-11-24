//! Gas golden vectors: the exact fuel each module costs on the launch tier.
//!
//! wasmtime does not promise stable fuel accounting across releases, so the
//! pin in `crates/vm/Cargo.toml` is consensus-critical. The other suites show
//! gas is deterministic (same call, same gas) and identical across tiers; they
//! do not notice the whole schedule moving by one unit. These numbers do. They
//! were recorded on wasmtime 48.0.8 before the 48.0.10 bump and held
//! unchanged across it on Windows. If a wasmtime bump changes one, the bump is
//! a gas schedule change and needs to be treated as one, not re-recorded
//! quietly. One vector was re-recorded, for a config change rather than a
//! bump: "host calls" has a data segment, and turning copy-on-write memory
//! images off (ADR-034 amendment 2026-11-23) made its gas the same on every
//! OS. The reason is written next to it.
//!
//! All seven were re-recorded once more when ADR-142 (PR #549) landed: the
//! gas a call reports now includes the priced compile, instantiation and
//! host work (`crate::gas`) on top of the fuel, and a call whose metered work
//! passes its limit fails, so each least sufficient limit equals the gas. The
//! outputs did not move. That is a schedule change by design, recorded with
//! the ADR, not a wasmtime drift. Then the call-depth bound (re-freeze R-6,
//! `maya_vm::depth`) added its bookkeeping, 12 units for every function
//! call, `invoke` included: 12 on each vector, more on the one that calls.
//!
//! Each vector also pins its boundary: the smallest gas limit the call
//! succeeds with. wasmtime checks fuel at function entries and loop
//! back-edges, not after every instruction, so a call can finish on a little
//! less than it costs (it then reports its whole limit as used). Where those
//! checks sit decides which calls run out, so it is part of the schedule too.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_vm::host::MemoryState;
use maya_vm::runtime::Vm;

const CONTRACT: [u8; 32] = [5u8; 32];
const AMPLE_GAS: u64 = 50_000_000;

/// (name, WAT, gas, least sufficient gas limit, output) recorded on wasmtime
/// 48.0.8, Cranelift. Each module returns its result as 8 little-endian output
/// bytes at address 0, so the value it computed is pinned with what it cost.
const VECTORS: &[(&str, &str, u64, u64, u64)] = &[
    (
        "empty",
        r#"(module (memory (export "memory") 1)
          (func (export "invoke") (param i32) (result i64) (i64.const 8)))"#,
        210_314,
        210_314,
        0,
    ),
    (
        "i64 arithmetic loop",
        r#"(module (memory (export "memory") 1)
          (func (export "invoke") (param i32) (result i64)
            (local $i i64) (local $acc i64)
            (loop $l
              (local.set $acc (i64.add (local.get $acc)
                (i64.rem_u (i64.mul (local.get $i) (i64.const 7919)) (i64.const 104729))))
              (local.set $i (i64.add (local.get $i) (i64.const 1)))
              (br_if $l (i64.lt_u (local.get $i) (i64.const 5000))))
            (i64.store (i32.const 0) (local.get $acc)) (i64.const 8)))"#,
        382_607,
        382_607,
        0x0f9f_1e3e,
    ),
    (
        // Direct calls only: the VM refuses every table, so `call_indirect`
        // never reaches a contract.
        "calls and recursion",
        r#"(module (memory (export "memory") 1)
          (func $fib (param i64) (result i64)
            (if (result i64) (i64.lt_u (local.get 0) (i64.const 2))
              (then (local.get 0))
              (else (i64.add
                (call $fib (i64.sub (local.get 0) (i64.const 1)))
                (call $fib (i64.sub (local.get 0) (i64.const 2)))))))
          (func $double (param i64) (result i64) (i64.shl (local.get 0) (i64.const 1)))
          (func (export "invoke") (param i32) (result i64)
            (i64.store (i32.const 0) (call $double (call $fib (i64.const 15))))
            (i64.const 8)))"#,
        663_337,
        663_337,
        0x4c4,
    ),
    (
        "br_table, select and globals",
        r#"(module (memory (export "memory") 1)
          (global $g (mut i32) (i32.const 0))
          (func (export "invoke") (param i32) (result i64)
            (local $i i32)
            (loop $l
              (block $c (block $b (block $a
                (br_table $a $b $c (i32.rem_u (local.get $i) (i32.const 3))))
                (global.set $g (i32.add (global.get $g) (i32.const 1))) (br $c))
                (global.set $g (select (i32.const 5) (global.get $g) (local.get $i))))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br_if $l (i32.lt_u (local.get $i) (i32.const 3000))))
            (i64.store (i32.const 0) (i64.extend_i32_u (global.get $g))) (i64.const 8)))"#,
        516_438,
        516_438,
        0x5,
    ),
    (
        "memory grow, loads, stores and bulk ops",
        r#"(module (memory (export "memory") 1)
          (func (export "invoke") (param i32) (result i64)
            (local $i i32)
            (drop (memory.grow (i32.const 2)))
            (memory.fill (i32.const 0) (i32.const 9) (i32.const 100000))
            (memory.copy (i32.const 120000) (i32.const 0) (i32.const 40000))
            (loop $l
              (i64.store (i32.mul (local.get $i) (i32.const 8))
                (i64.add (i64.load offset=120000 (local.get $i)) (i64.const 1)))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br_if $l (i32.lt_u (local.get $i) (i32.const 2000))))
            (i64.store (i32.const 0) (i64.load (i32.const 800))) (i64.const 8)))"#,
        500_398,
        500_398,
        0x0909_0909_0909_090a,
    ),
    (
        "floats with NaN canonicalisation",
        r#"(module (memory (export "memory") 1)
          (func (export "invoke") (param i32) (result i64)
            (local $x f64) (local $i i32)
            (local.set $x (f64.const 2))
            (loop $l
              (local.set $x (f64.add (f64.sqrt (local.get $x)) (f64.div (f64.const 0) (f64.const 0))))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br_if $l (i32.lt_u (local.get $i) (i32.const 500))))
            (i64.store (i32.const 0) (i64.reinterpret_f64 (local.get $x))) (i64.const 8)))"#,
        319_560,
        319_560,
        0x7ff8_0000_0000_0000,
    ),
    (
        "host calls: storage and events",
        r#"(module
          (import "env" "storage_write" (func $w (param i32 i32 i32 i32)))
          (import "env" "emit_event" (func $e (param i32 i32 i32 i32)))
          (memory (export "memory") 1)
          (data (i32.const 0) "key0value-0topic")
          (func (export "invoke") (param i32) (result i64)
            (local $i i32)
            (loop $l
              (i32.store8 (i32.const 3) (i32.add (i32.const 48) (local.get $i)))
              (call $w (i32.const 0) (i32.const 4) (i32.const 4) (i32.const 7))
              (call $e (i32.const 11) (i32.const 5) (i32.const 4) (i32.const 7))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br_if $l (i32.lt_u (local.get $i) (i32.const 8))))
            (i64.const 8)))"#,
        // Re-recorded 2026-11-23 after `memory_init_cow(false)` (ADR-034
        // amendment): 204 = 20 for instantiating the 16-byte segment (pinned
        // by `a_data_segment_costs_the_same_gas_on_every_host_and_tier`) plus
        // 184 for the loop and its 16 host calls. The old 4,283 was a Windows
        // copy-on-write artefact (4,099 for the padded 4 KiB page + 184);
        // Linux measured 186 (2 + 184) for the same module.
        691_102,
        691_102,
        0x756c_6176_3779_656b,
    ),
];

/// The gas used and the 8 output bytes, or why the call failed.
fn run(vm: &Vm, code: &[u8], limit: u64) -> Result<(u64, u64), String> {
    let outcome = vm
        .execute(code, CONTRACT, &[], limit, MemoryState::at_height(7))
        .outcome
        .map_err(|e| e.to_string())?;
    let output: [u8; 8] = outcome
        .output
        .as_slice()
        .try_into()
        .expect("8 output bytes");
    Ok((outcome.gas_used, u64::from_le_bytes(output)))
}

#[test]
fn every_vector_costs_exactly_its_recorded_gas() {
    let vm = Vm::new().expect("vm");
    let mut drift = Vec::new();
    for (name, text, gas, _, output) in VECTORS {
        let code = wat::parse_str(text).expect("valid WAT");
        let measured = run(&vm, &code, AMPLE_GAS).unwrap_or_else(|e| panic!("{name}: {e}"));
        if measured != (*gas, *output) {
            drift.push(format!(
                "{name}: recorded ({gas}, {output:#x}), measured ({}, {:#x})",
                measured.0, measured.1
            ));
        }
    }
    assert!(
        drift.is_empty(),
        "the gas schedule moved:\n{}",
        drift.join("\n")
    );
}

/// The smallest limit in `0..=gas` the call succeeds with (success is
/// monotone in the limit; the test below checks both sides of the answer).
fn least_sufficient_gas(vm: &Vm, code: &[u8], gas: u64) -> u64 {
    let (mut lo, mut hi) = (0, gas);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if run(vm, code, mid).is_ok() {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

#[test]
fn every_vector_runs_out_exactly_below_its_recorded_boundary() {
    let vm = Vm::new().expect("vm");
    let mut drift = Vec::new();
    for (name, text, gas, least, _) in VECTORS {
        let code = wat::parse_str(text).expect("valid WAT");
        let measured = least_sufficient_gas(&vm, &code, *gas);
        assert!(run(&vm, &code, measured).is_ok(), "{name} at {measured}");
        if measured > 0 {
            assert!(
                run(&vm, &code, measured - 1).is_err(),
                "{name} below {measured}"
            );
        }
        if measured != *least {
            drift.push(format!("{name}: recorded {least}, measured {measured}"));
        }
    }
    assert!(
        drift.is_empty(),
        "the out-of-gas boundary moved:\n{}",
        drift.join("\n")
    );
}
