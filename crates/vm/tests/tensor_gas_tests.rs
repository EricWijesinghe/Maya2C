//! Tensor multiplication inside the sandbox: what it costs, and where it stops.
//!
//! # Why there is no tensor host function
//!
//! An int8 matmul written as wasm is metered by the same fuel counter as every
//! other instruction, so it is priced exactly — instruction for instruction —
//! with no constant anybody had to guess. A native `host_matmul_i8` would be
//! faster and would need a hand-set price per multiply-accumulate, and that
//! price would be consensus-critical and wrong on some machine. The only native
//! work the VM prices by hand is proof verification (`crates/vm/src/zkml.rs`), because
//! there the alternative is not "slower", it is "impossible".
//!
//! These tests pin what that choice means in numbers: fuel per MAC, that cost
//! grows as `n³`, and that the 16 MiB memory ceiling is where a tensor stops.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_vm::{MemoryState, Vm, VmError};

/// An `n×n` int8 × int8 → int32 matmul, `C = A·B`, over zeroed memory.
///
/// Zeroed inputs are fine for metering: fuel counts instructions executed, not
/// values, and this kernel has no data-dependent branch.
fn matmul(n: u32) -> Vec<u8> {
    let a = 0;
    let b = n * n;
    let c = 2 * n * n;
    let bytes = c + 4 * n * n;
    let pages = bytes.div_ceil(65_536).max(1);
    let wat = format!(
        r#"(module
            (memory (export "memory") {pages})
            (func (export "invoke") (param i32) (result i64)
                (local $i i32) (local $j i32) (local $k i32) (local $acc i32)
                (loop $rows
                    (local.set $j (i32.const 0))
                    (loop $cols
                        (local.set $acc (i32.const 0))
                        (local.set $k (i32.const 0))
                        (loop $inner
                            (local.set $acc (i32.add (local.get $acc)
                                (i32.mul
                                    (i32.load8_s (i32.add (i32.const {a})
                                        (i32.add (i32.mul (local.get $i) (i32.const {n})) (local.get $k))))
                                    (i32.load8_s (i32.add (i32.const {b})
                                        (i32.add (i32.mul (local.get $k) (i32.const {n})) (local.get $j)))))))
                            (local.set $k (i32.add (local.get $k) (i32.const 1)))
                            (br_if $inner (i32.lt_u (local.get $k) (i32.const {n}))))
                        (i32.store (i32.add (i32.const {c})
                            (i32.shl (i32.add (i32.mul (local.get $i) (i32.const {n})) (local.get $j)) (i32.const 2)))
                            (local.get $acc))
                        (local.set $j (i32.add (local.get $j) (i32.const 1)))
                        (br_if $cols (i32.lt_u (local.get $j) (i32.const {n}))))
                    (local.set $i (i32.add (local.get $i) (i32.const 1)))
                    (br_if $rows (i32.lt_u (local.get $i) (i32.const {n}))))
                (i64.const 0)))"#
    );
    wat::parse_str(&wat).expect("wat")
}

fn gas_for(n: u32) -> u64 {
    let vm = Vm::new().expect("vm");
    vm.execute(
        &matmul(n),
        [0; 32],
        &[],
        u64::MAX / 2,
        MemoryState::at_height(1),
    )
    .outcome
    .expect("matmul runs")
    .gas_used
}

#[test]
fn a_multiply_accumulate_costs_a_stable_amount_of_fuel() {
    // The inner loop is two address computations, two loads, a multiply, an
    // add, and the loop test — about twenty instructions. The bound is loose on
    // purpose: it pins the order of magnitude a contract author can plan on,
    // not wasmtime's exact instruction count.
    let n = 64u64;
    let per_mac = gas_for(64) as f64 / (n * n * n) as f64;
    eprintln!("int8 matmul: {per_mac:.1} fuel per multiply-accumulate");
    assert!((10.0..=40.0).contains(&per_mac), "{per_mac} fuel per MAC");
}

#[test]
fn matmul_cost_grows_as_the_cube_of_the_side() {
    // Doubling the side is eight times the MACs. Allow a little for the O(n²)
    // outer-loop and store overhead, which shrinks relative to n³ as n grows.
    let small = gas_for(32) as f64;
    let large = gas_for(64) as f64;
    let ratio = large / small;
    assert!(
        (7.5..=8.5).contains(&ratio),
        "gas ratio {ratio} for doubling n"
    );
}

#[test]
fn verifying_a_proof_costs_what_a_large_matmul_costs() {
    // The comparison that decides when zkML is worth using at all. Verification
    // is priced at ZKML_VERIFY_BASE_FUEL; re-running a model in the guest costs
    // its MAC count times the per-MAC fuel above. Below the crossover, running
    // the model is cheaper than checking a proof of it — which is true of every
    // model this repository's circuit can currently prove. See docs/zkml.md.
    let n = 64u64;
    let per_mac = gas_for(64) as f64 / (n * n * n) as f64;
    let crossover = maya_vm::zkml::ZKML_VERIFY_BASE_FUEL as f64 / per_mac;
    eprintln!("verification = re-running a model of ~{crossover:.0} MACs");
    // The fixture classifier is 4·8 + 8·3 = 56 MACs.
    assert!(crossover > 1_000_000.0);
}

#[test]
fn a_tensor_larger_than_sixteen_mebibytes_cannot_be_allocated() {
    // 4097 × 4097 int8 is just over 16 MiB. The limiter refuses the growth and
    // the call fails with the limit named, rather than the guest seeing -1 from
    // memory.grow and carrying on with a tensor it does not have.
    let wat = r#"(module
        (memory (export "memory") 1)
        (func (export "invoke") (param i32) (result i64)
            (drop (memory.grow (i32.const 256)))
            (i64.const 0)))"#;
    let vm = Vm::new().expect("vm");
    let outcome = vm
        .execute(
            &wat::parse_str(wat).expect("wat"),
            [0; 32],
            &[],
            10_000_000,
            MemoryState::at_height(1),
        )
        .outcome;
    assert!(
        matches!(outcome, Err(VmError::MemoryLimit { limit: 256, .. })),
        "{outcome:?}"
    );
}

#[test]
fn a_tensor_that_fits_is_allocated() {
    // The other side of the boundary: 255 more pages is exactly 16 MiB.
    let wat = r#"(module
        (memory (export "memory") 1)
        (func (export "invoke") (param i32) (result i64)
            (i64.extend_i32_s (memory.grow (i32.const 255)))))"#;
    let vm = Vm::new().expect("vm");
    let outcome = vm
        .execute(
            &wat::parse_str(wat).expect("wat"),
            [0; 32],
            &[],
            10_000_000,
            MemoryState::at_height(1),
        )
        .outcome;
    // `invoke`'s i64 is read as (ptr << 32) | len; the previous size, 1 page,
    // reads as an empty output at offset 0.
    assert!(outcome.is_ok(), "{outcome:?}");
}
