//! Worst-case block timing under the contract gas caps (ADR-142).
//!
//! Each test fills one block, up to `vm.max_block_gas`, with one kind of the
//! most expensive priced contract work, and times `apply_block` on it, which
//! is what a validator does with a block it receives: signatures, execution,
//! the state root, and the commit.
//!
//! The figures printed (`TIMING ...`) are the measurement ADR-142's
//! derivation cites. The assertion is the requirement, a block inside the 1 s
//! block time. Every test here is `#[ignore]`d: wall time on a shared CI
//! runner proves nothing about a validator, and a loaded runner would fail at
//! random (PR #549 review). Run with `cargo test -p custom-l1-node --test
//! vm_gas_timing_tests -- --ignored --nocapture --test-threads 1` and record
//! the output in `reports/`. In the dev profile the node is unoptimised, so a
//! release build is faster than what is printed.

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
    extra_per_iteration: u64,
) -> u64 {
    let vm = Vm::new().unwrap();
    let gas = |n| {
        vm.execute(&make(n), [0; 32], &[], u64::MAX / 4, state())
            .outcome
            .expect("preflight runs")
            .gas_used
    };
    let (one, two) = (gas(1), gas(2));
    let per = two - one + extra_per_iteration;
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
#[ignore = "timing: run with --ignored --nocapture and record the figures in ADR-142"]
fn a_block_of_host_calls_validates_inside_the_block_time() {
    let fixture = fixture(CALLS as usize);
    let limit = block_cap() / CALLS;
    let import = r#"(import "env" "get_balance_u256" (func $b (param i32 i32) (result i32)))"#;
    let body = "(drop (call $b (i32.const 0) (i32.const 64)))";
    let make = |n| looping(import, "", body, n);
    let n = iterations_within(limit, make, || MemoryState::at_height(2), 0);
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
#[ignore = "timing: run with --ignored --nocapture and record the figures in ADR-142"]
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
    // Every read is of a distinct committed key, so on the node every one
    // is the block's first: cold (GAS-4). The in-memory preflight host has
    // nothing cold, so the surcharge is added per iteration here.
    let n = iterations_within(limit, make, filled, maya_vm::gas::STORAGE_COLD_READ_FUEL);
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
        "storage reads (cold price, RocksDB hits)",
        &fixture.db,
        &block_of(txs),
        2,
        block_cap(),
    );
    assert_inside("storage reads", ms);
}

#[test]
#[ignore = "timing: run with --ignored --nocapture and record the figures in ADR-142"]
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
    let n = iterations_within(
        limit,
        make,
        || {
            let mut state = MemoryState::at_height(2);
            state.caller = Some([1; 32]);
            state
        },
        0,
    );
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
#[ignore = "timing: run with --ignored --nocapture and record the figures in ADR-142"]
fn blocks_of_cold_compiles_validate_inside_the_block_time() {
    let fixture = fixture(CALLS as usize);
    // As large as ten cold calls fit in one block, by their whole cold price
    // (length and shape).
    let per_call = block_cap() / CALLS;
    let price = |m: &Vec<u8>| maya_vm::gas::cold_call_fuel(m).expect("shape");
    let mut target =
        usize::try_from((per_call - maya_vm::gas::CALL_BASE_FUEL) / maya_vm::gas::CODE_BYTE_FUEL)
            .unwrap();
    let modules: Vec<Vec<u8>> = loop {
        let modules: Vec<Vec<u8>> = (0..CALLS)
            .map(|salt| compile_heavy(target, salt + 1))
            .collect();
        if modules.iter().all(|m| price(m) <= per_call * 97 / 100) {
            break modules;
        }
        target = target * 9 / 10;
    };
    let deploy_gas: u64 = modules.iter().map(price).sum();
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
    let call_gas: u64 = modules.iter().map(price).sum();
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
#[ignore = "timing: run with --ignored --nocapture and record the figures in ADR-142"]
fn a_block_of_guest_compute_validates_inside_the_block_time() {
    let fixture = fixture(1);
    let limit = block_cap();
    let body = "(drop (i64.div_u (i64.const -1) (i64.extend_i32_u (i32.add (local.get $i) (i32.const 3)))))";
    let make = |n| looping("", "", body, n);
    let n = iterations_within(limit, make, || MemoryState::at_height(2), 0);
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

// ---------------------------------------------------------------------------
// PR #549 review: the cases the first measurement left out
// ---------------------------------------------------------------------------

/// xorshift64*, so the key order is reproducible.
fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed >> 12;
    *seed ^= *seed << 25;
    *seed ^= *seed >> 27;
    seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

#[test]
#[ignore = "timing: run with --ignored --nocapture and record the figures in ADR-142"]
fn cold_point_reads_against_the_read_price() {
    // The RocksDB reads behind `storage_read` when nothing is cached: a 1 MiB
    // block cache and direct I/O, so neither RocksDB nor the OS page cache
    // holds the data block. The node's own options otherwise (open_tuned).
    use rocksdb::{BlockBasedOptions, Cache, DB, Options, WriteBatch};
    let dir = TempDir::new().unwrap();
    let options = |cache_mib: usize, bloom: bool, direct: bool| {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        let mut block = BlockBasedOptions::default();
        block.set_block_cache(&Cache::new_lru_cache(cache_mib << 20));
        block.set_cache_index_and_filter_blocks(true);
        if bloom {
            block.set_bloom_filter(10.0, false);
        }
        opts.set_block_based_table_factory(&block);
        opts.set_use_direct_reads(direct);
        opts
    };
    let key = |i: u32| {
        let mut k = b"cstate:".to_vec();
        k.extend_from_slice(&[7u8; 32]);
        k.extend_from_slice(&i.to_le_bytes());
        k
    };
    for (label, count, value_len, bloom) in [
        ("1 B values, no bloom", 400_000u32, 1usize, false),
        ("4 KiB values, no bloom", 40_000, 4_096, false),
        // For the record: a whole-file bloom filter that the 1 MiB cache
        // cannot hold is reread on every lookup, which is why `open_tuned`
        // has none.
        ("1 B values, bloom", 400_000, 1, true),
    ] {
        let path = dir.path().join(label.replace([' ', ','], "_"));
        {
            let db = DB::open(&options(64, bloom, false), &path).unwrap();
            let value = vec![9u8; value_len];
            let mut batch = WriteBatch::default();
            for i in 0..count {
                batch.put(key(i), &value);
                if i % 10_000 == 9_999 {
                    db.write(std::mem::take(&mut batch)).unwrap();
                }
            }
            db.write(batch).unwrap();
            db.flush().unwrap();
            db.compact_range::<&[u8], &[u8]>(None, None);
        }
        let (db, direct) = match DB::open(&options(1, bloom, true), &path) {
            Ok(db) => (db, "direct I/O"),
            Err(error) => {
                eprintln!("TIMING   (direct reads refused: {error}; OS cache in play)");
                (
                    DB::open(&options(1, bloom, false), &path).unwrap(),
                    "buffered",
                )
            }
        };
        let reads = 20_000u32;
        let mut seed = 0x9E37_79B9_7F4A_7C15;
        let started = Instant::now();
        for _ in 0..reads {
            let i = u32::try_from(next(&mut seed) % u64::from(count)).unwrap();
            assert!(db.get(key(i)).unwrap().is_some());
        }
        let hit = started.elapsed().as_secs_f64() * 1e6 / f64::from(reads);
        let started = Instant::now();
        for _ in 0..reads {
            let i = count + u32::try_from(next(&mut seed) % u64::from(count)).unwrap();
            assert!(db.get(key(i)).unwrap().is_none());
        }
        let absent = started.elapsed().as_secs_f64() * 1e6 / f64::from(reads);
        eprintln!(
            "TIMING cold read {label:<22} {count:>7} keys, 1 MiB cache, {direct}: \
             hit {hit:>7.2} us  absent {absent:>7.2} us  (read base {} fuel)",
            maya_vm::gas::STORAGE_READ_BASE_FUEL
        );
    }
}

const FOREVER_WAT: &str = r#"(module (memory (export "memory") 1)
    (func (export "invoke") (param i32) (result i64)
      (loop $forever (br $forever)) (i64.const 0)))"#;

#[test]
#[ignore = "timing: run with --ignored --nocapture and record the figures in ADR-142"]
fn building_a_block_of_failing_max_gas_calls_stays_inside_the_block_time() {
    // Every DAG-BFT validator runs selection on the same sub-DAG. A failing
    // call is not included, so before the review fix its gas was never
    // counted: K of them cost K x the cap in execution.
    const FAILING: usize = 8;
    let fixture = fixture(FAILING);
    let contract = deploy_one(&fixture.db, &fixture.keys[0], &wasm(FOREVER_WAT));
    let mut txs = calls(&fixture, contract, block_cap(), 0);
    txs.truncate(FAILING);
    txs[0] = calls(&fixture, contract, block_cap(), 1).remove(0);
    let started = Instant::now();
    let kept = fixture
        .db
        .select_applicable(
            txs,
            BlockContext::at_height(2),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    let ms = started.elapsed().as_secs_f64() * 1e3;
    eprintln!("TIMING selection, {FAILING} failing max-gas calls {ms:>9.1} ms");
    assert!(kept.is_empty());
    assert_inside("failing calls in selection", ms);
}

#[test]
#[ignore = "timing: run with --ignored --nocapture and record the figures in ADR-142"]
fn a_block_that_writes_then_calls_many_times_validates_inside_the_block_time() {
    // Before the review fix every call copied this block's earlier writes to
    // its contract into a fresh map: half a block of small writes, then many
    // cheap calls, was O(entries x calls) work at no price.
    const READS: u64 = 200;
    let fixture = fixture(1);
    let make = |n: u64| {
        wasm(&format!(
            r#"(module
                (import "env" "storage_write" (func $w (param i32 i32 i32 i32)))
                (import "env" "storage_read" (func $r (param i32 i32 i32 i32) (result i32)))
                (memory (export "memory") 1)
                (func (export "input_ptr") (result i32) (i32.const 1024))
                (func (export "input_cap") (result i32) (i32.const 16))
                (func (export "invoke") (param $len i32) (result i64)
                  (local $i i32)
                  (if (i32.eqz (local.get $len))
                    (then (block $done (loop $again
                      (br_if $done (i32.ge_u (local.get $i) (i32.const {n})))
                      (i32.store (i32.const 0) (local.get $i))
                      (call $w (i32.const 0) (i32.const 4) (i32.const 8) (i32.const 1))
                      (local.set $i (i32.add (local.get $i) (i32.const 1)))
                      (br $again))))
                    (else (drop (call $r (i32.const 0) (i32.const 4) (i32.const 64) (i32.const 64)))))
                  (i64.const 0)))"#
        ))
    };
    let half = block_cap() / 2;
    let n = iterations_within(half, make, || MemoryState::at_height(2), 0);
    let contract = deploy_one(&fixture.db, &fixture.keys[0], &make(n));
    let key = &fixture.keys[0];
    let call = |input: Vec<u8>, gas_limit: u64, nonce: u64| {
        signed(
            TxKind::CallContract(ContractCall {
                contract,
                input,
                gas_limit,
            }),
            nonce,
            key,
        )
    };
    let mut txs = vec![call(Vec::new(), half, 1)];
    txs.extend((0..READS).map(|i| call(vec![1], 1_000_000, 2 + i)));
    let ms = time_block(
        &format!("{n} writes, then {READS} warm calls"),
        &fixture.db,
        &block_of(txs),
        2,
        half + READS * 1_000_000,
    );
    assert_inside("writes then warm calls", ms);
}

#[test]
#[ignore = "timing: run with --ignored --nocapture and record the figures in ADR-142"]
fn a_block_of_fresh_linear_memory_validates_inside_the_block_time() {
    // Touching a fresh page costs a page fault and zeroing that the fuel
    // meter does not see; `MEMORY_PAGE_FUEL` is what prices it now.
    let fixture = fixture(1);
    let code = wasm(
        r#"(module (memory (export "memory") 256)
            (func (export "invoke") (param i32) (result i64)
              (local $p i32)
              (loop $l
                (i32.store8 (local.get $p) (i32.const 1))
                (local.set $p (i32.add (local.get $p) (i32.const 4096)))
                (br_if $l (i32.lt_u (local.get $p) (i32.const 16777216))))
              (i64.const 0)))"#,
    );
    let vm = Vm::new().unwrap();
    let per_call = vm
        .execute(&code, [0; 32], &[], u64::MAX / 4, MemoryState::at_height(2))
        .outcome
        .expect("runs")
        .gas_used;
    let contract = deploy_one(&fixture.db, &fixture.keys[0], &code);
    let count = (block_cap() / (per_call + 1)).min(400);
    let txs = (0..count)
        .map(|i| {
            signed(
                TxKind::CallContract(ContractCall {
                    contract,
                    input: Vec::new(),
                    gas_limit: per_call + 1,
                }),
                1 + i,
                &fixture.keys[0],
            )
        })
        .collect();
    let ms = time_block(
        &format!("{count} calls touching 16 MiB each"),
        &fixture.db,
        &block_of(txs),
        2,
        count * per_call,
    );
    assert_inside("fresh linear memory", ms);
}

#[test]
#[ignore = "timing: run with --ignored --nocapture and record the figures in ADR-142"]
fn the_contracts_root_fold_grows_with_total_contract_state() {
    // Not a gas price: the contracts layer of the state root folds every
    // contract key every block (ROOT-5, ADR-142 "open"). Measured so the
    // open item carries a number: an empty block on top of N committed
    // contract entries, which is the fold and nothing else.
    for entries in [0u32, 100_000, 400_000] {
        let fixture = fixture(1);
        for i in 0..entries {
            let mut key = b"cstate:".to_vec();
            key.extend_from_slice(&[9u8; 32]);
            key.extend_from_slice(&i.to_le_bytes());
            fixture.db.raw_put_for_test(&key, &[1]).unwrap();
        }
        let started = Instant::now();
        fixture
            .db
            .apply_block(&block_of(Vec::new()), BlockContext::at_height(1))
            .unwrap();
        let ms = started.elapsed().as_secs_f64() * 1e3;
        eprintln!("TIMING contracts root fold, {entries:>7} entries {ms:>9.1} ms");
    }
}
