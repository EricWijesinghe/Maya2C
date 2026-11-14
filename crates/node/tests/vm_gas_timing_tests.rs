//! Worst-case block timing under the contract gas caps (ADR-142).
//!
//! Each test fills one block, up to `vm.max_block_gas`, with one kind of the
//! most expensive priced contract work, and times `apply_block` on it, which
//! is what a validator does with a block it receives: signatures, execution,
//! the state root, and the commit.
//!
//! The figures printed (`TIMING ...`) are the measurement ADR-142's
//! derivation cites. The assertion is the requirement, a block inside the 1 s
//! block time; the derivation's target is a quarter of it. Run with
//! `cargo test -p custom-l1-node --test vm_gas_timing_tests -- --nocapture
//! --test-threads 1`. Built in the dev profile, which leaves the node and the
//! VM unoptimised, so a release build is faster than what is printed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::time::Instant;

use custom_l1_node::core::payload::{ContractCall, ContractDeploy, derive_contract_id};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::{Account, BlockContext, StateDB};
use maya_governance::params::ParameterKey;
use maya_vm::{MemoryState, Vm};
use tempfile::TempDir;

mod common;

/// The block time the cap must stay inside.
const BLOCK_TIME_MS: f64 = 1_000.0;
/// Calls per block in the call scenarios.
const CALLS: u64 = 10;

fn block_cap() -> u64 {
    ParameterKey::VmMaxBlockGas
        .bounds()
        .default
        .saturating_to_u64()
}

fn wasm(text: &str) -> Vec<u8> {
    wat::parse_str(text).expect("valid WAT")
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key, &common::test_chain()).expect("sign");
    tx
}

struct Fixture {
    db: StateDB,
    _dir: TempDir,
    keys: Vec<HybridSigningKey>,
}

fn fixture(senders: usize) -> Fixture {
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    common::bind(&db);
    let keys: Vec<_> = (0..senders)
        .map(|_| generate_signing_key().unwrap())
        .collect();
    for key in &keys {
        db.put_account(
            &key.address(),
            &Account {
                balance: primitive_types::U256::from(1_000_000u64),
                nonce: 0,
            },
        )
        .unwrap();
    }
    Fixture {
        db,
        _dir: dir,
        keys,
    }
}

/// Times `apply_block` and prints the rate.
fn time_block(label: &str, db: &StateDB, block: &Block, height: u64, gas: u64) -> f64 {
    let started = Instant::now();
    db.apply_block(block, BlockContext::at_height(height))
        .unwrap_or_else(|e| panic!("{label}: the block must apply: {e}"));
    let ms = started.elapsed().as_secs_f64() * 1e3;
    eprintln!(
        "TIMING {label:<34} {ms:>9.1} ms  {:>3} txs  gas {gas:>11}  {:>6.3} ns/gas",
        block.transactions.len(),
        ms * 1e6 / gas.max(1) as f64
    );
    ms
}

/// A contract that runs `body` `iterations` times; `$i` is the counter.
fn looping(imports: &str, data: &str, body: &str, iterations: u64) -> Vec<u8> {
    wasm(&format!(
        r#"(module {imports}
            (memory (export "memory") 2)
            {data}
            (func (export "invoke") (param i32) (result i64)
              (local $i i32)
              (block $done (loop $again
                (br_if $done (i32.ge_u (local.get $i) (i32.const {iterations})))
                {body}
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br $again)))
              (i64.const 0)))"#
    ))
}

/// The most iterations of `make(n)` a cold call can afford under `limit`,
/// measured on the VM itself: fuel is the same whatever the host.
fn iterations_within(
    limit: u64,
    make: impl Fn(u64) -> Vec<u8>,
    state: impl Fn() -> MemoryState,
) -> u64 {
    let vm = Vm::new().unwrap();
    let gas = |n| {
        vm.execute(&make(n), [0; 32], &[], u64::MAX / 4, state())
            .outcome
            .expect("preflight runs")
            .gas_used
    };
    let (one, two) = (gas(1), gas(2));
    let per = two - one;
    // 1% spare for the module growing a byte as the constant widens.
    (limit - one) * 99 / 100 / per
}

/// Deploys `code` once from `key` at nonce 0, in its own block at height 1.
fn deploy_one(db: &StateDB, key: &HybridSigningKey, code: &[u8]) -> [u8; 32] {
    let tx = signed(
        TxKind::DeployContract(ContractDeploy {
            code: code.to_vec(),
        }),
        0,
        key,
    );
    db.apply_block(&block_of(vec![tx]), BlockContext::at_height(1))
        .unwrap();
    derive_contract_id(&key.address(), 0, code)
}

/// `CALLS` calls to `contract`, one per sender, each with `limit`.
fn calls(fixture: &Fixture, contract: [u8; 32], limit: u64, nonce: u64) -> Vec<Transaction> {
    fixture
        .keys
        .iter()
        .take(CALLS as usize)
        .map(|key| {
            signed(
                TxKind::CallContract(ContractCall {
                    contract,
                    input: Vec::new(),
                    gas_limit: limit,
                }),
                nonce,
                key,
            )
        })
        .collect()
}

fn assert_inside(label: &str, ms: f64) {
    assert!(
        ms < BLOCK_TIME_MS,
        "{label}: a block of worst-case work took {ms:.1} ms, past the {BLOCK_TIME_MS} ms block time"
    );
}

#[test]
fn a_block_of_host_calls_validates_inside_the_block_time() {
    let fixture = fixture(CALLS as usize);
    let limit = block_cap() / CALLS;
    let import = r#"(import "env" "get_balance_u256" (func $b (param i32 i32) (result i32)))"#;
    let body = "(drop (call $b (i32.const 0) (i32.const 64)))";
    let make = |n| looping(import, "", body, n);
    let n = iterations_within(limit, make, || MemoryState::at_height(2));
    // The deployer is also the first caller; its call is at nonce 1.
    let contract = deploy_one(&fixture.db, &fixture.keys[0], &make(n));
    let mut txs = calls(&fixture, contract, limit, 0);
    txs[0] = calls(&fixture, contract, limit, 1).remove(0);
    let ms = time_block(
        "host calls (get_balance_u256)",
        &fixture.db,
        &block_of(txs),
        2,
        block_cap(),
    );
    assert_inside("host calls", ms);
}

#[test]
fn a_block_of_committed_storage_reads_validates_inside_the_block_time() {
    let fixture = fixture(CALLS as usize);
    let limit = block_cap() / CALLS;
    let import = r#"(import "env" "storage_read" (func $r (param i32 i32 i32 i32) (result i32)))"#;
    // Reads key `i` (4 bytes little-endian), a 1-byte value: the lookup
    // dominates the price, the worst ratio of RocksDB work to gas.
    let body = "(i32.store (i32.const 0) (local.get $i))
                (drop (call $r (i32.const 0) (i32.const 4) (i32.const 64) (i32.const 64)))";
    let make = |n| looping(import, "", body, n);
    let keys = 8_000u32;
    let filled = || {
        let mut state = MemoryState::at_height(2);
        for i in 0..keys {
            state
                .storage
                .insert(([0; 32], i.to_le_bytes().to_vec()), vec![1]);
        }
        state
    };
    let n = iterations_within(limit, make, filled);
    assert!(
        n <= u64::from(keys),
        "every read must hit: {n} reads, {keys} keys"
    );
    let contract = deploy_one(&fixture.db, &fixture.keys[0], &make(n));
    // Committed, so each read is a RocksDB point lookup, not an overlay hit.
    for i in 0..keys {
        let mut key = b"cstate:".to_vec();
        key.extend_from_slice(&contract);
        key.extend_from_slice(&i.to_le_bytes());
        fixture.db.raw_put_for_test(&key, &[1]).unwrap();
    }
    let mut txs = calls(&fixture, contract, limit, 0);
    txs[0] = calls(&fixture, contract, limit, 1).remove(0);
    let ms = time_block(
        "storage reads (RocksDB hits)",
        &fixture.db,
        &block_of(txs),
        2,
        block_cap(),
    );
    assert_inside("storage reads", ms);
}

#[test]
fn a_block_of_storage_writes_validates_inside_the_block_time() {
    let fixture = fixture(CALLS as usize);
    let limit = block_cap() / CALLS;
    let imports = r#"(import "env" "storage_write" (func $w (param i32 i32 i32 i32)))
                     (import "env" "caller" (func $c (param i32) (result i32)))"#;
    // Key: the caller's first 4 bytes, then `i`, so every write in the block
    // is a new 4 KiB record.
    let body = "(drop (call $c (i32.const 0)))
                (i32.store (i32.const 4) (local.get $i))
                (call $w (i32.const 0) (i32.const 8) (i32.const 1024) (i32.const 4096))";
    let make = |n| looping(imports, "", body, n);
    let n = iterations_within(limit, make, || {
        let mut state = MemoryState::at_height(2);
        state.caller = Some([1; 32]);
        state
    });
    let contract = deploy_one(&fixture.db, &fixture.keys[0], &make(n));
    let mut txs = calls(&fixture, contract, limit, 0);
    txs[0] = calls(&fixture, contract, limit, 1).remove(0);
    eprintln!(
        "TIMING   ({} writes of 4 KiB: {} KiB of new contract state)",
        n * CALLS,
        n * CALLS * 4
    );
    let ms = time_block(
        "storage writes (4 KiB)",
        &fixture.db,
        &block_of(txs),
        2,
        block_cap(),
    );
    assert_inside("storage writes", ms);
}

/// A distinct module of about `target` bytes: many functions of straight-line
/// i64 arithmetic, the shape that measured slowest to compile per byte.
fn compile_heavy(target: usize, salt: u64) -> Vec<u8> {
    let mut body = String::new();
    for i in 0..200u64 {
        body.push_str(&format!(
            "(local.set $a (i64.add (i64.mul (local.get $a) (i64.const {})) (local.get $b)))\
             (local.set $b (i64.xor (local.get $b) (i64.rotl (local.get $a) (i64.const 7))))",
            i ^ salt
        ));
    }
    let mut funcs = String::new();
    for n in 0.. {
        funcs.push_str(&format!(
            "(func $f{n} (param $a i64) (param $b i64) (result i64) {body} (local.get $a))"
        ));
        let code = wasm(&format!(
            r#"(module (memory (export "memory") 1) {funcs}
                (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#
        ));
        if code.len() >= target {
            return code;
        }
    }
    unreachable!()
}

#[test]
fn blocks_of_cold_compiles_validate_inside_the_block_time() {
    let fixture = fixture(CALLS as usize);
    // As large as ten cold calls fit in one block.
    let per_call = block_cap() / CALLS;
    let target =
        usize::try_from((per_call - maya_vm::gas::CALL_BASE_FUEL) / maya_vm::gas::CODE_BYTE_FUEL)
            .unwrap()
            * 97
            / 100;
    let modules: Vec<Vec<u8>> = (0..CALLS)
        .map(|salt| compile_heavy(target, salt + 1))
        .collect();
    let deploy_gas: u64 = modules
        .iter()
        .map(|m| maya_vm::gas::deploy_fuel(m.len()))
        .sum();
    assert!(deploy_gas <= block_cap());

    // Ten deployments: each one compiles to validate.
    let deploys: Vec<Transaction> = fixture
        .keys
        .iter()
        .zip(&modules)
        .map(|(key, code)| {
            signed(
                TxKind::DeployContract(ContractDeploy { code: code.clone() }),
                0,
                key,
            )
        })
        .collect();
    let ms = time_block(
        "deploys (compile)",
        &fixture.db,
        &block_of(deploys),
        1,
        deploy_gas,
    );
    assert_inside("deploys", ms);

    // Then one cold call to each: ten compiles of distinct modules.
    let txs: Vec<Transaction> = fixture
        .keys
        .iter()
        .zip(&modules)
        .map(|(key, code)| {
            let contract = derive_contract_id(&key.address(), 0, code);
            signed(
                TxKind::CallContract(ContractCall {
                    contract,
                    input: Vec::new(),
                    gas_limit: per_call,
                }),
                1,
                key,
            )
        })
        .collect();
    let call_gas: u64 = modules
        .iter()
        .map(|m| maya_vm::gas::call_fuel(m.len(), false))
        .sum();
    eprintln!("TIMING   ({CALLS} modules of {} B)", modules[0].len());
    let ms = time_block(
        "cold calls (compile)",
        &fixture.db,
        &block_of(txs),
        2,
        call_gas,
    );
    assert_inside("cold calls", ms);
}

#[test]
fn a_block_of_guest_compute_validates_inside_the_block_time() {
    let fixture = fixture(1);
    let limit = block_cap();
    let body = "(drop (i64.div_u (i64.const -1) (i64.extend_i32_u (i32.add (local.get $i) (i32.const 3)))))";
    let make = |n| looping("", "", body, n);
    let n = iterations_within(limit, make, || MemoryState::at_height(2));
    let contract = deploy_one(&fixture.db, &fixture.keys[0], &make(n));
    let tx = signed(
        TxKind::CallContract(ContractCall {
            contract,
            input: Vec::new(),
            gas_limit: limit,
        }),
        1,
        &fixture.keys[0],
    );
    let ms = time_block(
        "guest compute (i64.div_u)",
        &fixture.db,
        &block_of(vec![tx]),
        2,
        limit,
    );
    assert_inside("guest compute", ms);
}
