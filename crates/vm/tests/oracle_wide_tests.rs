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

#[test]
fn the_legacy_read_clamps_a_price_past_u64() {
    assert_eq!(run("oracle_read", 8), u64::MAX.to_le_bytes());
}
