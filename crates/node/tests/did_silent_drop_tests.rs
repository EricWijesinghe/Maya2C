//! Reproduction: `maya2c notary setup` / `stamp` on a fee-live DAG-BFT devnet.
//!
//! Reported 2026-11-10: the notary CLI's transactions were accepted by
//! `send_raw_transaction`, never reached a block, left the sender's nonce
//! where it was, and no node log said why. This file builds exactly the
//! transactions the CLI builds (`bins/maya2c-cli/src/notary.rs` on
//! `feat/notary`: empty inputs, one fee output to the collector sized at twice
//! the base fee on a signed probe, kind set after `Transaction::new`, then
//! signed), admits them through the same `Mempool::insert` the RPC handler
//! calls, and builds the block the way `consensus::bft::build_block` does —
//! `select_applicable` then `candidate_block_sealed`.
//!
//! What it finds:
//!
//! - The CLI's `RegisterDid` is valid and lands, on a plain fee-live chain and
//!   under the devnet's staking genesis alike.
//! - The CLI's stamp (`AnchorAttestation`, schema `"notary/v1"`) is admitted
//!   and then dropped at build: execution allows only lowercase, digits and
//!   hyphens in a schema, the mempool never checks it, and
//!   `StateDB::select_applicable_within` discards the error.
//! - The same admit-then-drop holds for every identity rule checked only at
//!   execution (no DID yet, DID already registered).
//!
//! `candidate_block` on the same transaction is the diagnostic: it stages
//! through the same `stage_transaction` but returns the error the builder
//! throws away.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use primitive_types::U256;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::TxKind;
use custom_l1_node::core::identity_payload::{AnchorAttestation, RegisterDid};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::validator_keys as keys;
use custom_l1_node::error::NodeError;
use custom_l1_node::genesis::{
    Allocation, BftGenesis, FeesGenesis, GenesisBond, GenesisConfig, StakingGenesis,
};
use custom_l1_node::network::Mempool;
use custom_l1_node::state::db::StateDB;
use custom_l1_node::state::fees::FEE_COLLECTOR;
use maya_identity::Did;
use tempfile::TempDir;

mod common;

/// The devnet's base fee (`xtask/src/bft_devnet.rs`: `initial_base_fee: 1`).
const BASE_FEE: u64 = 1;
const START: u64 = 1_000_000_000;
const GENESIS_TIME: u64 = 1_000_000;
/// `notary::SCHEMA` on `feat/notary`.
const SCHEMA: &str = "notary/v1";
/// A schema execution accepts.
const VALID_SCHEMA: &str = "notary-v1";

/// The devnet's numbers (`xtask/src/bft_devnet.rs`).
const DEVNET_BALANCE: u64 = 10_000_000;
const DEVNET_BOND: u64 = 100_000;
const DEVNET_EPOCH_BLOCKS: u64 = 20;
const DEVNET_VALIDATORS: u8 = 4;

fn wallet() -> HybridSigningKey {
    signing_key_from_seed(&[0x5a; 32]).unwrap()
}

/// A fee-live chain. `devnet_staking` adds what `cargo xtask bft-devnet`
/// writes on top: four validators, each bonded at genesis by the wallet's own
/// address, which is also the one funded account.
fn chain(dir: &TempDir, devnet_staking: bool) -> Chain {
    let count = if devnet_staking { DEVNET_VALIDATORS } else { 1 };
    let validators: Vec<String> = (1..=count)
        .map(|seed| {
            let key = keys::signing_key_from_seed(&[seed; 32]).unwrap();
            hex::encode(key.verifying_key().to_bytes())
        })
        .collect();
    let operator = hex::encode(wallet().address());
    let staking = devnet_staking.then(|| {
        StakingGenesis::with_bonds(
            DEVNET_EPOCH_BLOCKS,
            (0..count)
                .map(|_| GenesisBond {
                    operator: operator.clone(),
                    bond: U256::from(DEVNET_BOND),
                    commission_bps: 0,
                })
                .collect(),
        )
    });
    let balance = if devnet_staking {
        DEVNET_BALANCE
    } else {
        START
    };
    let config = GenesisConfig {
        chain_id: "maya-bft-devnet".to_string(),
        timestamp: GENESIS_TIME,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: vec![Allocation {
            address: operator,
            balance: U256::from(balance),
        }],
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: Vec::new(),
        suite_deprecations: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: Some(BftGenesis {
            validators,
            anchor_timeout_ms: 1_000,
            batch_size: 500,
            round_interval_ms: 500,
            mining: None,
            staking,
            fees: Some(FeesGenesis {
                initial_base_fee: U256::from(BASE_FEE),
                min_base_fee: U256::from(1),
                target_block_bytes: 2_621_440,
                change_denominator: 8,
            }),
        }),
    };
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    config.seed_state(&state).unwrap();
    common::open_chain(
        state,
        config.genesis_block().unwrap(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap()
}

/// `notary::submit` + `notary::fee_outputs`, line for line.
fn cli_tx(key: &HybridSigningKey, kind: TxKind, nonce: u64) -> Transaction {
    let mut outputs = vec![TxOutput {
        amount: U256::zero(),
        recipient: FEE_COLLECTOR,
    }];
    let mut probe = Transaction::new(vec![], outputs.clone(), nonce);
    probe.kind = kind.clone();
    probe.sign(key, &common::test_chain()).unwrap();
    let size = probe.to_bytes().len() as u64;
    // `guard::quote_fee` with no override: base fee x size x 2.
    outputs[0].amount = U256::from(BASE_FEE) * U256::from(size) * U256::from(2u8);

    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.kind = kind;
    tx.sign(key, &common::test_chain()).unwrap();
    tx
}

/// `notary::setup`'s payload.
fn cli_register(key: &HybridSigningKey) -> TxKind {
    let mut public_key = Vec::new();
    key.public_key().encode_into(&mut public_key);
    TxKind::RegisterDid(Box::new(RegisterDid {
        public_key,
        endpoints: vec![],
    }))
}

/// `notary::stamp`'s payload, under `schema`.
fn stamp_kind(schema: &str) -> TxKind {
    TxKind::AnchorAttestation(Box::new(AnchorAttestation {
        schema: schema.to_string(),
        root: [0xab; 32],
        expires_at: 0,
    }))
}

/// What `consensus::bft::build_block` does with an ordered list, minus the
/// sub-DAG decode: select, then seal. Returns how many the block kept.
fn build_and_insert(c: &mut Chain, txs: Vec<Transaction>) -> usize {
    let height = c.height() + 1;
    let context = c.context_at(height).unwrap();
    let target = c.next_target(&c.tip()).unwrap();
    let kept = c.state().select_applicable(txs, context, target).unwrap();
    let count = kept.len();
    let block = c
        .candidate_block_sealed(GENESIS_TIME + height, kept, height)
        .unwrap();
    c.insert_block(block).unwrap();
    count
}

fn nonce(c: &Chain) -> u64 {
    c.state().get_account(&wallet().address()).unwrap().nonce
}

/// Admits `tx` through the RPC's path, then builds a block from it.
fn admit_and_build(c: &mut Chain, pool: &Mempool, tx: &Transaction) -> usize {
    let admitted = pool.insert(tx.clone());
    assert!(matches!(admitted, Ok(true)), "admission: {admitted:?}");
    build_and_insert(c, vec![tx.clone()])
}

/// Admission refuses the transaction with the reason execution gives, and
/// nothing is pooled: the block builder would only refuse it later, silently.
fn refused_at_admission(pool: &Mempool, tx: &Transaction) -> NodeError {
    let refused = pool.insert(tx.clone());
    assert!(!pool.contains(&tx.txid()), "pooled despite refusal");
    refused.expect_err("admission must refuse it")
}

fn register_lands(devnet_staking: bool) {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, devnet_staking);
    let key = wallet();
    let pool = Mempool::new(Arc::clone(c.state()));

    let tx = cli_tx(&key, cli_register(&key), 0);
    let staged = c.candidate_block(GENESIS_TIME + 1, vec![tx.clone()]);
    assert!(staged.is_ok(), "the block refuses it: {:?}", staged.err());
    assert_eq!(admit_and_build(&mut c, &pool, &tx), 1, "dropped at build");
    assert_eq!(nonce(&c), 1);
    let did = Did::from_address(key.address());
    assert!(
        c.state().stored_did_document(&did).unwrap().is_some(),
        "the document is stored"
    );
}

#[test]
fn the_cli_register_did_lands_on_a_fee_live_chain() {
    register_lands(false);
}

#[test]
fn the_cli_register_did_lands_with_the_devnets_staking_genesis() {
    // The devnet wallet is also every genesis bond's operator; that does not
    // stop its registration.
    register_lands(true);
}

#[test]
fn the_cli_stamp_is_refused_at_admission_for_its_schema() {
    // `AnchorAttestation::decode` checks only length and UTF-8, so the schema
    // rule lives in execution (`CryptographicAttestation`: lowercase, digits and
    // hyphens). Admission now runs the same payload rules as block building.
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, true);
    let key = wallet();
    let pool = Mempool::new(Arc::clone(c.state()));
    let setup = cli_tx(&key, cli_register(&key), 0);
    assert_eq!(admit_and_build(&mut c, &pool, &setup), 1);

    let stamp = cli_tx(&key, stamp_kind(SCHEMA), 1);
    let reason = refused_at_admission(&pool, &stamp);
    assert!(
        matches!(&reason, NodeError::Decode(m) if m.contains("notary/v1") && m.contains("hyphens")),
        "{reason:?}"
    );
    assert_eq!(nonce(&c), 1, "the nonce does not move");

    // Control: the same stamp under a hyphenated schema is admitted and lands.
    let fixed = cli_tx(&key, stamp_kind(VALID_SCHEMA), 1);
    assert_eq!(admit_and_build(&mut c, &pool, &fixed), 1);
    assert_eq!(nonce(&c), 2);
}

#[test]
fn a_stamp_before_setup_is_refused_at_admission_for_the_missing_did() {
    // A valid schema, but the issuer has no document. Execution checks that
    // (`identity_exec.rs` `require_document`), so admission does too.
    let dir = TempDir::new().unwrap();
    let c = chain(&dir, true);
    let pool = Mempool::new(Arc::clone(c.state()));

    let stamp = cli_tx(&wallet(), stamp_kind(VALID_SCHEMA), 0);
    let reason = refused_at_admission(&pool, &stamp);
    assert!(
        matches!(&reason, NodeError::Decode(m) if m.contains("has no document")),
        "{reason:?}"
    );
    assert_eq!(nonce(&c), 0);
}

#[test]
fn a_second_registration_is_refused_at_admission() {
    // Re-registration is refused at execution only (`register_did`).
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, true);
    let key = wallet();
    let pool = Mempool::new(Arc::clone(c.state()));
    let setup = cli_tx(&key, cli_register(&key), 0);
    assert_eq!(admit_and_build(&mut c, &pool, &setup), 1);

    let again = cli_tx(&key, cli_register(&key), 1);
    let reason = refused_at_admission(&pool, &again);
    assert!(
        matches!(&reason, NodeError::Decode(m) if m.contains("already has a document")),
        "{reason:?}"
    );
    assert_eq!(nonce(&c), 1);
}
