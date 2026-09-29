//! Chain-bound signatures (ADR-036): transactions commit to the genesis block id
//! and cannot be replayed across chains.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::Mutex;

use custom_l1_node::core::{Block, BlockHeader, ChainTag, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::signing_key_from_seed;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::StateDB;
use tempfile::TempDir;

mod common;

fn test_chain_a() -> ChainTag {
    ChainTag::from_genesis([1; 32])
}

fn test_chain_b() -> ChainTag {
    ChainTag::from_genesis([2; 32])
}

fn signing_key() -> custom_l1_node::crypto::hybrid::HybridSigningKey {
    signing_key_from_seed(&[42; 32]).expect("derive")
}

fn signed_transfer(chain: &ChainTag, amount: u64) -> Transaction {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount,
            recipient: [9; 32],
        }],
        0,
    );
    tx.sign(&signing_key(), chain).expect("sign");
    tx
}

#[test]
fn tx_signed_for_chain_a_fails_verify_for_chain_b() {
    let policy = custom_l1_node::crypto::suites::verification_policy();
    let tx_for_a = signed_transfer(&test_chain_a(), 100);

    // Should verify on chain A
    assert!(tx_for_a.verify_at(0, &policy, &test_chain_a()).is_ok());

    // Should fail on chain B
    assert!(tx_for_a.verify_at(0, &policy, &test_chain_b()).is_err());
}

#[test]
fn statedb_never_bound_refuses_verification() {
    let dir = TempDir::new().expect("temp dir");
    let state = Arc::new(StateDB::open(dir.path()).expect("open"));

    // Trying to get chain tag before binding should fail
    assert!(state.get_chain_tag().is_err());

    // Trying to verify_cached before binding should fail
    let policy = custom_l1_node::crypto::suites::verification_policy();
    let tx = signed_transfer(&test_chain_a(), 100);
    assert!(state.verify_cached(&tx, 0, &policy).is_err());
}

#[test]
fn binding_two_different_tags_is_refused() {
    let dir = TempDir::new().expect("temp dir");
    let state = Arc::new(StateDB::open(dir.path()).expect("open"));

    // Bind to chain A
    assert!(state.bind_chain(test_chain_a()).is_ok());

    // Try to bind to chain B — should fail
    assert!(state.bind_chain(test_chain_b()).is_err());
}

#[test]
fn binding_same_tag_twice_is_ok() {
    let dir = TempDir::new().expect("temp dir");
    let state = Arc::new(StateDB::open(dir.path()).expect("open"));

    // Bind to chain A
    assert!(state.bind_chain(test_chain_a()).is_ok());

    // Bind to chain A again — should succeed (idempotent)
    assert!(state.bind_chain(test_chain_a()).is_ok());

    // Verify we're still bound to A
    assert_eq!(state.get_chain_tag().unwrap(), &test_chain_a());
}

#[test]
fn verified_cache_does_not_serve_across_tags() {
    let cache = custom_l1_node::state::verified::VerifiedCache::default();
    let policy = custom_l1_node::crypto::suites::verification_policy();

    let tx = signed_transfer(&test_chain_a(), 100);

    // Verify on chain A and cache it
    assert!(cache.verify(&tx, 0, &policy, &test_chain_a()).is_ok());
    assert_eq!(cache.len(), 1);

    // The same transaction should not hit the cache on chain B
    // (it will fail verification instead)
    assert!(cache.verify(&tx, 0, &policy, &test_chain_b()).is_err());
    assert_eq!(cache.len(), 1, "failed verifications are not cached");

    // But a hit on chain A still works
    assert!(cache.verify(&tx, 0, &policy, &test_chain_a()).is_ok());
    assert_eq!(cache.len(), 1, "repeated verification does not grow cache");
}

#[test]
fn txid_is_constant_across_chains() {
    let policy = custom_l1_node::crypto::suites::verification_policy();
    let tx_for_a = signed_transfer(&test_chain_a(), 100);
    let tx_for_b = signed_transfer(&test_chain_b(), 100);

    // Same source and amount, different chains for signing
    // txid should be the same because txid does not include the chain tag
    assert_eq!(tx_for_a.id(), tx_for_b.id());

    // But they verify on different chains only
    assert!(tx_for_a.verify_at(0, &policy, &test_chain_a()).is_ok());
    assert!(tx_for_a.verify_at(0, &policy, &test_chain_b()).is_err());

    assert!(tx_for_b.verify_at(0, &policy, &test_chain_b()).is_ok());
    assert!(tx_for_b.verify_at(0, &policy, &test_chain_a()).is_err());
}

#[test]
fn txid_changes_when_transfer_amount_changes() {
    let tx1 = signed_transfer(&test_chain_a(), 100);
    let tx2 = signed_transfer(&test_chain_a(), 200);

    // Different amounts should produce different txids
    assert_ne!(tx1.id(), tx2.id());
}

#[test]
fn end_to_end_chain_open_and_apply() {
    let dir = TempDir::new().expect("temp dir");
    let state = Arc::new(StateDB::open(dir.path()).expect("open"));

    // Create a genesis block with a fixed id
    let genesis = {
        let header = BlockHeader {
            height: 0,
            prev_hash: [0; 32],
            merkle_root: [0; 32],
            timestamp: 0,
            difficulty_target: [0xff; 32],
            nonce: 0,
        };
        Block::new(header, vec![])
    };

    let genesis_id = genesis.header.id();
    let chain_tag = ChainTag::from_genesis(genesis_id);

    // Bind the state to this chain
    state.bind_chain(chain_tag.clone()).expect("bind");

    // Verify that we can verify a transaction signed for this chain
    let tx = signed_transfer(&chain_tag, 50);
    let policy = custom_l1_node::crypto::suites::verification_policy();
    assert!(state.verify_cached(&tx, 0, &policy).is_ok());

    // Verify that a transaction signed for a different chain fails
    let wrong_chain = ChainTag::from_genesis([99; 32]);
    let tx_wrong = signed_transfer(&wrong_chain, 50);
    assert!(state.verify_cached(&tx_wrong, 0, &policy).is_err());
}
