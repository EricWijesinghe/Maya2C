//! Compile and memory work in shapes built to be slow per byte, against
//! their price (ADR-142 review, 2026-11-14).
//!
//! `gas_price_calibration_tests.rs` measured three ordinary module shapes.
//! These are the adversarial ones: many tiny functions, many locals, deep
//! nesting, many branches, many imports, and fresh linear memory. Timing is
//! machine-dependent, so the timing test is `#[ignore]`d and prints; run it
//! with `cargo test -p maya-vm --test gas_adversarial_calibration_tests --
//! --ignored --nocapture --test-threads 1`. ADR-142 records the figures.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use std::time::Instant;

use maya_vm::{MemoryState, Vm, gas};

const RUNS: usize = 3;

fn leb(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn section(id: u8, payload: &[u8], out: &mut Vec<u8>) {
    out.push(id);
    leb(payload.len() as u64, out);
    out.extend_from_slice(payload);
}

/// A module of `functions` copies of one body (`locals` i64 locals, then
/// `body`, then `end`), optionally all exported, after `imports` imports of
/// `env.block_height`. Type 0 is `() -> ()`, type 1 `() -> i64`.
fn shape(imports: u32, functions: u32, locals: u32, body: &[u8], exported: bool) -> Vec<u8> {
    let mut out = b"\0asm\x01\0\0\0".to_vec();
    section(1, &[2, 0x60, 0, 0, 0x60, 0, 1, 0x7e], &mut out);
    if imports > 0 {
        let mut payload = Vec::new();
        leb(u64::from(imports), &mut payload);
        for _ in 0..imports {
            payload.extend([3, b'e', b'n', b'v', 12]);
            payload.extend(b"block_height");
            payload.extend([0, 1]);
        }
        section(2, &payload, &mut out);
    }
    let mut payload = Vec::new();
    leb(u64::from(functions), &mut payload);
    payload.extend(std::iter::repeat_n(0u8, functions as usize));
    section(3, &payload, &mut out);
    if exported {
        let mut payload = Vec::new();
        leb(u64::from(functions), &mut payload);
        for i in 0..functions {
            let name = format!("f{i}");
            leb(name.len() as u64, &mut payload);
            payload.extend(name.as_bytes());
            payload.push(0);
            leb(u64::from(imports + i), &mut payload);
        }
        section(7, &payload, &mut out);
    }
    let mut one = Vec::new();
    if locals == 0 {
        one.push(0);
    } else {
        one.push(1);
        leb(u64::from(locals), &mut one);
        one.push(0x7e);
    }
    one.extend_from_slice(body);
    one.push(0x0b);
    let mut payload = Vec::new();
    leb(u64::from(functions), &mut payload);
    for _ in 0..functions {
        leb(one.len() as u64, &mut payload);
        payload.extend(&one);
    }
    section(10, &payload, &mut out);
    out
}

fn nested_blocks(depth: usize) -> Vec<u8> {
    let mut body = [0x02, 0x40].repeat(depth);
    body.extend(std::iter::repeat_n(0x0b, depth));
    body
}

fn branches(count: usize) -> Vec<u8> {
    // i32.const 1; if; nop; end
    [0x41, 0x01, 0x04, 0x40, 0x01, 0x0b].repeat(count)
}

fn locals_in_a_loop(locals: u32, rounds: usize) -> Vec<u8> {
    let mut body = vec![0x03, 0x40];
    for _ in 0..rounds {
        for i in 0..locals {
            body.push(0x20);
            leb(u64::from(i), &mut body);
            body.push(0x21);
            leb(u64::from((i + 1) % locals), &mut body);
        }
    }
    body.push(0x0b);
    body
}

fn br_table(labels: usize) -> Vec<u8> {
    // block; i32.const 0; br_table 0 0 ... 0; end
    let mut body = vec![0x02, 0x40, 0x41, 0x00, 0x0e];
    leb(labels as u64, &mut body);
    body.extend(std::iter::repeat_n(0u8, labels + 1));
    body.push(0x0b);
    body
}

fn report_compile(label: &str, code: &[u8], functions: u64, locals: u64) {
    let mut best = f64::MAX;
    for _ in 0..RUNS {
        let vm = Vm::new().unwrap();
        let started = Instant::now();
        // Validation is the full compile; a refusal after it still paid it.
        let _ = vm.validate(code);
        best = best.min(started.elapsed().as_secs_f64());
    }
    let price = gas::call_fuel(code.len(), false);
    eprintln!(
        "ADV {label:<36} {:>8} B {functions:>6} fn {locals:>9} loc {:>10.3} ms {:>10.1} ns/B \
         price {price:>11} {:>8.3} ns/fuel {:>8.2} us/fn {:>7.2} ns/local",
        code.len(),
        best * 1e3,
        best * 1e9 / code.len() as f64,
        best * 1e9 / price as f64,
        best * 1e6 / functions.max(1) as f64,
        best * 1e9 / locals.max(1) as f64,
    );
}

#[test]
#[ignore = "timing; run with --ignored --nocapture and record the figures in ADR-142"]
fn calibrate_adversarial_compile_shapes() {
    for (label, imports, functions, locals, body, exported) in [
        ("empty functions", 0, 20_000, 0, Vec::new(), false),
        ("empty functions, exported", 0, 10_000, 0, Vec::new(), true),
        ("imports", 10_000, 1, 0, Vec::new(), false),
        ("locals 2000 x 1024", 0, 2_000, 1_024, Vec::new(), false),
        ("locals 500 x 4096", 0, 500, 4_096, Vec::new(), false),
        ("locals 200 x 50000", 0, 200, 50_000, Vec::new(), false),
        ("1024 locals used in a loop", 0, 10, 1_024, locals_in_a_loop(1_024, 4), false),
        ("nested blocks 1 x 10000", 0, 1, 0, nested_blocks(10_000), false),
        ("nested blocks 100 x 2000", 0, 100, 0, nested_blocks(2_000), false),
        ("branches 1 x 40000", 0, 1, 0, branches(40_000), false),
        ("br_table 1 x 200000", 0, 1, 0, br_table(200_000), false),
    ] {
        let code = shape(imports, functions, locals, &body, exported);
        report_compile(
            label,
            &code,
            u64::from(imports + functions),
            u64::from(functions) * u64::from(locals),
        );
    }
}

#[test]
#[ignore = "timing; run with --ignored --nocapture and record the figures in ADR-142"]
fn calibrate_fresh_memory_pages() {
    // Touch one byte in every 4 KiB of `pages` wasm pages: the page faults and
    // the zeroing the fuel meter does not see.
    let vm = Vm::new().unwrap();
    for (label, initial, grow) in [("declared 256 pages", 256u32, 0u32), ("grown 1 -> 256", 1, 255)] {
        let code = wat::parse_str(format!(
            r#"(module (memory (export "memory") {initial})
                (func (export "invoke") (param i32) (result i64)
                  (local $p i32)
                  (drop (memory.grow (i32.const {grow})))
                  (loop $l
                    (i32.store8 (local.get $p) (i32.const 1))
                    (local.set $p (i32.add (local.get $p) (i32.const 4096)))
                    (br_if $l (i32.lt_u (local.get $p) (i32.const 16777216))))
                  (i64.const 0)))"#
        ))
        .unwrap();
        let empty = wat::parse_str(format!(
            r#"(module (memory (export "memory") {initial})
                (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#
        ))
        .unwrap();
        let mut best = (f64::MAX, f64::MAX);
        let mut used = (0, 0);
        for _ in 0..RUNS {
            for (slot, module) in [(0, &code), (1, &empty)] {
                let started = Instant::now();
                let outcome = vm
                    .execute(module, [1; 32], &[], 400_000_000, MemoryState::at_height(1))
                    .outcome
                    .expect("runs");
                let elapsed = started.elapsed().as_secs_f64();
                if slot == 0 {
                    best.0 = best.0.min(elapsed);
                    used.0 = outcome.gas_used;
                } else {
                    best.1 = best.1.min(elapsed);
                    used.1 = outcome.gas_used;
                }
            }
        }
        let pages = 256.0;
        eprintln!(
            "ADV memory {label:<20} touch {:>9.1} us gas {:>9}  empty {:>9.1} us gas {:>9}  \
             {:>8.2} us/page over empty, {:>7.2} ns/gas",
            best.0 * 1e6,
            used.0,
            best.1 * 1e6,
            used.1,
            (best.0 - best.1) * 1e6 / pages,
            best.0 * 1e9 / used.0.max(1) as f64,
        );
    }
}
