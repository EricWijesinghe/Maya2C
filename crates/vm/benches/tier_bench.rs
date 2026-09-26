//! Speed of each execution tier on the same calls (Master Prompt 5 §2).
//!
//! Gas is identical in every tier (`tests/tier_differential_tests.rs`); this
//! measures only wall time. Plain harness: median of repeated runs, printed
//! as a table. `cargo bench -p maya-vm --bench tier_bench`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::time::Instant;

use maya_vm::host::MemoryState;
use maya_vm::{Tier, Vm};

const LOOP: &str = r#"(module (memory (export "memory") 1)
  (func (export "invoke") (param i32) (result i64)
    (local $i i64) (local $acc i64)
    (loop $l
      (local.set $acc (i64.add (local.get $acc) (i64.mul (local.get $i) (local.get $i))))
      (local.set $i (i64.add (local.get $i) (i64.const 1)))
      (br_if $l (i64.lt_u (local.get $i) (i64.const 200000))))
    (i64.const 0)))"#;

fn median_us(mut f: impl FnMut(), runs: usize) -> f64 {
    let mut samples: Vec<f64> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1e6
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    samples[runs / 2]
}

fn main() {
    let loop_wasm = wat::parse_str(LOOP).unwrap();
    let swap = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target-contracts/wasm32-unknown-unknown/release/token_swap.wasm"),
    )
    .ok();
    println!("maya-vm tier bench (median wall time per call; gas identical across tiers)");
    println!(
        "{:<12} {:<26} {:>14} {:>12}",
        "tier", "workload", "median us", "gas"
    );
    let mut rows = Vec::new();
    for tier in Tier::ALL {
        let vm = Vm::with_tier(tier).unwrap();
        // Cold compile: a fresh VM each run, so every call misses the cache.
        let cold = median_us(
            || {
                let fresh = Vm::with_tier(tier).unwrap();
                let _ = fresh.execute(
                    &loop_wasm,
                    [1; 32],
                    &[],
                    100_000_000,
                    MemoryState::at_height(1),
                );
            },
            11,
        );
        let gas = vm
            .execute(
                &loop_wasm,
                [1; 32],
                &[],
                100_000_000,
                MemoryState::at_height(1),
            )
            .outcome
            .unwrap()
            .gas_used;
        let warm = median_us(
            || {
                let _ = vm.execute(
                    &loop_wasm,
                    [1; 32],
                    &[],
                    100_000_000,
                    MemoryState::at_height(1),
                );
            },
            31,
        );
        println!(
            "{:<12} {:<26} {:>14.1} {:>12}",
            tier.name(),
            "200k-iteration loop, cold",
            cold,
            gas
        );
        println!(
            "{:<12} {:<26} {:>14.1} {:>12}",
            tier.name(),
            "200k-iteration loop, cached",
            warm,
            gas
        );
        rows.push(warm);
        if let Some(code) = &swap {
            let mut init = vec![0u8];
            init.extend_from_slice(&1_000_000u64.to_le_bytes());
            init.extend_from_slice(&2_000_000u64.to_le_bytes());
            let state = vm
                .execute(code, [1; 32], &init, 10_000_000, MemoryState::at_height(1))
                .state;
            let mut call = vec![1u8];
            call.extend_from_slice(&1_000u64.to_le_bytes());
            let gas = vm
                .execute(code, [1; 32], &call, 10_000_000, state.clone())
                .outcome
                .unwrap()
                .gas_used;
            let t = median_us(
                || {
                    let _ = vm.execute(code, [1; 32], &call, 10_000_000, state.clone());
                },
                101,
            );
            println!(
                "{:<12} {:<26} {:>14.1} {:>12}",
                tier.name(),
                "token_swap swap, cached",
                t,
                gas
            );
        }
    }
    if let [cranelift, pulley] = rows[..] {
        println!(
            "pulley / cranelift on the cached loop: {:.1}x slower",
            pulley / cranelift
        );
    }
}
