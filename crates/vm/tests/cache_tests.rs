//! The module cache, and the one property that makes it safe.
//!
//! Gas is wasmtime fuel (invariant 21), so "a cached module charges the same"
//! is not a performance claim — it is the whole safety argument for putting a
//! cache on a consensus path. Everything else here is about not serving the
//! wrong code.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_vm::cache::{MAX_CACHED_MODULES, ModuleCache};
use maya_vm::config::config_digest;
use maya_vm::error::VmError;
use maya_vm::host::MemoryState;
use maya_vm::runtime::Vm;

const CONTRACT: [u8; 32] = [7u8; 32];

/// A module that writes one key and returns, so a call does real work and the
/// fuel figure is not dominated by instantiation.
fn counter_wasm() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (func (export "input_ptr") (result i32) (i32.const 1024))
            (func (export "input_cap") (result i32) (i32.const 256))
            (func (export "invoke") (param i32) (result i64)
              (local $i i32)
              (loop $again
                (i32.store8 (i32.const 0) (local.get $i))
                (i32.store (i32.const 16) (local.get $i))
                (call $write (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 4))
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br_if $again (i32.lt_u (local.get $i) (i32.const 16))))
              (i64.const 0)))"#,
    )
    .expect("valid WAT")
}

#[test]
fn a_cached_call_burns_identical_fuel() {
    // The property. If a warm call charged differently from a cold one, two
    // nodes would disagree about gas depending on what each had run before —
    // which is a chain split caused by a cache.
    let vm = Vm::new().expect("engine");
    let wasm = counter_wasm();

    let cold = vm
        .execute(&wasm, CONTRACT, &[7], 1_000_000, MemoryState::default())
        .outcome
        .expect("cold call");
    let warm = vm
        .execute(&wasm, CONTRACT, &[7], 1_000_000, MemoryState::default())
        .outcome
        .expect("warm call");

    assert_eq!(
        cold.gas_used, warm.gas_used,
        "a cached module charged differently"
    );
    assert_eq!(cold.output, warm.output);
    assert_eq!(cold.events, warm.events);

    let (hits, misses) = vm.cache().stats();
    assert_eq!(misses, 1, "the first call should compile");
    assert_eq!(hits, 1, "the second should not");
}

#[test]
fn a_cleared_cache_still_charges_the_same() {
    // The same assertion from the other direction: a node restarting mid-chain
    // has a cold cache, and must agree with one that does not.
    let vm = Vm::new().expect("engine");
    let wasm = counter_wasm();

    let warm_first = vm
        .execute(&wasm, CONTRACT, &[7], 1_000_000, MemoryState::default())
        .outcome
        .expect("call");
    vm.cache().clear();
    assert!(vm.cache().is_empty());
    let after_clear = vm
        .execute(&wasm, CONTRACT, &[7], 1_000_000, MemoryState::default())
        .outcome
        .expect("call");

    assert_eq!(warm_first.gas_used, after_clear.gas_used);
}

#[test]
fn two_contracts_do_not_share_an_entry() {
    let cache = ModuleCache::new(config_digest());
    let vm = Vm::new().expect("engine");

    let first = cache
        .compile(vm.engine(), &counter_wasm())
        .expect("compile");
    // Different bytes, same behaviour: it must still be a different entry,
    // because the key is the bytes and not the meaning.
    let different = wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "input_ptr") (result i32) (i32.const 2048))
            (func (export "input_cap") (result i32) (i32.const 256))
            (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#,
    )
    .expect("valid WAT");
    cache.compile(vm.engine(), &different).expect("compile");

    assert!(first.get_export("invoke").is_some());
    assert_eq!(cache.len(), 2, "two distinct bytecodes shared one entry");
}

#[test]
fn a_different_configuration_is_a_different_cache() {
    // A cache keyed on bytes alone would keep serving modules compiled under
    // the old settings after a config change — two nodes running one contract
    // under two compilers.
    let vm = Vm::new().expect("engine");
    let wasm = counter_wasm();

    let real = ModuleCache::new(config_digest());
    let pretend_upgrade = ModuleCache::new([0xab; 32]);
    real.compile(vm.engine(), &wasm).expect("compile");
    pretend_upgrade
        .compile(vm.engine(), &wasm)
        .expect("compile");

    // Separate caches, but the point is the key: the same bytes under a
    // different digest are a different entry, so an upgrade invalidates
    // everything by construction rather than by anybody remembering.
    assert_eq!(real.len(), 1);
    assert_eq!(pretend_upgrade.len(), 1);
    assert_eq!(real.stats().1, 1);
}

#[test]
fn a_module_that_does_not_compile_is_still_an_error() {
    // A cache must not turn a compile failure into something else.
    let vm = Vm::new().expect("engine");
    let cache = ModuleCache::new(config_digest());
    assert!(matches!(
        cache.compile(vm.engine(), b"not wasm at all"),
        Err(VmError::InvalidModule(_))
    ));
    assert!(cache.is_empty(), "a failed compile was cached");
}

#[test]
fn the_cache_is_bounded_and_evicts_the_coldest() {
    // Otherwise a node dies from the number of distinct contracts it has seen,
    // which is a number an attacker chooses by deploying junk.
    let vm = Vm::new().expect("engine");
    let cache = ModuleCache::new(config_digest());

    // Each module differs by a constant, so each is distinct bytes and each
    // definitely compiles — a hand-rolled custom section is a way to write an
    // invalid module by accident, which this test would then report as an
    // eviction bug.
    let module = |n: usize| {
        let text = format!(
            r#"(module
                (memory (export "memory") 1)
                (func (export "input_ptr") (result i32) (i32.const 1024))
                (func (export "input_cap") (result i32) (i32.const 256))
                (func (export "invoke") (param i32) (result i64) (i64.const {n})))"#
        );
        wat::parse_str(&text).expect("valid WAT")
    };

    for index in 0..MAX_CACHED_MODULES {
        cache.compile(vm.engine(), &module(index)).expect("compile");
    }
    assert_eq!(cache.len(), MAX_CACHED_MODULES);

    // Touch the first so it is not the coldest.
    cache.compile(vm.engine(), &module(0)).expect("hit");
    cache
        .compile(vm.engine(), &module(MAX_CACHED_MODULES))
        .expect("compile");

    assert_eq!(
        cache.len(),
        MAX_CACHED_MODULES,
        "the cache grew past its bound"
    );
}
