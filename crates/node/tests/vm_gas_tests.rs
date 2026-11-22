//! Contract gas caps (ADR-142): a call's `gas_limit` is capped per
//! transaction, contract work is capped per block, and both caps are
//! governed parameters with compiled bounds.
//!
//! Refused at admission and in block validation, and the block builder
//! leaves a call that does not fit for a later block.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::core::payload::{ContractCall, ContractDeploy, derive_contract_id};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::network::mempool::Mempool;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_governance::params::ParameterKey;
use tempfile::TempDir;

mod common;

const WRITER_WAT: &str = r#"(module
    (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
    (memory (export "memory") 1)
    (func (export "invoke") (param i32) (result i64)
      (i32.store8 (i32.const 0) (i32.const 107))
      (i32.store (i32.const 16) (i32.const 1234))
      (call $write (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 4))
      (i64.const 0)))"#;

fn tx_cap() -> u64 {
    ParameterKey::VmMaxTxGas
        .bounds()
        .default
        .saturating_to_u64()
}

fn block_cap() -> u64 {
    ParameterKey::VmMaxBlockGas
        .bounds()
        .default
        .saturating_to_u64()
}

fn wasm(text: &str) -> Vec<u8> {
    wat::parse_str(text).expect("valid WAT")
}

fn open_state(funded: &[Address]) -> (StateDB, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    common::bind(&db);
    for address in funded {
        db.put_account(
            address,
            &Account {
                balance: primitive_types::U256::from(1_000_000u64),
                nonce: 0,
            },
        )
        .expect("fund");
    }
    (db, dir)
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

fn call(contract: [u8; 32], gas_limit: u64, nonce: u64, key: &HybridSigningKey) -> Transaction {
    signed(
        TxKind::CallContract(ContractCall {
            contract,
            input: Vec::new(),
            gas_limit,
        }),
        nonce,
        key,
    )
}

/// Deploys `code` from `key` at nonce 0, in block 1.
fn deploy(db: &StateDB, key: &HybridSigningKey, code: &[u8]) -> [u8; 32] {
    let tx = signed(
        TxKind::DeployContract(ContractDeploy {
            code: code.to_vec(),
        }),
        0,
        key,
    );
    db.apply_block(&block_of(vec![tx]), BlockContext::at_height(1))
        .expect("deploy");
    derive_contract_id(&key.address(), 0, code)
}

#[test]
fn the_caps_are_governed_with_compiled_bounds() {
    for key in [ParameterKey::VmMaxTxGas, ParameterKey::VmMaxBlockGas] {
        let bounds = key.bounds();
        assert!(bounds.contains(bounds.default), "{}", key.label());
        // No vote can lift either cap to "unbounded".
        assert!(
            key.check(maya_governance::Amount::from_u64(u64::MAX))
                .is_err()
        );
        assert!(key.check(maya_governance::Amount::from_u64(0)).is_err());
    }
    // A maximum-size module must stay callable under the lowest block cap.
    let max_module = maya_vm::gas::call_fuel(maya_vm::MAX_MODULE_BYTES, false);
    assert!(ParameterKey::VmMaxBlockGas.bounds().min.saturating_to_u64() >= max_module);
}

#[test]
fn a_call_over_the_transaction_cap_is_refused_in_block_validation() {
    let caller = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[caller.address()]);
    let contract = deploy(&db, &caller, &wasm(WRITER_WAT));

    for limit in [tx_cap() + 1, u64::MAX] {
        let result = db.apply_block(
            &block_of(vec![call(contract, limit, 1, &caller)]),
            BlockContext::at_height(2),
        );
        assert_eq!(
            result,
            Err(NodeError::GasAboveCap {
                limit,
                cap: tx_cap().min(block_cap())
            })
        );
    }
    // At the cap it runs.
    db.apply_block(
        &block_of(vec![call(contract, tx_cap(), 1, &caller)]),
        BlockContext::at_height(2),
    )
    .expect("a call at the cap applies");
}

#[test]
fn a_call_over_the_transaction_cap_is_refused_at_admission() {
    let caller = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[caller.address()]);
    let contract = deploy(&db, &caller, &wasm(WRITER_WAT));
    let pool = Mempool::new(Arc::new(db));

    assert!(matches!(
        pool.insert(call(contract, u64::MAX, 1, &caller)),
        Err(NodeError::GasAboveCap {
            limit: u64::MAX,
            ..
        })
    ));
    assert_eq!(pool.insert(call(contract, tx_cap(), 1, &caller)), Ok(true));
}

#[test]
fn the_block_cap_bounds_the_gas_of_the_whole_block() {
    let first = generate_signing_key().unwrap();
    let second = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[first.address(), second.address()]);
    let contract = deploy(&db, &first, &wasm(WRITER_WAT));

    // Each asks for the whole block. The first uses a little of it, so the
    // second no longer fits.
    let limit = tx_cap().min(block_cap());
    let a = call(contract, limit, 1, &first);
    let b = call(contract, limit, 0, &second);
    let result = db.apply_block(
        &block_of(vec![a.clone(), b.clone()]),
        BlockContext::at_height(2),
    );
    assert!(
        matches!(result, Err(NodeError::BlockGasExhausted { needed, .. }) if needed == limit),
        "{result:?}"
    );

    // The builder leaves the second out, and it applies in the next block.
    let kept = db
        .select_applicable(
            vec![a.clone(), b.clone()],
            BlockContext::at_height(2),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    assert_eq!(kept, vec![a.clone()]);
    db.apply_block(&block_of(kept), BlockContext::at_height(2))
        .unwrap();
    db.apply_block(&block_of(vec![b]), BlockContext::at_height(3))
        .expect("left out, applied in the next block");
}

/// A valid module of about `len` bytes: one passive data segment.
fn module_of_len(len: usize) -> Vec<u8> {
    let data = "a".repeat(len);
    wasm(&format!(
        r#"(module (memory (export "memory") 1) (data "{data}")
            (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#
    ))
}

#[test]
fn a_deployment_counts_against_the_block_cap() {
    let first = generate_signing_key().unwrap();
    let second = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[first.address(), second.address()]);
    // Each deploy is a little over half the block's gas.
    let len = usize::try_from(block_cap() / 2 / maya_vm::gas::CODE_BYTE_FUEL).unwrap();
    let code = module_of_len(len);
    assert!(maya_vm::gas::deploy_fuel(code.len()) * 2 > block_cap());
    let deploy_from = |key: &HybridSigningKey| {
        signed(
            TxKind::DeployContract(ContractDeploy { code: code.clone() }),
            0,
            key,
        )
    };
    let result = db.apply_block(
        &block_of(vec![deploy_from(&first), deploy_from(&second)]),
        BlockContext::at_height(1),
    );
    assert!(
        matches!(result, Err(NodeError::BlockGasExhausted { .. })),
        "{result:?}"
    );
    db.apply_block(
        &block_of(vec![deploy_from(&first)]),
        BlockContext::at_height(1),
    )
    .expect("one fits");
}

/// The error class a result names, as the vectors spell it.
fn class(result: &Result<(), NodeError>) -> Option<&'static str> {
    match result {
        Ok(()) => None,
        Err(NodeError::GasAboveCap { .. }) => Some("GasAboveCap"),
        Err(NodeError::BlockGasExhausted { .. }) => Some("BlockGasExhausted"),
        Err(NodeError::BlockContractWritesExhausted { .. }) => Some("BlockContractWritesExhausted"),
        Err(NodeError::ContractStateFull { .. }) => Some("ContractStateFull"),
        Err(_) => Some("other"),
    }
}

#[test]
fn the_spec_vectors_hold() {
    use custom_l1_node::state::contract_bound;
    use custom_l1_node::state::vm_exec::{admit_call_gas, admit_deploy_gas};
    use maya_vm::gas;

    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/tests/vm_gas.json");
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("vectors")).expect("json");
    let num = |v: &serde_json::Value| -> u64 {
        v.as_str().expect("decimal string").parse().expect("u64")
    };
    let cases = doc["cases"].as_array().expect("cases");
    assert!(cases.len() >= 40);
    let mut wrong = Vec::new();
    for case in cases {
        let id = case["id"].as_str().unwrap();
        let input = &case["input"];
        let expect = &case["expect"];
        let wants_error = expect["result"] == "error";
        let (got_error, value): (Option<&str>, Option<u64>) = match case["kind"].as_str().unwrap() {
            "call_fuel" => (
                None,
                Some(gas::call_fuel(
                    usize::try_from(num(&input["code_len"])).unwrap(),
                    input["warm"].as_bool().unwrap(),
                )),
            ),
            "call_affordable" => {
                let price = gas::call_fuel(
                    usize::try_from(num(&input["code_len"])).unwrap(),
                    input["warm"].as_bool().unwrap(),
                );
                (
                    (num(&input["gas_limit"]) < price).then_some("OutOfGas"),
                    None,
                )
            }
            "host_price" => {
                let bytes = num(&input["bytes"]);
                let price = match input["op"].as_str().unwrap() {
                    "host_call" => gas::HOST_CALL_FUEL,
                    "storage_read_lookup" => gas::priced(
                        gas::STORAGE_READ_BASE_FUEL,
                        gas::STORAGE_READ_BYTE_FUEL,
                        bytes,
                    ),
                    "storage_read_value" => gas::priced(0, gas::STORAGE_READ_BYTE_FUEL, bytes),
                    "storage_write" => gas::priced(
                        gas::STORAGE_WRITE_BASE_FUEL,
                        gas::STORAGE_WRITE_BYTE_FUEL,
                        bytes,
                    ),
                    "emit_event" => gas::priced(gas::HOST_CALL_FUEL, gas::EVENT_BYTE_FUEL, bytes),
                    other => panic!("{id}: unknown op {other}"),
                };
                (None, Some(price))
            }
            "admit_call" => (
                class(&admit_call_gas(
                    num(&input["gas_limit"]),
                    num(&input["tx_cap"]),
                    num(&input["block_cap"]),
                    num(&input["block_used"]),
                )),
                None,
            ),
            // The length part of the price; a module's shape only adds to it.
            "admit_deploy" => (
                class(&admit_deploy_gas(
                    gas::deploy_fuel(usize::try_from(num(&input["code_len"])).unwrap()),
                    num(&input["block_cap"]),
                    num(&input["block_used"]),
                )),
                None,
            ),
            "shape_fuel" => (
                None,
                Some(gas::shape_fuel(&maya_vm::shape::ModuleShape {
                    functions: num(&input["functions"]),
                    locals: num(&input["locals"]),
                    controls: num(&input["controls"]),
                    calls: num(&input["calls"]),
                    arity: num(&input["arity"]),
                    loop_arity: num(&input["loop_arity"]),
                })),
            ),
            "memory_fuel" => (None, Some(gas::MEMORY_PAGE_FUEL * num(&input["pages"]))),
            "sealed_budget" => (
                None,
                Some(custom_l1_node::state::vm_exec::sealed_gas_budget(num(
                    &input["block_cap"],
                ))),
            ),
            // A real execution, end to end: the gas a small corpus uses must
            // be the same number on every platform CI runs.
            "golden_gas" => {
                let code = wat::parse_str(input["wat"].as_str().unwrap()).expect("wat");
                let mut state = maya_vm::MemoryState::at_height(1);
                if let Some(value) = input["stored"].as_str() {
                    state
                        .storage
                        .insert(([5u8; 32], b"k".to_vec()), value.as_bytes().to_vec());
                }
                let execution = maya_vm::Vm::new().unwrap().execute(
                    &code,
                    [5u8; 32],
                    &[],
                    num(&input["gas_limit"]),
                    state,
                );
                let error = match &execution.outcome {
                    Ok(_) => None,
                    Err(maya_vm::VmError::OutOfGas { .. }) => Some("OutOfGas"),
                    Err(maya_vm::VmError::SizeLimit { .. }) => Some("SizeLimit"),
                    Err(maya_vm::VmError::MemoryLimit { .. }) => Some("MemoryLimit"),
                    Err(_) => Some("Trap"),
                };
                (error, Some(execution.gas_used))
            }
            "builder_select" => {
                let (kept, passed_over) = replay_selection(input);
                let expected: Vec<usize> = expect["kept"]
                    .as_array()
                    .expect("kept")
                    .iter()
                    .map(|index| usize::try_from(num(index)).unwrap())
                    .collect();
                if kept != expected {
                    wrong.push(format!("{id}: kept {kept:?}, expected {expected:?}"));
                }
                (passed_over.then_some("PassedOver"), None)
            }
            "contract_entry_size" => (
                None,
                Some(contract_bound::entry_size(
                    usize::try_from(num(&input["key_len"])).unwrap(),
                    usize::try_from(num(&input["value_len"])).unwrap(),
                )),
            ),
            "admit_contract_writes" => (
                class(&contract_bound::admit_block_writes(
                    num(&input["before"]),
                    num(&input["after"]),
                    num(&input["cap"]),
                )),
                None,
            ),
            "admit_contract_state" => (
                class(&contract_bound::admit_state_size(
                    num(&input["before"]),
                    num(&input["after"]),
                    num(&input["cap"]),
                )),
                None,
            ),
            "param" => {
                let key =
                    ParameterKey::from_tag(u16::try_from(num(&input["tag"])).unwrap()).unwrap();
                let checked = key.check(maya_governance::Amount::from_u64(num(&input["value"])));
                (checked.is_err().then_some("ParameterOutOfRange"), None)
            }
            other => panic!("{id}: unknown kind {other}"),
        };
        // Every mismatch, not the first: one run names every vector to fix.
        if got_error.is_some() != wants_error
            || got_error.is_some_and(|error| Some(error) != expect["error"].as_str())
        {
            wrong.push(format!(
                "{id}: error {got_error:?}, expected {}",
                expect["error"]
            ));
        }
        if let Some(value) = value
            && value != num(&expect["value"])
        {
            wrong.push(format!("{id}: value {value}, expected {}", expect["value"]));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A `builder_select` vector (GAS-9): deploys `contracts` at height 1, each
/// from a key of its own, then selects `txs` at height 2. Returns the indices
/// kept and whether a call to `writer`, which applies whenever it is tried,
/// was left out.
fn replay_selection(input: &serde_json::Value) -> (Vec<usize>, bool) {
    let num = |v: &serde_json::Value| -> u64 { v.as_str().unwrap().parse().unwrap() };
    let contracts = input["contracts"].as_object().expect("contracts");
    let txs = input["txs"].as_array().expect("txs");
    let senders = txs
        .iter()
        .map(|tx| usize::try_from(num(&tx["sender"])).unwrap() + 1)
        .max()
        .unwrap_or(0);
    let deployers = keys(contracts.len());
    let senders = keys(senders);
    let funded: Vec<Address> = deployers
        .iter()
        .chain(&senders)
        .map(HybridSigningKey::address)
        .collect();
    let (db, _dir) = open_state(&funded);
    let codes: Vec<Vec<u8>> = contracts
        .values()
        .map(|wat| wasm(wat.as_str().unwrap()))
        .collect();
    let ids = deploy_all(&db, &deployers.iter().collect::<Vec<_>>(), &codes);
    let id_of: std::collections::BTreeMap<&str, [u8; 32]> =
        contracts.keys().map(String::as_str).zip(ids).collect();
    let order: Vec<Transaction> = txs
        .iter()
        .map(|tx| {
            call(
                id_of[tx["contract"].as_str().unwrap()],
                num(&tx["gas_limit"]),
                num(&tx["nonce"]),
                &senders[usize::try_from(num(&tx["sender"])).unwrap()],
            )
        })
        .collect();
    let block = db
        .select_applicable(
            order.clone(),
            BlockContext::at_height(2),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    let kept: Vec<usize> = order
        .iter()
        .enumerate()
        .filter(|(_, tx)| block.contains(tx))
        .map(|(index, _)| index)
        .collect();
    let passed_over = txs
        .iter()
        .enumerate()
        .any(|(index, tx)| tx["contract"] == "writer" && !kept.contains(&index));
    (kept, passed_over)
}

#[test]
fn the_governance_floor_is_the_vm_price_of_the_largest_module() {
    // `maya-governance` has no dependencies, so it restates the VM's price of
    // a maximum-size cold call; this is where the two are held equal.
    let floor = ParameterKey::VmMaxBlockGas.bounds().min.saturating_to_u64();
    assert_eq!(
        floor,
        maya_vm::gas::call_fuel(maya_vm::MAX_MODULE_BYTES, false) + 4_000_000
    );
    assert_eq!(
        ParameterKey::VmMaxTxGas.bounds().min.saturating_to_u64(),
        floor
    );
}

// ---------------------------------------------------------------------------
// PR #549 review: work the block cap did not see
// ---------------------------------------------------------------------------

const FOREVER_WAT: &str = r#"(module (memory (export "memory") 1)
    (func (export "invoke") (param i32) (result i64)
      (loop $forever (br $forever)) (i64.const 0)))"#;

const TRAP_WAT: &str = r#"(module (memory (export "memory") 1)
    (func (export "invoke") (param i32) (result i64) unreachable))"#;

/// Deploys each of `codes` from its own key, in one block at height 1.
fn deploy_all(db: &StateDB, keys: &[&HybridSigningKey], codes: &[Vec<u8>]) -> Vec<[u8; 32]> {
    let txs = keys
        .iter()
        .zip(codes)
        .map(|(key, code)| {
            signed(
                TxKind::DeployContract(ContractDeploy { code: code.clone() }),
                0,
                key,
            )
        })
        .collect();
    db.apply_block(&block_of(txs), BlockContext::at_height(1))
        .expect("deploy");
    keys.iter()
        .zip(codes)
        .map(|(key, code)| derive_contract_id(&key.address(), 0, code))
        .collect()
}

/// What an out-of-gas call to `FOREVER_WAT` reports: its limit and its one
/// page (GAS-2, GAS-7).
fn spin_gas(limit: u64) -> u64 {
    limit + maya_vm::gas::MEMORY_PAGE_FUEL
}

/// The builder's budget for applied and failed contract work together.
fn builder_budget() -> u64 {
    use custom_l1_node::state::vm_exec::{BUILDER_GAS_CAPS, BUILDER_GAS_SLACK};
    BUILDER_GAS_CAPS * block_cap() + BUILDER_GAS_SLACK
}

#[test]
fn a_failing_call_at_the_cap_does_not_crowd_out_a_call_that_applies() {
    // Before (PR #549 review): a failed trial's gas stayed on the counter the
    // applied calls are admitted against. The failing call is never included,
    // so it pays nothing and stays pooled: one at the cap, early in the
    // sub-DAG, left every contract call after it out, every block.
    let attacker = generate_signing_key().unwrap();
    let honest = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[attacker.address(), honest.address()]);
    let ids = deploy_all(
        &db,
        &[&attacker, &honest],
        &[wasm(FOREVER_WAT), wasm(WRITER_WAT)],
    );
    let spin = call(ids[0], block_cap(), 1, &attacker);
    let after = call(ids[1], block_cap(), 1, &honest);
    let kept = db
        .select_applicable(
            vec![spin, after.clone()],
            BlockContext::at_height(2),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    assert_eq!(kept, vec![after.clone()]);
    // And the block it builds is valid: the failed work was never the block's.
    db.apply_block(&block_of(kept), BlockContext::at_height(2))
        .expect("the builder's block validates");
}

#[test]
fn failed_work_past_the_builder_budget_stops_contract_trials() {
    // The bound on the builder: failed work keeps its own count, and once
    // applied and failed work reach the budget no more contract work is
    // tried. Two failing calls at the cap, from two senders, get there; that
    // remaining lever is ADR-142 G1, closed only by charging failed calls.
    let spinners = [
        generate_signing_key().unwrap(),
        generate_signing_key().unwrap(),
    ];
    let honest = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[
        spinners[0].address(),
        spinners[1].address(),
        honest.address(),
    ]);
    let ids = deploy_all(
        &db,
        &[&spinners[0], &honest],
        &[wasm(FOREVER_WAT), wasm(WRITER_WAT)],
    );
    // Above the slack: a call this small would still fit after them.
    let limit = 2_000_000;
    let after = call(ids[1], limit, 1, &honest);
    let kept = db
        .select_applicable(
            vec![
                call(ids[0], block_cap(), 1, &spinners[0]),
                call(ids[0], block_cap(), 0, &spinners[1]),
                after,
            ],
            BlockContext::at_height(2),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    assert!(2 * spin_gas(block_cap()) + limit > builder_budget());
    assert!(kept.is_empty(), "{} kept", kept.len());
}

#[test]
fn one_sender_gets_one_failed_contract_trial_per_block() {
    // A sub-DAG can carry several versions of one sender's nonce, from
    // different proposers. Each failing one was tried, each free.
    let attacker = generate_signing_key().unwrap();
    let honest = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[attacker.address(), honest.address()]);
    let ids = deploy_all(
        &db,
        &[&attacker, &honest],
        &[wasm(FOREVER_WAT), wasm(WRITER_WAT)],
    );
    let spin = call(ids[0], block_cap(), 1, &attacker);
    let again = call(ids[0], block_cap() - 1, 1, &attacker);
    let after = call(ids[1], block_cap() / 2, 1, &honest);
    let kept = db
        .select_applicable(
            vec![spin, again, after.clone()],
            BlockContext::at_height(2),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    // The second version is not tried. Tried, the two would fill the
    // builder's budget and the honest call would not be.
    assert!(spin_gas(block_cap()) + spin_gas(block_cap() - 1) + block_cap() / 2 > builder_budget());
    assert_eq!(kept, vec![after]);
}

/// One failing call at the cap from `attacker`, first, then calls from each
/// of `honest`: the first at the cap, the rest at 1,000,000. `nonce` is the
/// honest senders' (the attacker's call is never included, so it keeps 1).
fn attack_order(
    ids: &[[u8; 32]],
    attacker: &HybridSigningKey,
    honest: &[HybridSigningKey],
    nonce: u64,
) -> (Transaction, Vec<Transaction>) {
    let spin = call(ids[0], block_cap(), 1, attacker);
    let calls = honest
        .iter()
        .enumerate()
        .map(|(i, key)| {
            // The deployer of the writer used nonce 0 for it.
            let nonce = if i == 0 { nonce + 1 } else { nonce };
            let limit = if i == 0 { block_cap() } else { 1_000_000 };
            call(ids[1], limit, nonce, key)
        })
        .collect();
    (spin, calls)
}

fn keys(n: usize) -> Vec<HybridSigningKey> {
    (0..n).map(|_| generate_signing_key().unwrap()).collect()
}

#[test]
fn a_pooled_failing_call_at_the_cap_leaves_honest_calls_in_every_produced_block() {
    // 09 R-2, producer path: the failing call is admitted (pooled), placed
    // first in every block's order, never included and never charged, and
    // the honest calls after it are included in each block regardless.
    let attacker = generate_signing_key().unwrap();
    let honest = keys(3);
    let mut funded: Vec<Address> = honest.iter().map(HybridSigningKey::address).collect();
    funded.push(attacker.address());
    let (db, _dir) = open_state(&funded);
    let db = Arc::new(db);
    let ids = deploy_all(
        &db,
        &[&attacker, &honest[0]],
        &[wasm(FOREVER_WAT), wasm(WRITER_WAT)],
    );
    let pool = Mempool::new(Arc::clone(&db));
    for height in 2..4 {
        let (spin, calls) = attack_order(&ids, &attacker, &honest, height - 2);
        if height == 2 {
            assert_eq!(
                pool.insert(spin.clone()),
                Ok(true),
                "admission checks only the caps"
            );
        }
        let mut order = vec![spin];
        order.extend(calls.iter().cloned());
        let kept = db
            .select_applicable(
                order,
                BlockContext::at_height(height),
                target_from_leading_zero_bits(0),
            )
            .unwrap();
        assert_eq!(kept, calls, "height {height}");
        db.apply_block(&block_of(kept), BlockContext::at_height(height))
            .expect("the builder's block validates");
        // Never included: no fee, no nonce bump, so it is pooled again.
        assert_eq!(db.get_account(&attacker.address()).unwrap().nonce, 1);
    }
}

/// A DAG-BFT chain whose tip has `ids` deployed: FOREVER by `attacker`, the
/// writer by `honest[0]`, at height 1.
fn bft_chain(
    attacker: &HybridSigningKey,
    honest: &[HybridSigningKey],
) -> (
    custom_l1_node::consensus::chain::Chain,
    Vec<[u8; 32]>,
    TempDir,
) {
    use custom_l1_node::consensus::chain::ChainConfig;
    let dir = TempDir::new().unwrap();
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    let mut genesis = block_of(Vec::new());
    genesis.header.timestamp = 1_000_000;
    let mut chain = common::open_chain(state, genesis, ChainConfig::dag_bft()).unwrap();
    for address in honest
        .iter()
        .map(HybridSigningKey::address)
        .chain([attacker.address()])
    {
        chain
            .state()
            .put_account(
                &address,
                &Account {
                    balance: primitive_types::U256::from(1_000_000u64),
                    nonce: 0,
                },
            )
            .unwrap();
    }
    let codes = [wasm(FOREVER_WAT), wasm(WRITER_WAT)];
    let deploys = [attacker, &honest[0]]
        .iter()
        .zip(&codes)
        .map(|(key, code)| {
            signed(
                TxKind::DeployContract(ContractDeploy { code: code.clone() }),
                0,
                key,
            )
        })
        .collect();
    let block = chain.candidate_block(1_000_001, deploys).unwrap();
    chain.insert_block(block).unwrap();
    let ids = [attacker, &honest[0]]
        .iter()
        .zip(&codes)
        .map(|(key, code)| derive_contract_id(&key.address(), 0, code))
        .collect();
    (chain, ids, dir)
}

/// A one-vertex sub-DAG at `round` carrying `txs` in order.
fn sub_dag_of(round: u64, txs: &[Transaction]) -> maya_dag_bft::SubDag {
    use maya_dag_bft::{Certificate, Vertex};
    let anchor = Certificate {
        vertex: Vertex {
            epoch: 0,
            round,
            author: 0,
            timestamp_ms: (1_000_001 + round) * 1_000,
            parents: Vec::new(),
            batch: txs.iter().map(Transaction::to_bytes).collect(),
        },
        votes: Vec::new(),
        signatures: Vec::new(),
    };
    maya_dag_bft::SubDag {
        anchor: anchor.clone(),
        certificates: vec![anchor],
    }
}

#[test]
fn a_pooled_failing_call_at_the_cap_leaves_honest_calls_in_every_bft_block() {
    // 09 R-2, DAG-BFT path: every validator derives the block from the
    // committed order, the failing call first, through `build_block`.
    let attacker = generate_signing_key().unwrap();
    let honest = keys(3);
    let (mut chain, ids, _dir) = bft_chain(&attacker, &honest);
    for round in 2..4 {
        let (spin, calls) = attack_order(&ids, &attacker, &honest, round - 2);
        let mut order = vec![spin];
        order.extend(calls.iter().cloned());
        let block =
            custom_l1_node::consensus::bft::build_block(&chain, &sub_dag_of(round, &order), 1)
                .unwrap();
        assert_eq!(block.transactions, calls, "round {round}");
        chain
            .insert_block(block)
            .expect("the derived block imports");
        assert_eq!(
            chain
                .state()
                .get_account(&attacker.address())
                .unwrap()
                .nonce,
            1
        );
    }
}

#[test]
fn a_replayed_old_transaction_does_not_shut_its_sender_out() {
    // A transaction refused at its nonce did no contract work, and anyone can
    // put a sender's old transaction in a sub-DAG: it must not cost the
    // sender its contract calls for the block.
    let honest = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[honest.address()]);
    let code = wasm(WRITER_WAT);
    let ids = deploy_all(&db, &[&honest], std::slice::from_ref(&code));
    let replay = signed(TxKind::DeployContract(ContractDeploy { code }), 0, &honest);
    let after = call(ids[0], 1_000_000, 1, &honest);
    let kept = db
        .select_applicable(
            vec![replay, after.clone()],
            BlockContext::at_height(2),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    assert_eq!(kept, vec![after]);
}

#[test]
fn a_call_below_its_upfront_price_still_counts_its_code_read_and_scan() {
    // Refused before the compile, but after a read of the code and a scan of
    // its shape: before the PR #549 review fix it counted nothing, so a pool
    // of them was builder work outside GAS-2's bound.
    // Seen at the edge of the builder's budget: two failing calls fill most
    // of it first, so the honest call fits only in what the refused one left.
    let spinners = [
        generate_signing_key().unwrap(),
        generate_signing_key().unwrap(),
    ];
    let attacker = generate_signing_key().unwrap();
    let honest = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[
        spinners[0].address(),
        spinners[1].address(),
        attacker.address(),
        honest.address(),
    ]);
    let code = wasm(WRITER_WAT);
    let ids = deploy_all(
        &db,
        &[&attacker, &spinners[0]],
        &[code.clone(), wasm(FOREVER_WAT)],
    );
    let counted = maya_vm::gas::CALL_BASE_FUEL + code.len() as u64;
    let half = block_cap() / 2;
    let select = |honest_limit: u64| {
        db.select_applicable(
            vec![
                call(ids[1], block_cap(), 1, &spinners[0]),
                call(ids[1], half, 0, &spinners[1]),
                call(ids[0], 1, 1, &attacker),
                call(ids[0], honest_limit, 0, &honest),
            ],
            BlockContext::at_height(2),
            target_from_leading_zero_bits(0),
        )
        .unwrap()
        .len()
    };
    let left = builder_budget() - spin_gas(block_cap()) - spin_gas(half) - counted;
    assert!(left < block_cap());
    assert_eq!(select(left), 1);
    assert_eq!(select(left + 1), 0);
}

#[test]
fn a_deployment_above_the_deploy_price_is_refused_at_admission() {
    // Under the block cap but over `MAX_COLD_CALL_FUEL`, which `Vm::validate`
    // refuses: before the review fix the pool took it, and every builder
    // paid its scan every block for a deployment that could never apply.
    let deployer = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[deployer.address()]);
    let code = wasm(&format!(
        r#"(module (memory 2) (data (i32.const 0) "{}") {})"#,
        "a".repeat(110_000),
        "(func)".repeat(1_500)
    ));
    let price = maya_vm::gas::cold_call_fuel(&code).unwrap();
    assert!(
        price > maya_vm::gas::MAX_COLD_CALL_FUEL && price <= block_cap(),
        "{price}"
    );
    let pool = Mempool::new(Arc::new(db));
    let tx = signed(
        TxKind::DeployContract(ContractDeploy { code }),
        0,
        &deployer,
    );
    assert_eq!(
        pool.insert(tx),
        Err(NodeError::GasAboveCap {
            limit: price,
            cap: maya_vm::gas::MAX_COLD_CALL_FUEL
        })
    );
}

#[test]
fn a_failed_call_spends_the_gas_it_used_not_the_gas_it_named() {
    // A call that traps on its first instruction ran almost nothing; charging
    // it its whole limit would let one transaction void the block's budget.
    // A spin at the cap first, so the trap is seen against the builder's
    // budget: charged its limit, it would leave no room for the honest call.
    let spinner = generate_signing_key().unwrap();
    let attacker = generate_signing_key().unwrap();
    let honest = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[spinner.address(), attacker.address(), honest.address()]);
    let ids = deploy_all(
        &db,
        &[&attacker, &honest, &spinner],
        &[wasm(TRAP_WAT), wasm(WRITER_WAT), wasm(FOREVER_WAT)],
    );
    let spin = call(ids[2], block_cap(), 1, &spinner);
    let trap = call(ids[0], block_cap(), 1, &attacker);
    let after = call(ids[1], block_cap() / 2, 1, &honest);
    assert!(spin_gas(block_cap()) + block_cap() + block_cap() / 2 > builder_budget());
    let kept = db
        .select_applicable(
            vec![spin, trap, after.clone()],
            BlockContext::at_height(2),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    assert_eq!(kept, vec![after]);
}

/// A module of about `len` bytes importing a name the host does not have:
/// it compiles in full, then fails validation.
fn bad_import_module_of_len(len: usize) -> Vec<u8> {
    let data = "a".repeat(len);
    wasm(&format!(
        r#"(module (import "env" "no_such_host_function" (func))
            (memory (export "memory") 1) (data "{data}")
            (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#
    ))
}

#[test]
fn a_deployment_that_fails_validation_still_spends_its_compile() {
    // Before: the compile ran inside `validate` and the gas was added only
    // after it passed, so a module with one unknown import compiled for free.
    // Seen against the builder's budget, after a spin at the cap: counted,
    // the bad compile leaves no room for the good one.
    let spinner = generate_signing_key().unwrap();
    let attacker = generate_signing_key().unwrap();
    let honest = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[spinner.address(), attacker.address(), honest.address()]);
    let forever = deploy(&db, &spinner, &wasm(FOREVER_WAT));
    let spin = call(forever, block_cap(), 1, &spinner);
    let len = usize::try_from(block_cap() * 3 / 5 / maya_vm::gas::CODE_BYTE_FUEL).unwrap();
    let bad = signed(
        TxKind::DeployContract(ContractDeploy {
            code: bad_import_module_of_len(len),
        }),
        0,
        &attacker,
    );
    let good = signed(
        TxKind::DeployContract(ContractDeploy {
            code: module_of_len(len),
        }),
        0,
        &honest,
    );
    let kept = db
        .select_applicable(
            vec![spin.clone(), bad, good.clone()],
            BlockContext::at_height(2),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    assert!(kept.is_empty(), "{} kept", kept.len());
    // Without the bad deployment the good one is tried and applies.
    let kept = db
        .select_applicable(
            vec![spin, good.clone()],
            BlockContext::at_height(2),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    assert_eq!(kept, vec![good]);
}

#[test]
fn a_later_call_to_a_contract_in_the_same_block_is_warm_by_block_order() {
    // GAS-3: the first call to a contract in a block pays for its compile and
    // later ones do not, decided by the block's order and nothing else.
    let key = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[key.address()]);
    let len = usize::try_from(block_cap() * 7 / 10 / maya_vm::gas::CODE_BYTE_FUEL).unwrap();
    let code = module_of_len(len);
    let contract = deploy(&db, &key, &code);
    let cold = block_cap() * 3 / 4;
    let warm = 2_000_000;
    assert!(maya_vm::gas::call_fuel(code.len(), true) < warm);
    assert!(maya_vm::gas::call_fuel(code.len(), false) > warm);

    db.apply_block(
        &block_of(vec![
            call(contract, cold, 1, &key),
            call(contract, warm, 2, &key),
        ]),
        BlockContext::at_height(2),
    )
    .expect("cold then warm fits");
    // The same small limit as the block's first call is a cold call, whatever
    // the node's module cache holds from the block before.
    let result = db.apply_block(
        &block_of(vec![call(contract, warm, 3, &key)]),
        BlockContext::at_height(3),
    );
    assert!(matches!(result, Err(NodeError::Vm(_))), "{result:?}");
}

// ---------------------------------------------------------------------------
// ADR-142 amendment 8: contract state bounds (CSTATE-1, CSTATE-2)
// ---------------------------------------------------------------------------

/// Writes `n` new one-byte slots per call, keyed by the caller's first four
/// bytes and the slot's index, so two callers never share a key.
fn slot_writer(n: u32) -> Vec<u8> {
    wasm(&format!(
        r#"(module
            (import "env" "storage_write" (func $w (param i32 i32 i32 i32)))
            (import "env" "caller" (func $c (param i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (local $i i32)
              (drop (call $c (i32.const 0)))
              (block $done (loop $again
                (br_if $done (i32.ge_u (local.get $i) (i32.const {n})))
                (i32.store (i32.const 4) (local.get $i))
                (call $w (i32.const 0) (i32.const 8) (i32.const 64) (i32.const 1))
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br $again)))
              (i64.const 0)))"#
    ))
}

/// Sets governed parameters straight in committed state, bounds unchecked.
fn set_parameters(db: &StateDB, values: &[(ParameterKey, u64)]) {
    let table = values.iter().fold(
        custom_l1_node::governance::record::ParameterTable::new(),
        |table, (key, value)| table.with(*key, maya_governance::Amount::from_u64(*value)),
    );
    db.raw_put_for_test(b"g:params", &table.encode())
        .expect("parameters");
}

#[test]
fn the_contract_state_caps_are_governed_with_compiled_bounds() {
    for key in [
        ParameterKey::VmMaxBlockContractWrites,
        ParameterKey::VmMaxContractStateBytes,
    ] {
        let bounds = key.bounds();
        assert!(bounds.contains(bounds.default), "{}", key.label());
        assert!(
            key.check(maya_governance::Amount::from_u64(u64::MAX))
                .is_err()
        );
        assert!(key.check(maya_governance::Amount::from_u64(0)).is_err());
    }
    // One largest module stays deployable under the lowest state ceiling.
    let largest =
        custom_l1_node::state::contract_bound::entry_size(5 + 32, maya_vm::MAX_MODULE_BYTES);
    assert!(
        ParameterKey::VmMaxContractStateBytes
            .bounds()
            .min
            .saturating_to_u64()
            >= largest
    );
}

#[test]
fn a_block_over_the_contract_write_cap_is_left_for_the_next() {
    let first = generate_signing_key().unwrap();
    let second = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[first.address(), second.address()]);
    let contract = deploy(&db, &first, &slot_writer(600));
    set_parameters(&db, &[(ParameterKey::VmMaxBlockContractWrites, 1_024)]);

    let a = call(contract, 10_000_000, 1, &first);
    let b = call(contract, 10_000_000, 0, &second);
    let result = db.apply_block(
        &block_of(vec![a.clone(), b.clone()]),
        BlockContext::at_height(2),
    );
    assert_eq!(
        result,
        Err(NodeError::BlockContractWritesExhausted {
            entries: 1_200,
            cap: 1_024
        })
    );

    let kept = db
        .select_applicable(
            vec![a.clone(), b.clone()],
            BlockContext::at_height(2),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    assert_eq!(kept, vec![a]);
    db.apply_block(&block_of(kept), BlockContext::at_height(2))
        .unwrap();
    db.apply_block(&block_of(vec![b]), BlockContext::at_height(3))
        .expect("left out, applied in the next block");
    assert_eq!(
        db.contract_state_size().unwrap(),
        db.contract_state_size_scan().unwrap()
    );
}

#[test]
fn contract_state_past_the_ceiling_is_refused_and_an_overwrite_is_not() {
    let key = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[key.address()]);
    let narrow = deploy(&db, &key, &slot_writer(10));
    let wide_code = slot_writer(11);
    db.apply_block(
        &block_of(vec![
            signed(
                TxKind::DeployContract(ContractDeploy {
                    code: wide_code.clone(),
                }),
                1,
                &key,
            ),
            call(narrow, 10_000_000, 2, &key),
        ]),
        BlockContext::at_height(2),
    )
    .unwrap();
    let wide = derive_contract_id(&key.address(), 1, &wide_code);
    let size = db.contract_state_size().unwrap();
    assert_eq!(size, db.contract_state_size_scan().unwrap());
    set_parameters(&db, &[(ParameterKey::VmMaxContractStateBytes, size)]);

    // The same ten slots again: an overwrite of the same size applies.
    db.apply_block(
        &block_of(vec![call(narrow, 10_000_000, 3, &key)]),
        BlockContext::at_height(3),
    )
    .expect("an overwrite does not grow contract state");
    // Eleven new slots: refused, in block validation and by the builder.
    let grow = call(wide, 10_000_000, 4, &key);
    let result = db.apply_block(&block_of(vec![grow.clone()]), BlockContext::at_height(4));
    assert!(
        matches!(result, Err(NodeError::ContractStateFull { cap, .. }) if cap == size),
        "{result:?}"
    );
    let kept = db
        .select_applicable(
            vec![grow],
            BlockContext::at_height(4),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    assert!(kept.is_empty());
    assert_eq!(db.contract_state_size().unwrap(), size);
}
