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
        Err(_) => Some("other"),
    }
}

#[test]
fn the_spec_vectors_hold() {
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

#[test]
fn a_call_that_fails_while_the_block_is_built_still_spends_its_gas() {
    // Before: the builder trial-ran a failing call on a copy of the overlay
    // and threw the copy away, gas included, so N failing calls of 250 M gas
    // cost every validator N x ~250 ms and counted for nothing.
    let attacker = generate_signing_key().unwrap();
    let honest = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[attacker.address(), honest.address()]);
    let ids = deploy_all(
        &db,
        &[&attacker, &honest],
        &[wasm(FOREVER_WAT), wasm(WRITER_WAT)],
    );
    let spin = call(ids[0], block_cap(), 1, &attacker);
    let after = call(ids[1], block_cap() / 2, 1, &honest);
    let kept = db
        .select_applicable(
            vec![spin, after],
            BlockContext::at_height(2),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    // The spin ran to the cap and failed; what it burned is gone from the
    // block, so the next call no longer fits and is left for a later block.
    assert!(kept.is_empty(), "{} kept", kept.len());
}

#[test]
fn a_failed_call_spends_the_gas_it_used_not_the_gas_it_named() {
    // A call that traps on its first instruction ran almost nothing; charging
    // it its whole limit would let one transaction void the block's budget.
    let attacker = generate_signing_key().unwrap();
    let honest = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[attacker.address(), honest.address()]);
    let ids = deploy_all(
        &db,
        &[&attacker, &honest],
        &[wasm(TRAP_WAT), wasm(WRITER_WAT)],
    );
    let trap = call(ids[0], block_cap(), 1, &attacker);
    let after = call(ids[1], block_cap() / 2, 1, &honest);
    let kept = db
        .select_applicable(
            vec![trap, after.clone()],
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
    let attacker = generate_signing_key().unwrap();
    let honest = generate_signing_key().unwrap();
    let (db, _dir) = open_state(&[attacker.address(), honest.address()]);
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
            vec![bad, good],
            BlockContext::at_height(1),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    assert!(kept.is_empty(), "{} kept", kept.len());
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
