//! The sparse root cache on a real chain (ADR-062a): the sparse tree switched
//! on by the genesis schedule, signed transfer blocks, a reorg that reverts
//! and re-applies blocks above activation, a restart, and a damaged cache
//! caught by `Chain::open`.
//!
//! Debug builds also compare every root the cache gives with the full rebuild
//! (`StateDB::root_from`), so each block here is checked twice.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::genesis::{Allocation, GenesisConfig};
use custom_l1_node::state::StateDB;
use custom_l1_node::upgrade::{FeatureSet, ProtocolUpgrade, SUPPORTED_PROTOCOL_VERSION};
use tempfile::TempDir;

mod common;

/// The sparse tree's first height.
const ACTIVATION: u64 = 2;
/// Blocks both branches share.
const SHARED: u64 = 4;

fn sender() -> HybridSigningKey {
    signing_key_from_seed(&[0x5C; 32]).unwrap()
}

fn config() -> GenesisConfig {
    GenesisConfig {
        chain_id: "root-cache-chain".into(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: vec![Allocation {
            address: hex::encode(sender().address()),
            balance: primitive_types::U256::from(10_000_000),
        }],
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: vec![ProtocolUpgrade {
            version: SUPPORTED_PROTOCOL_VERSION,
            activation_height: ACTIVATION,
            features: FeatureSet(FeatureSet::STATELESS),
        }],
        suite_deprecations: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: None,
    }
}

/// Opens the chain in `dir`, seeding genesis state only into a fresh
/// database (invariant 26: re-seeding an evolved one overwrites it).
fn open(dir: &TempDir) -> Chain {
    let config = config();
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    if state.chain_meta().unwrap().is_none() {
        config.seed_state(&state).unwrap();
    }
    common::open_chain(
        state,
        config.genesis_block().unwrap(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap()
}

/// A transfer of 1 to each of three recipients picked by `branch` and the height.
fn transfer(nonce: u64, branch: u8, height: u64) -> Transaction {
    let outputs = (0..3_u8)
        .map(|i| TxOutput {
            amount: primitive_types::U256::from(1),
            recipient: *blake3::hash(&[&[branch, i][..], &height.to_le_bytes()].concat())
                .as_bytes(),
        })
        .collect();
    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.sign(&sender(), &common::test_chain()).unwrap();
    tx
}

/// Mines the next block on `chain`'s tip for `branch`, and returns it.
fn next(chain: &mut Chain, branch: u8) -> custom_l1_node::core::Block {
    let height = chain.height() + 1;
    let block = chain
        .candidate_block(
            1_000_000 + height * 15 + u64::from(branch),
            vec![transfer(height - 1, branch, height)],
        )
        .unwrap();
    chain.insert_block(block.clone()).unwrap();
    block
}

/// The cache, where one is kept, equals a full rebuild branch for branch,
/// and the tip's declared root is the full rebuild's.
fn assert_cache_holds(chain: &Chain) {
    let state = chain.state();
    assert!(state.verify_root_cache().unwrap(), "the cache held");
    let tip = chain.get(&chain.tip()).unwrap();
    assert_eq!(state.state_root().unwrap(), tip.header.state_root);
}

#[test]
fn blocks_a_reorg_and_a_restart_keep_the_cache_equal_to_the_full_rebuild() {
    let dir = TempDir::new().unwrap();
    let mut chain = open(&dir);
    let shadow_dir = TempDir::new().unwrap();
    let mut shadow = open(&shadow_dir);

    // A shared prefix past activation, on both nodes.
    for _ in 0..SHARED {
        let block = next(&mut chain, 0);
        shadow.insert_block(block).unwrap();
        assert_cache_holds(&chain);
    }
    assert!(chain.context_at(ACTIVATION).unwrap().stateless_active());
    let shared = chain.state().root_cache_stats();
    assert!(
        shared.built >= 1,
        "the activation block built the cache: {shared:?}"
    );
    assert!(shared.walked >= 1, "blocks after it walked it: {shared:?}");

    // Branch A on this node, three blocks.
    for _ in 0..3 {
        next(&mut chain, 0xA);
        assert_cache_holds(&chain);
    }

    // Branch B, five blocks, mined on the shadow and fed in: a reorg that
    // reverts A's blocks through the cache and applies B's through it.
    let before_reorg = chain.state().root_cache_stats();
    for _ in 0..5 {
        let block = next(&mut shadow, 0xB);
        chain.insert_block(block).unwrap();
    }
    assert_eq!(chain.tip(), shadow.tip(), "the heavier branch won");
    assert_cache_holds(&chain);
    let after_reorg = chain.state().root_cache_stats();
    assert!(
        after_reorg.walked >= before_reorg.walked + 3 + 5,
        "three reverts and five applies, all from the cache: {before_reorg:?} -> {after_reorg:?}"
    );
    assert_eq!(after_reorg.repaired, 0);
    assert_eq!(
        chain.state().state_root().unwrap(),
        shadow.state().state_root().unwrap()
    );

    // A clean restart: the open-time check passes and changes nothing.
    drop(chain);
    let mut chain = open(&dir);
    assert_eq!(chain.state().root_cache_stats().repaired, 0);
    next(&mut chain, 0xB);
    assert_cache_holds(&chain);
    assert!(chain.state().root_cache_stats().walked >= 1);
}

#[test]
fn chain_open_rebuilds_a_damaged_cache_before_the_first_block() {
    let dir = TempDir::new().unwrap();
    let mut chain = open(&dir);
    for _ in 0..SHARED {
        next(&mut chain, 0);
    }
    assert_cache_holds(&chain);

    // A branch no build would store: `smt:`, 25 bytes of key bits and seven
    // zero bytes (a canonical depth-200 prefix), the depth, and children.
    let mut stray = b"smt:".to_vec();
    stray.extend_from_slice(&[0x5A; 25]);
    stray.extend_from_slice(&[0; 7]);
    stray.push(200);
    chain.state().raw_put_for_test(&stray, &[1; 64]).unwrap();
    drop(chain);

    let mut chain = open(&dir);
    assert_eq!(
        chain.state().root_cache_stats().repaired,
        1,
        "Chain::open caught it"
    );
    assert_cache_holds(&chain);
    next(&mut chain, 0);
    assert_cache_holds(&chain);
    assert_eq!(chain.state().root_cache_stats().repaired, 1);
}
