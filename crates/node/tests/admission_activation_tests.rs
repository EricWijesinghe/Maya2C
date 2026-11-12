//! Admission judges a shielded transaction at the chain's own activation
//! height, not the compiled default (ADR-099).
//!
//! The executor reads `ChainConfig::shielded_activation`, which the genesis
//! sets. The mempool used to check the compiled `SHIELDED_ACTIVATION_HEIGHT`,
//! so a pool on a chain whose pool opens later admitted a joinsplit the block
//! builder then dropped.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use primitive_types::U256;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::TxKind;
use custom_l1_node::core::mining_payload::MiningShare;
use custom_l1_node::core::payload::ShieldedJoinSplit;
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::validator_keys as keys;
use custom_l1_node::error::NodeError;
use custom_l1_node::genesis::{Allocation, BftGenesis, GenesisConfig};
use custom_l1_node::network::Mempool;
use custom_l1_node::state::db::StateDB;
use tempfile::TempDir;

mod common;

const GENESIS_TIME: u64 = 1_000_000;
const START: u64 = 1_000_000_000;
/// The genesis's shielded activation: the pool opens at this height.
const ACTIVATION: u64 = 100;
/// The height of the transaction under test. Admission judges at the tip's
/// successor, so the chain is grown to height 4 first.
const TX_HEIGHT: u64 = 5;

fn wallet() -> HybridSigningKey {
    signing_key_from_seed(&[0x6b; 32]).unwrap()
}

/// A single-validator DAG-BFT genesis with the pool opening at `ACTIVATION`.
fn genesis() -> GenesisConfig {
    let validator = hex::encode(
        keys::signing_key_from_seed(&[1; 32])
            .unwrap()
            .verifying_key()
            .to_bytes(),
    );
    GenesisConfig {
        chain_id: "maya-activation-test".to_string(),
        timestamp: GENESIS_TIME,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: vec![Allocation {
            address: hex::encode(wallet().address()),
            balance: U256::from(START),
        }],
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: Vec::new(),
        suite_deprecations: Vec::new(),
        security_council: None,
        shielded_activation_height: Some(ACTIVATION),
        bft: Some(BftGenesis {
            validators: vec![validator],
            anchor_timeout_ms: 1_000,
            batch_size: 500,
            round_interval_ms: 500,
            mining: None,
            staking: None,
            fees: None,
        }),
    }
}

fn chain(dir: &TempDir, config: &GenesisConfig) -> Chain {
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    config.seed_state(&state).unwrap();
    let chain_config: ChainConfig = config.chain_config().unwrap();
    common::open_chain(state, config.genesis_block().unwrap(), chain_config).unwrap()
}

/// Grows `c` by empty blocks until its tip is `height`.
fn grow_to(c: &mut Chain, height: u64) {
    while c.height() < height {
        let next = c.height() + 1;
        let block = c
            .candidate_block_sealed(GENESIS_TIME + next, Vec::new(), next)
            .unwrap();
        c.insert_block(block).unwrap();
    }
}

/// A joinsplit that is never proved: admission refuses it on the activation
/// rule before any proof is checked.
fn shielded_tx(nonce: u64) -> Transaction {
    let joinsplit = ShieldedJoinSplit {
        anchor: [0; 32],
        nullifiers: [[1; 32], [2; 32]],
        commitments: [[3; 32], [4; 32]],
        public_in: 0,
        public_out: 0,
        fee: 0,
        recipient: [0; 32],
        proof: Vec::new(),
    };
    let mut tx = Transaction::with_kind(TxKind::Shielded(Box::new(joinsplit)), nonce);
    tx.sign(&wallet(), &common::test_chain()).unwrap();
    tx
}

#[test]
fn a_shielded_transaction_is_refused_at_admission_before_the_chains_activation() {
    let dir = TempDir::new().unwrap();
    let config = genesis();
    let mut c = chain(&dir, &config);
    grow_to(&mut c, TX_HEIGHT - 1);
    assert_eq!(c.height() + 1, TX_HEIGHT);

    // The pool takes the activation the genesis resolved, as the node wires it.
    let pool = Mempool::new(Arc::clone(c.state()));
    pool.set_shielded_activation(config.shielded_activation());

    let refused = pool.insert(shielded_tx(0));
    assert!(
        matches!(refused, Err(NodeError::ShieldedInactive { activation, .. }) if activation == ACTIVATION),
        "admission must refuse with the chain's activation: {refused:?}"
    );
    assert!(
        !pool.contains(&shielded_tx(0).txid()),
        "pooled despite refusal"
    );
}

/// A fee-free share that carries an output is not a fee-free share: admission
/// runs the executor's payload rules on it, and this chain (which does not
/// mine) refuses it there, with the reason.
#[test]
fn a_mining_share_that_carries_outputs_is_judged_by_the_payload_rules_at_admission() {
    let dir = TempDir::new().unwrap();
    let c = chain(&dir, &genesis());
    let pool = Mempool::new(Arc::clone(c.state()));

    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: U256::one(),
            recipient: [9; 32],
        }],
        0,
    );
    tx.kind = TxKind::MiningShare(MiningShare { epoch: 1, nonce: 0 });
    tx.sign(&wallet(), &common::test_chain()).unwrap();

    let refused = pool.insert(tx.clone());
    assert!(
        matches!(
            &refused,
            Err(NodeError::MixedTransactionKind("mining_share"))
        ),
        "{refused:?}"
    );
    assert!(!pool.contains(&tx.txid()), "pooled despite refusal");
}
