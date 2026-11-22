//! A fault of this node's engine is told apart from a failed call (#647
//! review): the operating system refusing memory becomes
//! [`VmError::LocalFault`], which the node refuses rather than records.
//!
//! The refusal is injected through the store's resource limiter, the hook
//! wasmtime itself calls for every linear memory, in place of an allocation
//! failing: the error has the type an `mmap` or `VirtualAlloc` refusal has.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::host::{Address, Event};

/// A host whose "operating system" refuses memory past `allowed` bytes with
/// the error type wasmtime's mappings report, unrecorded, as an allocation
/// failure would be.
struct RefusingHost {
    allowed: usize,
}

impl HostState for RefusingHost {
    fn balance_of(&self, _address: &Address) -> maya_amount::Amount {
        maya_amount::Amount::ZERO
    }
    fn block_height(&self) -> u64 {
        1
    }
    fn storage_get(&self, _contract: &ContractId, _key: &[u8]) -> Option<Vec<u8>> {
        None
    }
    fn storage_set(&mut self, _contract: &ContractId, _key: Vec<u8>, _value: Vec<u8>) {}
    fn emit(&mut self, _event: Event) {}
}

impl wasmtime::ResourceLimiter for RefusingHost {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.allowed {
            return Err(std::io::Error::other("commit refused").into());
        }
        Ok(true)
    }
    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(false)
    }
}

const GROWS: &str = r#"(module (memory (export "memory") 1)
    (func (export "invoke") (param i32) (result i64)
      (drop (memory.grow (i32.const 1))) (i64.const 0)))"#;

fn run_refusing(wat: &str, allowed: usize) -> core::result::Result<Outcome, VmError> {
    let vm = Vm::new().unwrap();
    let wasm = wat::parse_str(wat).unwrap();
    let run = vm.run_prepared(
        Source::Wasm(&wasm),
        [0; 32],
        &[],
        10_000_000,
        RefusingHost { allowed },
        |_| Ok(()),
        |store| store.limiter(|ctx| &mut ctx.state),
    );
    execution(run).outcome
}

#[test]
fn memory_refused_at_instantiation_is_a_local_fault() {
    // Red before: classified as `Trap`, so a node short of memory recorded
    // the call as failed while the others applied it.
    let outcome = run_refusing(GROWS, 0);
    assert!(
        matches!(outcome, Err(VmError::LocalFault(_))),
        "got {outcome:?}"
    );
}

#[test]
fn memory_refused_while_running_is_a_local_fault() {
    let outcome = run_refusing(GROWS, PAGE_SIZE);
    assert!(
        matches!(outcome, Err(VmError::LocalFault(_))),
        "got {outcome:?}"
    );
}

#[test]
fn the_same_contract_runs_when_memory_is_there() {
    // The injection is the only difference: with memory allowed, it applies.
    let outcome = run_refusing(GROWS, 4 * PAGE_SIZE);
    assert!(outcome.is_ok(), "got {outcome:?}");
}

#[test]
fn the_limiter_passes_up_only_a_refusal_by_the_system() {
    // `memory_grow_failed` is wasmtime's report of a growth `memory_growing`
    // allowed: past the module's own maximum (alike everywhere, wasm's -1)
    // or refused by the operating system (this node's).
    let mut context = CallContext {
        state: crate::MemoryState::at_height(1),
        contract: [0; 32],
        events: Vec::new(),
        failure: None,
        max_pages: MAX_MEMORY_PAGES,
        peak_pages: 0,
        overflowed: false,
    };
    let limiter: &mut dyn wasmtime::ResourceLimiter = &mut context;
    let maximum = wasmtime::Error::msg("Memory maximum size exceeded");
    assert!(limiter.memory_grow_failed(maximum).is_ok());
    let refused: wasmtime::Error = std::io::Error::other("commit refused").into();
    assert!(limiter.memory_grow_failed(refused).is_err());
}

#[test]
fn deterministic_failures_stay_failures_of_the_call() {
    // A trap, a module's own memory maximum, and a table the limiter
    // refuses (a plain wasmtime error, not a `Trap`, alike on every node)
    // must never read as local: that would stop block building on every node
    // while the transaction is pooled.
    let vm = Vm::new().unwrap();
    let run = |wat: &str| {
        let wasm = wat::parse_str(wat).unwrap();
        vm.execute(
            &wasm,
            [0; 32],
            &[],
            10_000_000,
            crate::MemoryState::at_height(1),
        )
        .outcome
    };
    let trap = run(r#"(module (memory (export "memory") 1)
        (func (export "invoke") (param i32) (result i64) unreachable))"#);
    assert!(matches!(trap, Err(VmError::Trap(_))), "got {trap:?}");
    let past_max = run(r#"(module (memory (export "memory") 1 1)
        (func (export "invoke") (param i32) (result i64)
          (if (i32.ne (memory.grow (i32.const 1)) (i32.const -1)) (then unreachable))
          (i64.const 0)))"#);
    assert!(
        past_max.is_ok(),
        "growth past the module maximum is -1: {past_max:?}"
    );
    let table = run(r#"(module (memory (export "memory") 1) (table 1 funcref)
        (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#);
    assert!(matches!(table, Err(VmError::Trap(_))), "got {table:?}");
}

#[test]
fn a_compile_failure_is_local_only_when_the_system_refused() {
    let io: wasmtime::Error = std::io::Error::other("code memory refused").into();
    assert!(matches!(VmError::from_compile(&io), VmError::LocalFault(_)));
    let invalid = wasmtime::Error::msg("invalid module");
    assert!(matches!(
        VmError::from_compile(&invalid),
        VmError::InvalidModule(_)
    ));
    // And bytes that do not validate are the module's, through the engine.
    let vm = Vm::new().unwrap();
    assert!(matches!(
        vm.validate(b"\0asm\x01\0\0\0\x7f"),
        Err(VmError::InvalidModule(_))
    ));
}
