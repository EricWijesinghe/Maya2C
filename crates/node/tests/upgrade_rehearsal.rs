//! Upgrade rehearsal (Master Prompt 15 §4, ADR-082).
//!
//! The schedule a block executes under is read from committed state: genesis
//! seeds its entries, and governance or the security council write more. Two
//! behaviours are rehearsed on real chains over real storage:
//!
//! - an entry naming a version this binary does not implement halts the chain
//!   at its height with `UpgradeRequired`, and the chain stays at the last
//!   block it validated;
//! - an entry this binary implements switches its features on at its height,
//!   and the chain keeps producing past it.
//!
//! One binary cannot run as two, so "a binary that does not implement the
//! version" is modelled by an entry above [`SUPPORTED_PROTOCOL_VERSION`]. That
//! is the same comparison the halt makes on every node.
//!
//! A governance-written entry needs its lead (`MIN_UPGRADE_LEAD_BLOCKS`, 30
//! days) before it can take effect, so a chain cannot reach it inside a test.
//! Those entries are exercised at the state level in
//! `crates/node/tests/upgrade_schedule_tests.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::error::NodeError;
use custom_l1_node::genesis::GenesisConfig;
use custom_l1_node::state::StateDB;
use custom_l1_node::upgrade::{FeatureSet, ProtocolUpgrade, SUPPORTED_PROTOCOL_VERSION};
use tempfile::TempDir;

const H: u64 = 20;
const HEAD: u64 = 40;

fn config(protocol_upgrades: Vec<ProtocolUpgrade>) -> GenesisConfig {
    GenesisConfig {
        chain_id: "upgrade-rehearsal".to_string(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: Vec::new(),
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades,
        suite_deprecations: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: None,
    }
}

fn open(dir: &TempDir, config: &GenesisConfig) -> Chain {
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    config.seed_state(&state).unwrap();
    let genesis = config.genesis_block().unwrap();
    Chain::open(state, genesis, ChainConfig::without_pow_verification()).unwrap()
}

/// Extends `chain` to `height` with empty blocks, validating each one.
fn extend_to(chain: &mut Chain, height: u64) {
    while chain.height() < height {
        let h = chain.height() + 1;
        let block = chain
            .candidate_block(1_000_000 + h * 15, Vec::new())
            .unwrap();
        chain.insert_block(block).unwrap();
    }
}

#[test]
fn a_version_this_binary_does_not_implement_halts_the_chain_at_its_height() {
    let scheduled = ProtocolUpgrade {
        version: SUPPORTED_PROTOCOL_VERSION + 1,
        activation_height: H,
        features: FeatureSet::default(),
    };
    let dir = TempDir::new().unwrap();
    let mut chain = open(&dir, &config(vec![scheduled]));
    extend_to(&mut chain, H - 1);

    // The block at H is built, then refused on import with the halt.
    let block = chain
        .candidate_block(1_000_000 + H * 15, Vec::new())
        .unwrap();
    let halt = chain.insert_block(block).unwrap_err();
    assert!(
        matches!(halt, NodeError::UpgradeRequired { height, version, .. }
            if height == H && version == SUPPORTED_PROTOCOL_VERSION + 1),
        "{halt}"
    );
    assert!(
        halt.to_string()
            .starts_with("upgrade required before height 20")
    );
    assert_eq!(
        chain.height(),
        H - 1,
        "the halted chain kept its last good block"
    );

    // The state it stopped on is the one the last block committed.
    let tip = chain.get(&chain.tip()).unwrap();
    assert_eq!(chain.state().state_root().unwrap(), tip.header.state_root);
}

#[test]
fn a_version_this_binary_implements_switches_its_feature_on_at_its_height() {
    let scheduled = ProtocolUpgrade {
        version: SUPPORTED_PROTOCOL_VERSION,
        activation_height: H,
        features: FeatureSet(FeatureSet::DVP),
    };
    let dir = TempDir::new().unwrap();
    let mut chain = open(&dir, &config(vec![scheduled]));
    extend_to(&mut chain, HEAD);

    assert_eq!(chain.height(), HEAD, "an implemented version never halts");
    assert!(!chain.context_at(H - 1).unwrap().dvp_active());
    assert!(chain.context_at(H).unwrap().dvp_active());
}

#[test]
fn a_chain_with_no_scheduled_entry_runs_every_height_with_features_off() {
    let dir = TempDir::new().unwrap();
    let mut chain = open(&dir, &config(Vec::new()));
    extend_to(&mut chain, HEAD);
    assert_eq!(chain.height(), HEAD);
    assert!(!chain.context_at(HEAD).unwrap().dvp_active());
}

#[test]
fn a_regtest_genesis_scheduled_at_height_3_accepts_block_2_and_halts_at_3_across_a_reopen() {
    // (a): a genesis that schedules a version this binary lacks at height 3. Block
    // 2 is accepted, block 3 is refused, and a restart from disk refuses it again.
    let scheduled = ProtocolUpgrade {
        version: SUPPORTED_PROTOCOL_VERSION + 1,
        activation_height: 3,
        features: FeatureSet::default(),
    };
    let cfg = config(vec![scheduled]);
    let dir = TempDir::new().unwrap();
    let mut chain = open(&dir, &cfg);
    extend_to(&mut chain, 2);
    assert_eq!(chain.height(), 2);

    let block3 = chain
        .candidate_block(1_000_000 + 3 * 15, Vec::new())
        .unwrap();
    let halt = chain.insert_block(block3.clone()).unwrap_err();
    assert!(
        matches!(halt, NodeError::UpgradeRequired { height: 3, .. }),
        "{halt}"
    );
    assert_eq!(chain.height(), 2);
    drop(chain);

    let mut reopened = open(&dir, &cfg);
    assert_eq!(
        reopened.height(),
        2,
        "the reopened chain resumes at block 2"
    );
    let halt = reopened.insert_block(block3).unwrap_err();
    assert!(
        matches!(halt, NodeError::UpgradeRequired { height: 3, .. }),
        "{halt}"
    );
}
