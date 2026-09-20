//! What the module cache actually saves.
//!
//! ## The number this reports is measured, not targeted
//!
//! The brief that produced this asked for a 10x improvement of "JIT over
//! interpreted". There is no interpreted mode: `config.rs` sets
//! `Strategy::Cranelift`, so every module has always been compiled to native
//! code. What was missing is that the compile happened again on every call.
//!
//! So this measures the real thing — a call that recompiles against one that
//! does not — and reports whatever the ratio is. Producing a 10x by adding an
//! interpreter for it to beat would be a benchmark built to be won.
//!
//! Two contract shapes, because the answer depends entirely on the shape:
//!
//! - **trivial** — returns immediately. Almost all of the time is compilation,
//!   so the cache should look enormous here, and that number would flatter it.
//! - **busy** — a loop doing real work. Compilation is a smaller share, and this
//!   is the honest figure for a contract anybody would deploy.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use criterion::{Criterion, criterion_group, criterion_main};
use maya_vm::host::MemoryState;
use maya_vm::runtime::Vm;
use std::hint::black_box;

const CONTRACT: [u8; 32] = [7u8; 32];
const GAS: u64 = 50_000_000;

fn wasm(text: &str) -> Vec<u8> {
    wat::parse_str(text).expect("valid WAT")
}

/// Returns immediately. Nearly pure compilation cost.
fn trivial() -> Vec<u8> {
    wasm(
        r#"(module
            (memory (export "memory") 1)
            (func (export "input_ptr") (result i32) (i32.const 1024))
            (func (export "input_cap") (result i32) (i32.const 256))
            (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#,
    )
}

/// A loop with arithmetic and memory traffic: work a real contract might do.
fn busy() -> Vec<u8> {
    wasm(
        r#"(module
            (memory (export "memory") 1)
            (func (export "input_ptr") (result i32) (i32.const 1024))
            (func (export "input_cap") (result i32) (i32.const 256))
            (func (export "invoke") (param i32) (result i64)
              (local $i i32) (local $acc i64)
              (loop $again
                (i32.store (i32.const 0) (local.get $i))
                (local.set $acc
                  (i64.add (local.get $acc)
                    (i64.extend_i32_u (i32.load (i32.const 0)))))
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br_if $again (i32.lt_u (local.get $i) (i32.const 20000))))
              ;; The accumulator is stored rather than returned: `invoke`'s i64
              ;; is `(out_ptr << 32) | out_len`, so returning a sum here asks the
              ;; host to read 200 MB out of a one-page memory.
              (i64.store (i32.const 8) (local.get $acc))
              (i64.const 0)))"#,
    )
}

/// One call, warm or cold.
fn call(vm: &Vm, wasm: &[u8]) {
    let execution = vm.execute(wasm, CONTRACT, &[], GAS, MemoryState::default());
    black_box(execution.outcome.expect("call succeeds"));
}

fn benchmark(criterion: &mut Criterion) {
    for (name, bytes) in [("trivial", trivial()), ("busy", busy())] {
        let mut group = criterion.benchmark_group(name);

        // Cold: the cache is cleared before every call, so each one pays for a
        // full Cranelift compile — which is what every call paid before.
        let vm = Vm::new().expect("engine");
        group.bench_function("recompiled", |bencher| {
            bencher.iter(|| {
                vm.cache().clear();
                call(&vm, &bytes);
            });
        });

        // Warm: the compile happens once, outside the loop.
        let vm = Vm::new().expect("engine");
        call(&vm, &bytes);
        group.bench_function("cached", |bencher| bencher.iter(|| call(&vm, &bytes)));

        group.finish();
    }
}

criterion_group!(benches, benchmark);
criterion_main!(benches);
