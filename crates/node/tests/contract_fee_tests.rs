//! Contract transactions pay for their gas and their new state, and a
//! failing one pays for the gas it used and leaves the pool (Eric
//! 2026-11-24: R-2, R-3, R-1; spec 10-vm-gas GAS-8, GAS-11, GAS-13).
//!
//! Fees on (genesis `bft.fees`), applied straight to state block by block.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use primitive_types::U256;

use custom_l1_node::core::payload::{ContractCall, ContractDeploy, derive_contract_id};
use custom_l1_node::core::transaction::TxOutput;
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::crypto::validator_keys as keys;
use custom_l1_node::error::NodeError;
use custom_l1_node::genesis::{Allocation, BftGenesis, FeesGenesis, GenesisConfig};
use custom_l1_node::governance::{PendingParameter, pending_parameter_key};
use custom_l1_node::state::contract_fee::STORAGE_ENTRY_OVERHEAD;
use custom_l1_node::state::fees::FEE_COLLECTOR;
use custom_l1_node::state::shielded::FEE_SINK;
use custom_l1_node::state::{BlockContext, StateDB};
use maya_governance::params::ParameterKey;
use tempfile::TempDir;

mod common;

const BASE_FEE: u64 = 1_000;
const START: u64 = 1_000_000_000_000;
const GAS_LIMIT: u64 = 2_000_000;
const TIP: u64 = 7;

/// Writes one new key (1 byte) with a 4-byte value.
const WRITER_WAT: &str = r#"(module
    (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
    (memory (export "memory") 1)
    (func (export "invoke") (param i32) (result i64)
      (i32.store8 (i32.const 0) (i32.const 107))
      (i32.store (i32.const 16) (i32.const 1234))
      (call $write (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 4))
      (i64.const 0)))"#;

/// Writes one new key with a 4,096-byte value (`MAX_VALUE_BYTES`): more new
/// state than a 2,000,000-gas reservation pays for.
const BIG_WRITER_WAT: &str = r#"(module
    (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
    (memory (export "memory") 1)
    (func (export "invoke") (param i32) (result i64)
      (i32.store8 (i32.const 0) (i32.const 107))
      (call $write (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 4096))
      (i64.const 0)))"#;

/// Writes, then traps: the write must never land.
const WRITE_THEN_TRAP_WAT: &str = r#"(module
    (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
    (memory (export "memory") 1)
    (func (export "invoke") (param i32) (result i64)
      (i32.store8 (i32.const 0) (i32.const 107))
      (call $write (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 4))
      unreachable))"#;

fn user() -> HybridSigningKey {
    signing_key_from_seed(&[0x41; 32]).unwrap()
}

/// A second funded sender (GAS-12).
fn other() -> HybridSigningKey {
    signing_key_from_seed(&[0x42; 32]).unwrap()
}

fn default_of(key: ParameterKey) -> u64 {
    key.bounds().default.saturating_to_u64()
}

fn state(dir: &TempDir) -> StateDB {
    let validator = keys::signing_key_from_seed(&[1; 32]).unwrap();
    let config = GenesisConfig {
        chain_id: "contract-fee".to_string(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: vec![
            Allocation {
                address: hex::encode(user().address()),
                balance: U256::from(START),
            },
            Allocation {
                address: hex::encode(other().address()),
                balance: U256::from(START),
            },
        ],
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: Vec::new(),
        suite_deprecations: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: Some(BftGenesis {
            validators: vec![hex::encode(validator.verifying_key().to_bytes())],
            anchor_timeout_ms: 1_000,
            batch_size: 100,
            round_interval_ms: 500,
            mining: None,
            staking: None,
            // A target no test block reaches and a floor at the start, so the
            // base fee stays at BASE_FEE for the few blocks a test runs.
            fees: Some(FeesGenesis {
                initial_base_fee: U256::from(BASE_FEE),
                min_base_fee: U256::from(BASE_FEE),
                target_block_bytes: 100_000,
                change_denominator: 8,
            }),
        }),
    };
    let db = StateDB::open(dir.path()).unwrap();
    common::bind(&db);
    config.seed_state(&db).unwrap();
    db
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

/// `kind` from the user at `nonce`, paying `fee` to the collector.
fn paid(kind: TxKind, nonce: u64, fee: U256) -> Transaction {
    paid_by(&user(), kind, nonce, fee)
}

/// `kind` from `key` at `nonce`, paying `fee` to the collector.
fn paid_by(key: &HybridSigningKey, kind: TxKind, nonce: u64, fee: U256) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.outputs = vec![TxOutput {
        amount: fee,
        recipient: FEE_COLLECTOR,
    }];
    tx.sign(key, &common::test_chain()).unwrap();
    tx
}

/// The byte fee of `kind` at `nonce` paying `fee`: its length does not
/// depend on the amount's value.
fn byte_fee(kind: &TxKind, nonce: u64) -> U256 {
    let probe = paid(kind.clone(), nonce, U256::one());
    U256::from(BASE_FEE) * U256::from(probe.to_bytes().len())
}

fn fee_units(gas: u64) -> U256 {
    U256::from(gas.div_ceil(default_of(ParameterKey::VmGasPerFeeByte)))
}

fn storage_units(bytes: u64) -> U256 {
    U256::from(bytes) * U256::from(default_of(ParameterKey::VmStorageFeeBytesPerByte))
}

fn balance(db: &StateDB, who: &[u8; 32]) -> U256 {
    db.get_account(who).unwrap().balance
}

/// The user's deployment of `code` at `nonce`, paying exactly what GAS-11
/// and GAS-13 ask of it, and that fee.
fn deploy_tx(code: &[u8], nonce: u64) -> (Transaction, U256) {
    let kind = TxKind::DeployContract(ContractDeploy {
        code: code.to_vec(),
    });
    let cold = maya_vm::gas::cold_call_fuel(code).unwrap();
    let owed = U256::from(BASE_FEE)
        * (fee_units(cold) + storage_units(code.len() as u64 + STORAGE_ENTRY_OVERHEAD));
    let fee = byte_fee(&kind, nonce) + owed;
    (paid(kind, nonce, fee), fee)
}

/// Deploys `code` at nonce 0 in block 1, paying exactly what GAS-11 and
/// GAS-13 ask of a deployment, and returns its address.
fn deploy(db: &StateDB, code: &[u8]) -> [u8; 32] {
    let (tx, fee) = deploy_tx(code, 0);
    let sink = balance(db, &FEE_SINK);
    db.apply_block(&block_of(vec![tx]), BlockContext::at_height(1))
        .expect("a paid deployment applies");
    assert_eq!(
        balance(db, &FEE_SINK) - sink,
        fee,
        "a deployment burns its bytes, its cold price and its code as new state"
    );
    derive_contract_id(&user().address(), 0, code)
}

fn call(contract: [u8; 32]) -> TxKind {
    TxKind::CallContract(ContractCall {
        contract,
        input: Vec::new(),
        gas_limit: GAS_LIMIT,
    })
}

/// Fee a call at `GAS_LIMIT` must offer: bytes, its whole reservation and
/// `extra` (a tip, or room for new state).
fn call_fee(kind: &TxKind, nonce: u64, extra: U256) -> U256 {
    byte_fee(kind, nonce) + U256::from(BASE_FEE) * fee_units(GAS_LIMIT) + extra
}

#[test]
fn the_gas_price_makes_a_block_of_gas_cost_a_block_of_bytes() {
    // GAS-11's default: the default block gas over the block byte cap,
    // rounded down, so gas is never the cheaper way to fill a block.
    let block_gas = default_of(ParameterKey::VmMaxBlockGas);
    let bytes = custom_l1_node::state::fees::MAX_BLOCK_BYTES;
    assert_eq!(default_of(ParameterKey::VmGasPerFeeByte), block_gas / bytes);
}

#[test]
fn a_call_that_offers_only_its_bytes_is_refused_before_it_runs() {
    // R-3: before GAS-11 the byte fee bought a call's whole gas.
    let dir = TempDir::new().unwrap();
    let db = state(&dir);
    let contract = deploy(&db, &wat::parse_str(WRITER_WAT).unwrap());
    let kind = call(contract);
    let bytes_only = paid(kind.clone(), 1, byte_fee(&kind, 1));
    let refused = db.apply_block(&block_of(vec![bytes_only]), BlockContext::at_height(2));
    assert!(
        matches!(refused, Err(NodeError::FeeTooLow { .. })),
        "{refused:?}"
    );
    assert_eq!(db.get_contract_storage(&contract, b"k").unwrap(), None);
}

#[test]
fn a_call_pays_for_the_gas_it_used_and_its_new_state_and_gets_the_rest_back() {
    let dir = TempDir::new().unwrap();
    let db = state(&dir);
    let contract = deploy(&db, &wat::parse_str(WRITER_WAT).unwrap());
    let kind = call(contract);
    // The one new entry the writer stores (key 1, value 4) is paid out of
    // the reservation, with the gas; what is offered above the reservation
    // is the tip.
    let growth = 1 + 4 + STORAGE_ENTRY_OVERHEAD;
    let storage_fee = U256::from(BASE_FEE) * storage_units(growth);
    let fee = call_fee(&kind, 1, U256::from(TIP));
    let (sender, sink, collector) = (
        balance(&db, &user().address()),
        balance(&db, &FEE_SINK),
        balance(&db, &FEE_COLLECTOR),
    );
    db.apply_block(
        &block_of(vec![paid(kind.clone(), 1, fee)]),
        BlockContext::at_height(2),
    )
    .expect("a paid call applies");
    assert!(db.get_contract_storage(&contract, b"k").unwrap().is_some());

    let burned = balance(&db, &FEE_SINK) - sink;
    let gas_fee = burned - byte_fee(&kind, 1) - storage_fee;
    assert!(!gas_fee.is_zero(), "the gas it used is paid for");
    assert!(
        gas_fee < U256::from(BASE_FEE) * fee_units(GAS_LIMIT),
        "and only the gas it used, not its limit"
    );
    assert_eq!(gas_fee % U256::from(BASE_FEE), U256::zero());
    assert_eq!(
        balance(&db, &FEE_COLLECTOR) - collector,
        U256::from(TIP),
        "the tip waits for the epoch"
    );
    assert_eq!(
        sender - balance(&db, &user().address()),
        burned + U256::from(TIP),
        "the unused reservation went back to the sender"
    );
}

#[test]
fn a_call_that_cannot_pay_for_its_new_state_is_included_and_writes_nothing() {
    // GAS-13 (R-1): new state is not bought by the byte fee. Without room
    // for it the call fails like one out of gas: included (GAS-8), its gas
    // paid, its nonce moved, nothing written.
    let dir = TempDir::new().unwrap();
    let db = state(&dir);
    let contract = deploy(&db, &wat::parse_str(BIG_WRITER_WAT).unwrap());
    let kind = call(contract);
    let growth = 1 + 4_096 + STORAGE_ENTRY_OVERHEAD;
    assert!(
        storage_units(growth) > fee_units(GAS_LIMIT),
        "the fixture's new state costs more than the whole reservation"
    );
    let fee = call_fee(&kind, 1, U256::zero());
    let sink = balance(&db, &FEE_SINK);
    db.apply_block(
        &block_of(vec![paid(kind.clone(), 1, fee)]),
        BlockContext::at_height(2),
    )
    .expect("included as failed");
    assert_eq!(db.get_contract_storage(&contract, b"k").unwrap(), None);
    assert_eq!(db.get_account(&user().address()).unwrap().nonce, 2);
    assert!(
        balance(&db, &FEE_SINK) - sink > byte_fee(&kind, 1),
        "its gas is paid"
    );
}

#[test]
fn a_failing_call_pays_for_the_gas_it_used_and_leaves_the_pool() {
    // R-2 (GAS-8 with GAS-11): spam costs the spammer. The failing call is
    // included once, pays for the gas it ran, and its nonce has moved, so
    // the same transaction can never be proposed again.
    let dir = TempDir::new().unwrap();
    let db = state(&dir);
    let contract = deploy(&db, &wat::parse_str(WRITE_THEN_TRAP_WAT).unwrap());
    let kind = call(contract);
    let failing = paid(kind.clone(), 1, call_fee(&kind, 1, U256::zero()));
    let (sender, sink) = (balance(&db, &user().address()), balance(&db, &FEE_SINK));
    db.apply_block(&block_of(vec![failing.clone()]), BlockContext::at_height(2))
        .expect("a failing call no longer fails its block");

    let burned = balance(&db, &FEE_SINK) - sink;
    assert!(burned > byte_fee(&kind, 1), "the gas it ran is paid for");
    assert_eq!(
        sender - balance(&db, &user().address()),
        burned,
        "the rest came back"
    );
    assert_eq!(db.get_contract_storage(&contract, b"k").unwrap(), None);
    let again = db.apply_block(&block_of(vec![failing]), BlockContext::at_height(3));
    assert!(
        matches!(again, Err(NodeError::InvalidNonce { .. })),
        "{again:?}"
    );
}

#[test]
fn a_deployment_that_offers_only_its_bytes_is_refused() {
    // GAS-11 and GAS-13 for deployments: the cold price and the code as new
    // state are paid for, before the compile.
    let dir = TempDir::new().unwrap();
    let db = state(&dir);
    let kind = TxKind::DeployContract(ContractDeploy {
        code: wat::parse_str(WRITER_WAT).unwrap(),
    });
    let refused = db.apply_block(
        &block_of(vec![paid(kind.clone(), 0, byte_fee(&kind, 0))]),
        BlockContext::at_height(1),
    );
    assert!(
        matches!(refused, Err(NodeError::FeeTooLow { .. })),
        "{refused:?}"
    );
}

/// A loop contract that a cold call of it spends close to `target` gas on
/// (as `sealed_tests`' spinner).
fn spinner_using(target: u64) -> Vec<u8> {
    let make = |n: u64| {
        wat::parse_str(format!(
            r#"(module
                (memory (export "memory") 1)
                (func (export "invoke") (param i32) (result i64)
                  (local $i i64)
                  (loop $l
                    (local.set $i (i64.add (local.get $i) (i64.const 1)))
                    (br_if $l (i64.lt_u (local.get $i) (i64.const {n}))))
                  (i64.const 0)))"#
        ))
        .unwrap()
    };
    let vm = maya_vm::Vm::new().unwrap();
    let gas = |n| {
        vm.execute(
            &make(n),
            [0; 32],
            &[],
            u64::MAX / 4,
            maya_vm::MemoryState::at_height(1),
        )
        .outcome
        .unwrap()
        .gas_used
    };
    let (small, large) = (gas(1_000), gas(2_000));
    let per = (large - small) / 1_000;
    make(1_000 + (target - small) / per)
}

/// A call to `contract` naming `gas_limit`.
fn call_with(contract: [u8; 32], gas_limit: u64) -> TxKind {
    TxKind::CallContract(ContractCall {
        contract,
        input: Vec::new(),
        gas_limit,
    })
}

/// Bytes plus the whole gas reservation of a call (GAS-11), and a margin
/// for new state that is a tip where unused.
fn reserved_fee(key: &HybridSigningKey, kind: &TxKind, nonce: u64) -> U256 {
    let TxKind::CallContract(call) = kind else {
        unreachable!("a call")
    };
    let probe = paid_by(key, kind.clone(), nonce, U256::one());
    U256::from(BASE_FEE) * (U256::from(probe.to_bytes().len()) + fee_units(call.gas_limit))
        + U256::from(BASE_FEE) * storage_units(1_000)
}

#[test]
fn a_raised_block_cap_lets_the_sender_cap_bind_while_another_sender_gets_in() {
    // GAS-12 is a governance lever (PR #701 review): at the default caps the
    // sender cap equals the block cap, so GAS-2 refuses first and one sender
    // may still fill a block. Once the block cap is voted above it, one
    // sender's second call past its share is passed over while another
    // sender's call in the same block is included. It does not stop one
    // party splitting work across addresses: GAS-11 makes that cost the gas.
    let dir = TempDir::new().unwrap();
    let db = state(&dir);
    let sender_cap = default_of(ParameterKey::VmMaxSenderBlockGas);
    assert_eq!(sender_cap, default_of(ParameterKey::VmMaxBlockGas));
    let raised = ParameterKey::VmMaxBlockGas.bounds().max.saturating_to_u64();
    assert!(raised > sender_cap);

    let heavy_gas = sender_cap - 50_000_000;
    let spinner = deploy(&db, &spinner_using(heavy_gas));
    let writer_code = wat::parse_str(WRITER_WAT).unwrap();
    let writer = derive_contract_id(&user().address(), 1, &writer_code);
    // A passed vote's value, due at height 3: promoted by block 2.
    let due = PendingParameter {
        activation: 3,
        value: maya_dex::Amount::from_u64(raised),
    };
    db.raw_put_for_test(
        &pending_parameter_key(ParameterKey::VmMaxBlockGas.tag()),
        &due.encode(),
    )
    .unwrap();
    db.apply_block(
        &block_of(vec![deploy_tx(&writer_code, 1).0]),
        BlockContext::at_height(2),
    )
    .expect("the writer deploys and the raised cap is promoted");
    assert_eq!(
        db.parameters()
            .unwrap()
            .get(ParameterKey::VmMaxBlockGas)
            .saturating_to_u64(),
        raised
    );

    // The first call spends ~heavy_gas; the second fits the raised block
    // (GAS-2) but not what its sender has left (GAS-12).
    let heavy = call_with(spinner, heavy_gas + 10_000_000);
    let first = paid(heavy.clone(), 2, reserved_fee(&user(), &heavy, 2));
    let more = call_with(spinner, 60_000_000);
    let second = paid(more.clone(), 3, reserved_fee(&user(), &more, 3));
    let theirs = call_with(writer, 60_000_000);
    let elsewhere = paid_by(
        &other(),
        theirs.clone(),
        0,
        reserved_fee(&other(), &theirs, 0),
    );
    let context = BlockContext::at_height(3);

    let refused = db.apply_block(&block_of(vec![first.clone(), second.clone()]), context);
    assert!(
        matches!(refused, Err(NodeError::SenderGasExhausted { .. })),
        "{refused:?}"
    );
    let kept = db
        .select_applicable(
            vec![first.clone(), second, elsewhere.clone()],
            context,
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    let bytes = |txs: &[Transaction]| txs.iter().map(Transaction::to_bytes).collect::<Vec<_>>();
    assert_eq!(bytes(&kept), bytes(&[first, elsewhere]));
    db.apply_block(&block_of(kept), context)
        .expect("the selected block applies");
    assert!(db.get_contract_storage(&writer, b"k").unwrap().is_some());
}
