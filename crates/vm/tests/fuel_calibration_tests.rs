//! How much wall time one unit of gas buys — the number that prices every
//! host function doing native work.
//!
//! Gas is wasmtime fuel: roughly one unit per wasm instruction. A host function
//! that does native work the fuel meter cannot see must charge fuel equal to
//! the guest instructions that would take as long, or a contract gets that
//! work for free. `ZKML_VERIFY_BASE_FUEL` is priced from this measurement; see
//! `crates/vm/src/zkml.rs`.
//!
//! Timing assertions are flaky, so this asserts only that the loop spent the
//! fuel it should have, and prints the rate. Run it with `--nocapture`.

use std::time::Instant;

use maya_vm::{MemoryState, Vm};

const ITERATIONS: u32 = 20_000_000;

#[test]
fn measure_fuel_throughput() {
    let wat = format!(
        r#"(module
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
                (local $i i32)
                (block $done
                    (loop $again
                        (local.set $i (i32.add (local.get $i) (i32.const 1)))
                        (br_if $again (i32.lt_u (local.get $i) (i32.const {ITERATIONS})))))
                (i64.const 0)))"#
    );
    let wasm = wat::parse_str(&wat).expect("wat");
    let vm = Vm::new().expect("vm");

    let mut best = f64::MAX;
    let mut used = 0;
    for _ in 0..5 {
        let started = Instant::now();
        let execution = vm.execute(&wasm, [0; 32], &[], u64::MAX / 2, MemoryState::at_height(1));
        let elapsed = started.elapsed().as_secs_f64();
        used = execution.outcome.expect("runs").gas_used;
        best = best.min(elapsed);
    }

    assert!(used > u64::from(ITERATIONS), "the loop spent {used} fuel");
    let per_ms = used as f64 / (best * 1000.0);
    eprintln!(
        "fuel used {used} in {:.1} ms: {:.0} fuel per ms",
        best * 1000.0,
        per_ms
    );
}
