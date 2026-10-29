//! `oracle_read_u256` (ADR-066): a price past u64 reaches the contract exactly,
//! where the 8-byte `oracle_read` can only clamp it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_amount::Amount;
use maya_vm::host::{Address, ContractId, Event, HostState, MemoryState, OracleValue};
use maya_vm::runtime::Vm;

const CONTRACT: ContractId = [7u8; 32];
const GAS: u64 = 10_000_000;
/// 2^192: limb 3 = 1, far past u64::MAX.
const PRICE: Amount = Amount([0, 0, 0, 1]);

struct Host {
    inner: MemoryState,
}

impl HostState for Host {
    fn balance_of(&self, address: &Address) -> Amount {
        self.inner.balance_of(address)
    }
    fn block_height(&self) -> u64 {
        self.inner.block_height()
    }
    fn storage_get(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        self.inner.storage_get(contract, key)
    }
    fn storage_set(&mut self, contract: &ContractId, key: Vec<u8>, value: Vec<u8>) {
        self.inner.storage_set(contract, key, value);
    }
    fn emit(&mut self, event: Event) {
        self.inner.emit(event);
    }
    fn oracle_feed(&self, feed_id: &[u8; 32]) -> Option<OracleValue> {
        (feed_id == &[0u8; 32]).then_some(OracleValue {
            value: PRICE,
            updated_height: 0,
            round: 1,
            observations: 3,
        })
    }
}

/// Reads feed 0 with `import`, returns `len` bytes at 64.
fn contract(import: &str, len: u32) -> Vec<u8> {
    let packed = (64u64 << 32) | u64::from(len);
    wat::parse_str(format!(
        r#"(module
            (import "env" "{import}" (func $read (param i32 i64 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (drop (call $read (i32.const 0) (i64.const 100) (i32.const 64)))
              (i64.const {packed})))"#
    ))
    .expect("valid WAT")
}

fn run(import: &str, len: u32) -> Vec<u8> {
    let host = Host {
        inner: MemoryState::default(),
    };
    let exec = Vm::new()
        .unwrap()
        .execute(&contract(import, len), CONTRACT, &[], GAS, host);
    exec.outcome.expect("ran").output
}

#[test]
fn the_wide_read_returns_the_exact_price() {
    assert_eq!(run("oracle_read_u256", 32), PRICE.to_le_bytes());
}

/// Calls `import` and returns its status code, which it stores at 128.
fn status_of(import: &str) -> i32 {
    let packed = (128u64 << 32) | 4;
    let wasm = wat::parse_str(format!(
        r#"(module
            (import "env" "{import}" (func $read (param i32 i64 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (i32.store (i32.const 128)
                (call $read (i32.const 0) (i64.const 100) (i32.const 64)))
              (i64.const {packed})))"#
    ))
    .expect("valid WAT");
    let host = Host {
        inner: MemoryState::default(),
    };
    let output = Vm::new()
        .unwrap()
        .execute(&wasm, CONTRACT, &[], GAS, host)
        .outcome
        .expect("ran")
        .output;
    i32::from_le_bytes(output.try_into().unwrap())
}

#[test]
fn the_legacy_read_refuses_a_price_past_u64_rather_than_clamping() {
    // A clamped u64::MAX would read as a real price; -3 says "use the wide call".
    assert_eq!(status_of("oracle_read"), -3);
    assert_eq!(
        run("oracle_read", 8),
        [0u8; 8],
        "nothing is written on failure"
    );
    assert_eq!(status_of("oracle_read_u256"), 32);
}
