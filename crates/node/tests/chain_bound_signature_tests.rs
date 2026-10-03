//! Chain-bound signatures (ADR-036): every signature commits to the genesis
//! block id of one chain, so a transaction signed for one `Maya2C` chain is
//! worthless on any other.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::{ChainTag, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::crypto::suites::verification_policy;
use custom_l1_node::state::StateDB;
use custom_l1_node::state::account::Account;
use custom_l1_node::state::verified::VerifiedCache;
use tempfile::TempDir;

const CHAIN_A: ChainTag = ChainTag::from_genesis([1; 32]);
const CHAIN_B: ChainTag = ChainTag::from_genesis([2; 32]);

fn key() -> HybridSigningKey {
    signing_key_from_seed(&[42; 32]).expect("derive")
}

fn transfer(chain: &ChainTag, amount: u64) -> Transaction {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount,
            recipient: [9; 32],
        }],
        0,
    );
    tx.sign(&key(), chain).expect("sign");
    tx
}

fn genesis(timestamp: u64) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        Vec::new(),
    )
}

#[test]
fn a_signature_for_one_chain_does_not_verify_on_another() {
    let tx = transfer(&CHAIN_A, 100);
    tx.verify(&CHAIN_A).expect("valid on its own chain");
    assert!(tx.verify(&CHAIN_B).is_err());

    let policy = verification_policy();
    tx.verify_at(0, &policy, &CHAIN_A)
        .expect("valid at height 0");
    assert!(tx.verify_at(0, &policy, &CHAIN_B).is_err());
}

#[test]
fn the_wire_bytes_carry_no_tag() {
    // The tag is signed, never sent: re-parsing the frame cannot recover a
    // chain, so the receiving node's own tag decides validity.
    let tx = transfer(&CHAIN_A, 100);
    let parsed = Transaction::from_bytes(&tx.to_bytes()).expect("parse");
    assert_eq!(parsed.to_bytes(), tx.to_bytes());
    parsed.verify(&CHAIN_A).expect("still valid on A");
    assert!(parsed.verify(&CHAIN_B).is_err());
}

#[test]
fn an_unbound_state_refuses_to_verify() {
    let dir = TempDir::new().expect("dir");
    let state = StateDB::open(dir.path()).expect("open");
    let tx = transfer(&CHAIN_A, 1);
    assert!(state.verify_cached(&tx, 0, &verification_policy()).is_err());
}

#[test]
fn a_state_binds_once_and_never_to_a_second_chain() {
    let dir = TempDir::new().expect("dir");
    let state = StateDB::open(dir.path()).expect("open");
    state.bind_chain(CHAIN_A).expect("bind");
    state
        .bind_chain(CHAIN_A)
        .expect("binding the same chain again is a no-op");
    assert!(state.bind_chain(CHAIN_B).is_err());

    let policy = verification_policy();
    state
        .verify_cached(&transfer(&CHAIN_A, 1), 0, &policy)
        .expect("A's transaction");
    assert!(
        state
            .verify_cached(&transfer(&CHAIN_B, 1), 0, &policy)
            .is_err()
    );
}

#[test]
fn the_verified_cache_does_not_answer_for_another_chain() {
    let cache = VerifiedCache::default();
    let policy = verification_policy();
    let tx = transfer(&CHAIN_A, 100);

    cache
        .verify(&tx, 0, &policy, &CHAIN_A)
        .expect("verify on A");
    assert_eq!(cache.len(), 1);
    assert!(cache.verify(&tx, 0, &policy, &CHAIN_B).is_err());
    assert_eq!(cache.len(), 1, "a failed verification is not cached");
}

#[test]
fn a_chain_accepts_its_own_transactions_and_refuses_a_replay_from_another() {
    let dir = TempDir::new().expect("dir");
    let state = Arc::new(StateDB::open(dir.path()).expect("open"));
    state
        .put_account(
            &key().address(),
            &Account {
                balance: 1_000_000,
                nonce: 0,
            },
        )
        .expect("fund");
    let block0 = genesis(1_000_000);
    let own = ChainTag::from_genesis(block0.header.id());
    let mut chain =
        Chain::open(state, block0, ChainConfig::without_pow_verification()).expect("open");

    // Signed for a different genesis — say the testnet — and replayed here.
    let replay = transfer(&ChainTag::from_genesis(genesis(2_000_000).header.id()), 10);
    let candidate = chain.candidate_block(1_000_015, vec![replay]);
    let refused = candidate.map_or(true, |block| chain.insert_block(block).is_err());
    assert!(
        refused,
        "a transaction signed for another chain must not apply"
    );

    let block = chain
        .candidate_block(1_000_015, vec![transfer(&own, 10)])
        .expect("candidate");
    chain
        .insert_block(block)
        .expect("its own transaction applies");
    assert_eq!(chain.height(), 1);
}
