//! The measurements the gas prices in `crates/vm/src/gas.rs` are derived from
//! (ADR-142).
//!
//! Each case runs one kind of work in a loop and prints wall time per call
//! and per unit of fuel. The derivation in ADR-142 takes the worst
//! nanoseconds-per-fuel figure printed here and sizes the per-block gas cap so
//! a block made only of that work still validates well inside the 1 s block
//! time.
//!
//! Timing assertions are flaky, so the tests assert only what is
//! deterministic (the fuel each case spends) and print the rates. Run with
//! `cargo test -p maya-vm --test gas_price_calibration_tests -- --nocapture
//! --test-threads 1`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::time::Instant;

use maya_vm::host::{MAX_KEY_BYTES, MAX_VALUE_BYTES};
use maya_vm::{MemoryState, Vm};

const CONTRACT: [u8; 32] = [7u8; 32];
const RUNS: usize = 3;

fn wasm(text: &str) -> Vec<u8> {
    wat::parse_str(text).expect("valid WAT")
}

/// Best-of-`RUNS` wall time and the fuel the call spent.
fn time_call(vm: &Vm, code: &[u8], state: impl Fn() -> MemoryState) -> (f64, u64) {
    let mut best = f64::MAX;
    let mut used = 0;
    for _ in 0..RUNS {
        let host = state();
        let started = Instant::now();
        let execution = vm.execute(code, CONTRACT, &[], u64::MAX / 2, host);
        let elapsed = started.elapsed().as_secs_f64();
        used = execution.outcome.expect("calibration call runs").gas_used;
        best = best.min(elapsed);
    }
    (best, used)
}

fn report(label: &str, seconds: f64, fuel: u64, calls: u64) {
    eprintln!(
        "CAL {label:<34} {:>10.3} ms  fuel {fuel:>12}  {:>9.1} ns/call  {:>8.3} ns/fuel",
        seconds * 1e3,
        seconds * 1e9 / calls as f64,
        seconds * 1e9 / fuel.max(1) as f64,
    );
}

/// A loop of `calls` iterations around `body`.
fn looped(imports: &str, data: &str, body: &str, calls: u32) -> Vec<u8> {
    wasm(&format!(
        r#"(module
            {imports}
            (memory (export "memory") 2)
            {data}
            (func (export "invoke") (param i32) (result i64)
              (local $i i32)
              (loop $again
                {body}
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br_if $again (i32.lt_u (local.get $i) (i32.const {calls}))))
              (i64.const 0)))"#
    ))
}

const CALLS: u32 = 20_000;

#[test]
fn calibrate_guest_compute() {
    let vm = Vm::new().unwrap();
    let cases = [
        ("guest: empty loop", ""),
        (
            "guest: i64.div_u chain",
            "(drop (i64.div_u (i64.div_u (i64.const -1) (i64.extend_i32_u (i32.add (local.get $i) (i32.const 3)))) (i64.const 7)))",
        ),
        (
            "guest: memory.fill 64 KiB",
            "(memory.fill (i32.const 0) (i32.const 1) (i32.const 65536))",
        ),
        (
            "guest: memory.copy 64 KiB",
            "(memory.copy (i32.const 0) (i32.const 65536) (i32.const 65536))",
        ),
    ];
    for (label, body) in cases {
        let code = looped("", "", body, CALLS);
        let (seconds, fuel) = time_call(&vm, &code, || MemoryState::at_height(1));
        assert!(fuel >= u64::from(CALLS), "{label} spent {fuel}");
        report(label, seconds, fuel, u64::from(CALLS));
    }
}

#[test]
fn calibrate_host_calls() {
    let vm = Vm::new().unwrap();
    let key = "k".repeat(MAX_KEY_BYTES);
    let cases: Vec<(&str, &str, String, &str)> = vec![
        (
            "host: block_height",
            r#"(import "env" "block_height" (func $f (result i64)))"#,
            String::new(),
            "(drop (call $f))",
        ),
        (
            "host: get_balance",
            r#"(import "env" "get_balance" (func $f (param i32) (result i64)))"#,
            String::new(),
            "(drop (call $f (i32.const 0)))",
        ),
        (
            "host: get_balance_u256",
            r#"(import "env" "get_balance_u256" (func $f (param i32 i32) (result i32)))"#,
            String::new(),
            "(drop (call $f (i32.const 0) (i32.const 64)))",
        ),
        (
            "host: caller",
            r#"(import "env" "caller" (func $f (param i32) (result i32)))"#,
            String::new(),
            "(drop (call $f (i32.const 0)))",
        ),
        (
            "host: oracle_feed_age (absent)",
            r#"(import "env" "oracle_feed_age" (func $f (param i32) (result i64)))"#,
            String::new(),
            "(drop (call $f (i32.const 0)))",
        ),
        (
            "host: storage_read miss 128 B key",
            r#"(import "env" "storage_read" (func $f (param i32 i32 i32 i32) (result i32)))"#,
            format!(r#"(data (i32.const 0) "{key}")"#),
            "(drop (call $f (i32.const 0) (i32.const 128) (i32.const 1024) (i32.const 4096)))",
        ),
    ];
    for (label, import, data, body) in cases {
        let code = looped(import, &data, body, CALLS);
        let (seconds, fuel) = time_call(&vm, &code, || {
            let mut state = MemoryState::at_height(1);
            state.caller = Some([1; 32]);
            state
        });
        report(label, seconds, fuel, u64::from(CALLS));
    }
}

#[test]
fn calibrate_storage_bytes() {
    let vm = Vm::new().unwrap();
    let key = "k".repeat(MAX_KEY_BYTES);
    let read = looped(
        r#"(import "env" "storage_read" (func $f (param i32 i32 i32 i32) (result i32)))"#,
        &format!(r#"(data (i32.const 0) "{key}")"#),
        "(drop (call $f (i32.const 0) (i32.const 128) (i32.const 1024) (i32.const 4096)))",
        CALLS,
    );
    let (seconds, fuel) = time_call(&vm, &read, || {
        let mut state = MemoryState::at_height(1);
        state.storage.insert(
            (CONTRACT, key.as_bytes().to_vec()),
            vec![5; MAX_VALUE_BYTES],
        );
        state
    });
    report(
        "host: storage_read hit 128 B + 4 KiB",
        seconds,
        fuel,
        u64::from(CALLS),
    );

    // Distinct keys, so every write inserts rather than overwrites.
    let write = looped(
        r#"(import "env" "storage_write" (func $f (param i32 i32 i32 i32)))"#,
        &format!(r#"(data (i32.const 0) "{key}")"#),
        "(i32.store (i32.const 0) (local.get $i)) (call $f (i32.const 0) (i32.const 128) (i32.const 1024) (i32.const 4096))",
        CALLS,
    );
    let (seconds, fuel) = time_call(&vm, &write, || MemoryState::at_height(1));
    report(
        "host: storage_write 128 B + 4 KiB",
        seconds,
        fuel,
        u64::from(CALLS),
    );

    let emit = looped(
        r#"(import "env" "emit_event" (func $f (param i32 i32 i32 i32)))"#,
        "",
        "(call $f (i32.const 0) (i32.const 4096) (i32.const 4096) (i32.const 4096))",
        64,
    );
    let (seconds, fuel) = time_call(&vm, &emit, || MemoryState::at_height(1));
    report("host: emit_event 4 KiB + 4 KiB (x64)", seconds, fuel, 64);
}

/// A module of about `target` bytes: many functions of straight-line i64
/// arithmetic, the shape that keeps Cranelift busiest per byte.
fn big_module(target: usize) -> Vec<u8> {
    let mut body = String::new();
    for i in 0..200 {
        body.push_str(&format!(
            "(local.set $a (i64.add (i64.mul (local.get $a) (i64.const {i})) (local.get $b)))\
             (local.set $b (i64.xor (local.get $b) (i64.rotl (local.get $a) (i64.const 7))))"
        ));
    }
    let mut funcs = String::new();
    let mut n = 0;
    loop {
        funcs.push_str(&format!(
            "(func $f{n} (param $a i64) (param $b i64) (result i64) {body} (local.get $a))"
        ));
        n += 1;
        let code = wasm(&format!(
            r#"(module (memory (export "memory") 1) {funcs}
                (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#
        ));
        if code.len() >= target {
            return code;
        }
    }
}

/// One function of about `target` bytes: a single body with many locals, the
/// shape that loads the register allocator rather than the function count.
fn one_big_function(target: usize) -> Vec<u8> {
    let locals = "(local $c i64) (local $d i64) (local $e i64) (local $f i64)";
    let step = "(local.set $c (i64.add (local.get $a) (local.get $d)))\
                (local.set $d (i64.mul (local.get $c) (local.get $e)))\
                (local.set $e (i64.xor (local.get $f) (local.get $c)))\
                (local.set $f (i64.sub (local.get $e) (local.get $b)))";
    let mut body = String::new();
    loop {
        for _ in 0..500 {
            body.push_str(step);
        }
        let code = wasm(&format!(
            r#"(module (memory (export "memory") 1)
                (func $big (param $a i64) (param $b i64) (result i64) {locals} {body} (local.get $f))
                (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#
        ));
        if code.len() >= target {
            return code;
        }
    }
}

/// A module of about `target` bytes made of one huge `br_table`.
fn big_br_table(target: usize) -> Vec<u8> {
    let labels = "0 ".repeat(target / 2);
    wasm(&format!(
        r#"(module (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (block (br_table {labels} 0 (local.get 0)))
              (i64.const 0)))"#
    ))
}

#[test]
fn calibrate_compile_and_instantiate() {
    let started = Instant::now();
    let vm = Vm::new().unwrap();
    let engine = started.elapsed().as_secs_f64();
    eprintln!(
        "CAL Vm::new (engine + cache)           {:>10.3} ms",
        engine * 1e3
    );

    for (label, code) in [
        ("many functions", big_module(256 * 1024)),
        ("one function", one_big_function(256 * 1024)),
        ("br_table", big_br_table(256 * 1024)),
    ] {
        // Fresh VM each run, so every run is a cache miss: the compile is paid.
        let mut best = f64::MAX;
        for _ in 0..RUNS {
            let vm = Vm::new().unwrap();
            let started = Instant::now();
            let outcome = vm
                .execute(&code, CONTRACT, &[], 100_000_000, MemoryState::at_height(1))
                .outcome;
            best = best.min(started.elapsed().as_secs_f64());
            // A refusal still paid the compile; only report it.
            if let Err(error) = outcome {
                eprintln!("CAL   ({label} refused after compile: {error})");
            }
        }
        eprintln!(
            "CAL cold call, {label:<15} {:>7} B {:>10.3} ms  {:>8.1} ns/byte",
            code.len(),
            best * 1e3,
            best * 1e9 / code.len() as f64
        );
    }

    let tiny = wasm(
        r#"(module (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#,
    );
    let _ = vm.execute(&tiny, CONTRACT, &[], 1_000_000, MemoryState::at_height(1));
    let mut best = f64::MAX;
    for _ in 0..200 {
        let started = Instant::now();
        vm.execute(&tiny, CONTRACT, &[], 1_000_000, MemoryState::at_height(1))
            .outcome
            .expect("runs");
        best = best.min(started.elapsed().as_secs_f64());
    }
    eprintln!(
        "CAL warm call, empty contract           {:>10.3} us",
        best * 1e6
    );
}
