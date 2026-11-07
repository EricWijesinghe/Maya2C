//! A suite sunset committed at genesis, enforced at the mempool and by
//! consensus (ADR-082), and the legacy untagged hybrid path (M1).
//!
//! Genesis is the only seed the schedule has here, so these tests run real
//! blocks to the sunset height. The sunset is an emergency one with the shortest
//! window the network permits, `MIN_EMERGENCY_WINDOW` (17,280 blocks), so the
//! chain reaches it in one run of empty blocks. Nothing in the tests overrides a
//! lead: the schedule comes from the genesis file.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::transaction::{ChainTag, Transaction};
use custom_l1_node::crypto::hybrid::generate_signing_key;
use custom_l1_node::crypto::suites::{SuiteSunset, SuiteSunsets, verification_policy};
use custom_l1_node::genesis::{Allocation, GenesisConfig, SuiteDeprecationGenesis};
use custom_l1_node::network::mempool::Mempool;
use custom_l1_node::state::StateDB;
use maya_crypto_pq::agility::MIN_EMERGENCY_WINDOW;
use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite, SuiteId};
use tempfile::TempDir;

/// The block the sunset was announced at. The emergency window runs from here.
const ANNOUNCED: u64 = 1;
/// The first height the sunset suite no longer signs.
const SUNSET: u64 = ANNOUNCED + MIN_EMERGENCY_WINDOW;

fn config(
    suite_deprecations: Vec<SuiteDeprecationGenesis>,
    allocations: Vec<Allocation>,
) -> GenesisConfig {
    GenesisConfig {
        chain_id: "sunset-regtest".to_string(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations,
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: Vec::new(),
        suite_deprecations,
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
fn a_legacy_untagged_hybrid_frame_is_refused_once_0x30_is_sunset() {
    // M1: an untagged frame is the hybrid 0x30 and the schedule judges it.
    let chain = ChainTag::from_genesis([0x5A; 32]);
    let key = generate_signing_key().unwrap();
    let mut tx = Transaction::new(Vec::new(), Vec::new(), 0);
    tx.sign(&key, &chain).unwrap();

    let sunsets = SuiteSunsets::new(&[SuiteSunset {
        suite: SuiteId::HybridMlDsa65SlhDsa128s,
        announced_at: ANNOUNCED,
        sunset_height: SUNSET,
        emergency: true,
    }])
    .unwrap();
    let policy = verification_policy(&sunsets).unwrap();
    tx.verify_at(SUNSET - 1, &policy, &chain)
        .expect("accepted one block before the sunset");
    assert!(
        tx.verify_at(SUNSET, &policy, &chain).is_err(),
        "refused at the sunset height"
    );
}

#[test]
fn a_genesis_sunset_admits_a_v7_frame_one_block_before_and_evicts_or_refuses_it_at_its_height() {
    // (b): an emergency sunset of 0x10 from genesis. The mempool admits the frame
    // at SUNSET-1 and drops or refuses it at SUNSET.
    let dir = TempDir::new().unwrap();
    // The sender is the suite address of the key, which does not depend on the
    // chain tag, so a probe signature names the account the genesis must fund.
    let key = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([0x33; 32]));
    let mut probe = Transaction::new(Vec::new(), Vec::new(), 0);
    probe
        .sign_with_suite::<MlDsa65>(&key, &ChainTag::from_genesis([0; 32]))
        .unwrap();
    let funded = Allocation {
        address: hex::encode(probe.sender()),
        balance: primitive_types::U256::from(1_000_000u64),
    };

    let genesis = config(
        vec![SuiteDeprecationGenesis {
            suite: SuiteId::MlDsa65.to_byte(),
            announced_at: ANNOUNCED,
            sunset_height: SUNSET,
            emergency: true,
        }],
        vec![funded],
    );
    let mut chain = open(&dir, &genesis);
    let tag = ChainTag::from_genesis(chain.genesis());
    let mut tx = Transaction::new(Vec::new(), Vec::new(), 0);
    tx.sign_with_suite::<MlDsa65>(&key, &tag).unwrap();

    let pool = Mempool::new(Arc::clone(chain.state()));

    // Admission height is tip + 1. Stop at SUNSET-2 so admission is SUNSET-1.
    extend_to(&mut chain, SUNSET - 2);
    pool.insert(tx.clone())
        .expect("admitted at the last height the suite may sign");

    // Admission height is now SUNSET: the pool drops the frame it holds...
    extend_to(&mut chain, SUNSET - 1);
    assert_eq!(
        pool.revalidate().unwrap(),
        1,
        "the held frame is evicted at the sunset"
    );
    // ...and refuses it when it arrives again.
    assert!(
        pool.insert(tx).is_err(),
        "a v7 frame under a sunset suite is refused at its height"
    );
}
