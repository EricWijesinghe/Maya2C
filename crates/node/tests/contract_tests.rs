//! Contracts deployed and invoked through L1 transactions.
//!
//! `crates/vm/tests/` covers the VM in isolation. These cover the integration: a real
//! transaction carrying a module, code persisted under consensus, and a call
//! whose storage writes land in the same atomic batch as everything else in the
//! block — or land nowhere at all.

use custom_l1_node::core::payload::{ContractCall, ContractDeploy, derive_contract_id};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::generate_signing_key;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};

use custom_l1_node::crypto::hybrid::HybridSigningKey;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// A module that writes one storage key and returns nothing.
///
/// Hand-written WAT rather than the compiled contract: these tests are about
/// the transaction path, and a module whose behaviour is visible in four lines
/// makes a failure easier to attribute.
const WRITER_WAT: &str = r#"(module
    (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
    (memory (export "memory") 1)
    (func (export "invoke") (param i32) (result i64)
      (i32.store8 (i32.const 0) (i32.const 107))
      (i32.store (i32.const 16) (i32.const 1234))
      (call $write (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 4))
      (i64.const 0)))"#;

/// A module that writes a key and then loops until its gas runs out.
const RUNAWAY_WAT: &str = r#"(module
    (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
    (memory (export "memory") 1)
    (func (export "invoke") (param i32) (result i64)
      (i32.store8 (i32.const 0) (i32.const 107))
      (i32.store (i32.const 16) (i32.const 9999))
      (call $write (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 4))
      (loop $forever (br $forever))
      (i64.const 0)))"#;

fn wasm(text: &str) -> Vec<u8> {
    wat::parse_str(text).expect("valid WAT")
}

fn open_state(funded: &[(Address, u64)]) -> (StateDB, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    for (address, balance) in funded {
        db.put_account(
            address,
            &Account {
                balance: *balance,
                nonce: 0,
            },
        )
        .expect("fund");
    }
    (db, dir)
}

fn address_of(key: &HybridSigningKey) -> Address {
    key.address()
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
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
    tx.sign(key).expect("sign");
    tx
}

// ---------------------------------------------------------------------------
// deployment
// ---------------------------------------------------------------------------

#[test]
fn deploying_a_contract_stores_its_code() {
    let deployer = generate_signing_key().expect("keygen");
    let (db, _dir) = open_state(&[(address_of(&deployer), 1_000)]);

    let code = wasm(WRITER_WAT);
    let tx = signed(
        TxKind::DeployContract(ContractDeploy { code: code.clone() }),
        0,
        &deployer,
    );

    db.apply_block(&block_of(vec![tx]), BlockContext::at_height(1))
        .expect("deploy");

    let contract = derive_contract_id(&address_of(&deployer), 0, &code);
    assert_eq!(db.get_code(&contract).expect("read"), Some(code));
}

#[test]
fn an_invalid_module_is_rejected_at_deploy_time() {
    let deployer = generate_signing_key().expect("keygen");
    let (db, _dir) = open_state(&[(address_of(&deployer), 1_000)]);

    let tx = signed(
        TxKind::DeployContract(ContractDeploy {
            code: b"this is not webassembly".to_vec(),
        }),
        0,
        &deployer,
    );

    // Rejecting here means one transaction pays for the mistake, rather than
    // every future caller discovering it.
    assert!(matches!(
        db.apply_block(&block_of(vec![tx]), BlockContext::at_height(1)),
        Err(NodeError::Vm(_))
    ));
}

#[test]
fn a_module_using_a_disabled_proposal_is_rejected() {
    let deployer = generate_signing_key().expect("keygen");
    let (db, _dir) = open_state(&[(address_of(&deployer), 1_000)]);

    // SIMD is disabled for determinism; a module using it must never deploy.
    let simd = wasm(
        r#"(module
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (drop (v128.const i32x4 1 2 3 4))
              (i64.const 0)))"#,
    );

    let tx = signed(
        TxKind::DeployContract(ContractDeploy { code: simd }),
        0,
        &deployer,
    );
    assert!(matches!(
        db.apply_block(&block_of(vec![tx]), BlockContext::at_height(1)),
        Err(NodeError::Vm(_))
    ));
}

#[test]
fn deploying_the_same_module_twice_yields_two_contracts() {
    let deployer = generate_signing_key().expect("keygen");
    let (db, _dir) = open_state(&[(address_of(&deployer), 1_000)]);
    let code = wasm(WRITER_WAT);

    for nonce in 0..2u64 {
        let tx = signed(
            TxKind::DeployContract(ContractDeploy { code: code.clone() }),
            nonce,
            &deployer,
        );
        db.apply_block(&block_of(vec![tx]), BlockContext::at_height(nonce + 1))
            .expect("deploy");
    }

    // The nonce is part of the address, so identical code deploys twice without
    // colliding.
    let first = derive_contract_id(&address_of(&deployer), 0, &code);
    let second = derive_contract_id(&address_of(&deployer), 1, &code);
    assert_ne!(first, second);
    assert!(db.get_code(&first).expect("read").is_some());
    assert!(db.get_code(&second).expect("read").is_some());
}

// ---------------------------------------------------------------------------
// invocation
// ---------------------------------------------------------------------------

/// Deploys `code` and returns its address.
fn deploy(db: &StateDB, deployer: &HybridSigningKey, code: &[u8], nonce: u64) -> [u8; 32] {
    let tx = signed(
        TxKind::DeployContract(ContractDeploy {
            code: code.to_vec(),
        }),
        nonce,
        deployer,
    );
    db.apply_block(&block_of(vec![tx]), BlockContext::at_height(1))
        .expect("deploy");
    derive_contract_id(&address_of(deployer), nonce, code)
}

#[test]
fn calling_a_contract_persists_its_storage_writes() {
    let caller = generate_signing_key().expect("keygen");
    let (db, _dir) = open_state(&[(address_of(&caller), 1_000)]);
    let contract = deploy(&db, &caller, &wasm(WRITER_WAT), 0);

    let call = signed(
        TxKind::CallContract(ContractCall {
            contract,
            input: Vec::new(),
            gas_limit: 1_000_000,
        }),
        1,
        &caller,
    );
    db.apply_block(&block_of(vec![call]), BlockContext::at_height(2))
        .expect("call");

    assert_eq!(
        db.get_contract_storage(&contract, b"k").expect("read"),
        Some(1234u32.to_le_bytes().to_vec())
    );
}

#[test]
fn calling_an_undeployed_address_is_rejected() {
    let caller = generate_signing_key().expect("keygen");
    let (db, _dir) = open_state(&[(address_of(&caller), 1_000)]);

    let call = signed(
        TxKind::CallContract(ContractCall {
            contract: [9u8; 32],
            input: Vec::new(),
            gas_limit: 1_000_000,
        }),
        0,
        &caller,
    );

    assert!(matches!(
        db.apply_block(&block_of(vec![call]), BlockContext::at_height(1)),
        Err(NodeError::UnknownContract(_))
    ));
}

#[test]
fn gas_exhaustion_writes_nothing_to_the_database() {
    let caller = generate_signing_key().expect("keygen");
    let (db, _dir) = open_state(&[(address_of(&caller), 1_000)]);
    let contract = deploy(&db, &caller, &wasm(RUNAWAY_WAT), 0);

    let call = signed(
        TxKind::CallContract(ContractCall {
            contract,
            input: Vec::new(),
            gas_limit: 200_000,
        }),
        1,
        &caller,
    );

    // The contract writes before it loops. That write must not survive.
    assert!(matches!(
        db.apply_block(&block_of(vec![call]), BlockContext::at_height(2)),
        Err(NodeError::Vm(_))
    ));

    assert_eq!(
        db.get_contract_storage(&contract, b"k").expect("read"),
        None,
        "a call that ran out of gas left a write behind"
    );
}

#[test]
fn a_failed_call_aborts_the_whole_block() {
    let caller = generate_signing_key().expect("keygen");
    let other = generate_signing_key().expect("keygen");
    let (db, _dir) = open_state(&[(address_of(&caller), 1_000), (address_of(&other), 500)]);
    let contract = deploy(&db, &caller, &wasm(RUNAWAY_WAT), 0);

    // A perfectly good transfer, in the same block as a doomed call.
    let mut transfer = Transaction::new(
        vec![],
        vec![custom_l1_node::core::TxOutput {
            amount: 100,
            recipient: [3u8; 32],
        }],
        0,
    );
    transfer.sign(&other).expect("sign");
    let call = signed(
        TxKind::CallContract(ContractCall {
            contract,
            input: Vec::new(),
            gas_limit: 200_000,
        }),
        1,
        &caller,
    );

    assert!(
        db.apply_block(&block_of(vec![transfer, call]), BlockContext::at_height(2))
            .is_err()
    );

    // Block atomicity holds across the VM boundary: the unrelated transfer is
    // discarded too.
    assert_eq!(
        db.get_account(&address_of(&other)).map(|a| a.balance),
        Ok(500)
    );
    assert_eq!(db.get_account(&[3u8; 32]).map(|a| a.balance), Ok(0));
}

#[test]
fn a_contract_sees_the_executing_block_height() {
    let caller = generate_signing_key().expect("keygen");
    let (db, _dir) = open_state(&[(address_of(&caller), 1_000)]);

    // Stores the height it observes, so the test can read it back.
    let height_writer = wasm(
        r#"(module
            (import "env" "block_height" (func $height (result i64)))
            (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (i32.store8 (i32.const 0) (i32.const 104))
              (i64.store (i32.const 16) (call $height))
              (call $write (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 8))
              (i64.const 0)))"#,
    );
    let contract = deploy(&db, &caller, &height_writer, 0);

    let call = signed(
        TxKind::CallContract(ContractCall {
            contract,
            input: Vec::new(),
            gas_limit: 1_000_000,
        }),
        1,
        &caller,
    );
    db.apply_block(&block_of(vec![call]), BlockContext::at_height(7_777))
        .expect("call");

    assert_eq!(
        db.get_contract_storage(&contract, b"h").expect("read"),
        Some(7_777u64.to_le_bytes().to_vec()),
        "the height must be the executing block's, not the chain tip's"
    );
}

#[test]
fn two_calls_in_one_block_see_each_others_writes() {
    let caller = generate_signing_key().expect("keygen");
    let (db, _dir) = open_state(&[(address_of(&caller), 1_000)]);

    // Increments a counter each call.
    let counter = wasm(
        r#"(module
            (import "env" "storage_read" (func $read (param i32 i32 i32 i32) (result i32)))
            (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              (i32.store8 (i32.const 0) (i32.const 99))
              (drop (call $read (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 4)))
              (i32.store (i32.const 16) (i32.add (i32.load (i32.const 16)) (i32.const 1)))
              (call $write (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 4))
              (i64.const 0)))"#,
    );
    let contract = deploy(&db, &caller, &counter, 0);

    let first = signed(
        TxKind::CallContract(ContractCall {
            contract,
            input: Vec::new(),
            gas_limit: 1_000_000,
        }),
        1,
        &caller,
    );
    let second = signed(
        TxKind::CallContract(ContractCall {
            contract,
            input: Vec::new(),
            gas_limit: 1_000_000,
        }),
        2,
        &caller,
    );

    db.apply_block(&block_of(vec![first, second]), BlockContext::at_height(2))
        .expect("both calls");

    // Two calls, so the counter is 2 — the second saw the first's write through
    // the overlay rather than the stale committed value.
    assert_eq!(
        db.get_contract_storage(&contract, b"c").expect("read"),
        Some(2u32.to_le_bytes().to_vec())
    );
}

#[test]
fn a_contract_call_may_not_also_carry_transfer_outputs() {
    let caller = generate_signing_key().expect("keygen");
    let (db, _dir) = open_state(&[(address_of(&caller), 1_000)]);
    let contract = deploy(&db, &caller, &wasm(WRITER_WAT), 0);

    let mut tx = Transaction::with_kind(
        TxKind::CallContract(ContractCall {
            contract,
            input: Vec::new(),
            gas_limit: 1_000_000,
        }),
        1,
    );
    tx.outputs.push(custom_l1_node::core::TxOutput {
        amount: 10,
        recipient: [4u8; 32],
    });
    tx.sign(&caller).expect("sign");
    assert!(matches!(
        db.apply_block(&block_of(vec![tx]), BlockContext::at_height(2)),
        Err(NodeError::MixedTransactionKind(_))
    ));
}

// ---------------------------------------------------------------------------
// reorgs and the state root
// ---------------------------------------------------------------------------

/// Applies `transactions` through the chain's checked, journaled path,
/// declaring the root they execute to, and returns the root.
fn apply_journaled(
    db: &StateDB,
    transactions: Vec<Transaction>,
    id: [u8; 32],
    height: u64,
) -> [u8; 32] {
    let context = BlockContext::at_height(height);
    let mut block = block_of(transactions);
    block.header.state_root = db.preview_root(&block, context).expect("preview");
    db.apply_block_journaled(&block, &id, context)
        .expect("apply")
}

#[test]
fn reverting_contract_blocks_restores_code_storage_and_the_root() {
    // Contract code and storage were missing from the undo journal: a reorg
    // left the abandoned branch's slots behind. They are now under the state
    // root too, so a stale slot would also fail the next block's root check.
    let caller = generate_signing_key().expect("keygen");
    let (db, _dir) = open_state(&[(address_of(&caller), 1_000)]);
    let code = wasm(WRITER_WAT);
    let contract = derive_contract_id(&address_of(&caller), 0, &code);

    let genesis_root = db.state_root().expect("root");
    let deploy = signed(TxKind::DeployContract(ContractDeploy { code }), 0, &caller);
    let deployed_root = apply_journaled(&db, vec![deploy], [1u8; 32], 1);
    assert_ne!(deployed_root, genesis_root, "code must move the root");

    let call = signed(
        TxKind::CallContract(ContractCall {
            contract,
            input: Vec::new(),
            gas_limit: 1_000_000,
        }),
        1,
        &caller,
    );
    let called_root = apply_journaled(&db, vec![call], [2u8; 32], 2);
    assert_ne!(
        called_root, deployed_root,
        "a storage write must move the root"
    );
    assert_eq!(
        db.uncovered_keys().expect("scan"),
        Vec::<Vec<u8>>::new(),
        "every stored key must be under the state root or declared local-only"
    );

    db.revert_block(&[2u8; 32]).expect("revert the call");
    assert_eq!(
        db.get_contract_storage(&contract, b"k").expect("read"),
        None
    );
    assert_eq!(db.state_root().expect("root"), deployed_root);

    db.revert_block(&[1u8; 32]).expect("revert the deploy");
    assert_eq!(db.get_code(&contract).expect("read"), None);
    assert_eq!(db.state_root().expect("root"), genesis_root);
}
