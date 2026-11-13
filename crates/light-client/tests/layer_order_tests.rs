//! A stateless light node follows a chain whose genesis seeds an upgrade schedule.
//!
//! `LAYER_ORDER` listed `UpgradeSchedule` before `Stateless` while the state
//! root folds `sl:` first (`RECORD_LAYERS`). A genesis that schedules any
//! protocol upgrade writes `up:` records, so once stateless activation wrote
//! `sl:accounts-sparse` every witness the node produced was refused as "out of
//! order" by `StateWitness::decode`: the light node could not follow the chain.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::{ChainTag, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::signing_key_from_seed;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::genesis::{Allocation, GenesisConfig};
use custom_l1_node::state::proof::StateLayer;
use custom_l1_node::state::{BlockContext, StateDB, StateWitness};
use custom_l1_node::upgrade::{FeatureSet, ProtocolUpgrade};
use maya_light_client::StatelessValidator;

const CHAIN: ChainTag = ChainTag::from_genesis([0x5C; 32]);

fn context(height: u64) -> BlockContext {
    BlockContext::at_height(height).with_stateless_activation(1)
}

fn genesis(funded: [u8; 32]) -> GenesisConfig {
    GenesisConfig {
        chain_id: "layer-order".to_string(),
        timestamp: 1_756_252_800,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: vec![Allocation {
            address: hex::encode(funded),
            balance: primitive_types::U256::from(1_000_000u64),
        }],
        oracle: None,
        sealed: None,
        treasury: None,
        // The `up:` records under test: any scheduled upgrade seeds them.
        protocol_upgrades: vec![ProtocolUpgrade {
            version: 2,
            features: FeatureSet::default(),
            activation_height: 1_000_000,
        }],
        suite_deprecations: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: None,
    }
}

fn next_header(parent: &BlockHeader) -> BlockHeader {
    BlockHeader {
        version: 0,
        prev_hash: parent.id(),
        state_root: [0; 32],
        timestamp: parent.timestamp + 1,
        nonce: 0,
        difficulty_target: parent.difficulty_target,
        tx_root: [0; 32],
    }
}

#[test]
fn a_witness_from_a_state_holding_stateless_and_upgrade_records_validates() {
    let directory = tempfile::tempdir().expect("tempdir");
    let db = StateDB::open(directory.path()).expect("open");
    db.bind_chain(CHAIN).expect("bind");
    let key = signing_key_from_seed(&[3; 32]).expect("derive");
    let root = genesis(key.address()).seed_state(&db).expect("seed");

    let genesis_header = BlockHeader {
        version: 0,
        prev_hash: [0; 32],
        state_root: root,
        timestamp: 1_756_252_800,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    };
    // The activation block writes `sl:accounts-sparse`.
    let draft = Block::new(next_header(&genesis_header), vec![]);
    let anchor = BlockHeader {
        state_root: db.apply_block(&draft, context(1)).expect("activate"),
        ..draft.header
    };

    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: primitive_types::U256::from(7u64),
            recipient: [0xAB; 32],
        }],
        0,
    );
    tx.sign(&key, &CHAIN).expect("sign");
    let draft = Block::new(next_header(&anchor), vec![tx]);
    let witness = db.block_witness(&draft).expect("witness");
    let layers: Vec<StateLayer> = witness.layers.iter().map(|d| d.layer).collect();
    assert!(
        layers.contains(&StateLayer::Stateless) && layers.contains(&StateLayer::UpgradeSchedule),
        "the witness must carry both layers: {layers:?}"
    );
    let block = Block::new(
        BlockHeader {
            state_root: db.apply_block(&draft, context(2)).expect("apply"),
            ..draft.header
        },
        draft.transactions,
    );

    // Over the wire, as a light node receives it.
    let received = StateWitness::decode(&witness.encode()).expect("the witness decodes");
    let mut validator = StatelessValidator::new(&anchor, 1, CHAIN);
    validator
        .validate(&block, received)
        .expect("the light node accepts the block");
    assert_eq!(validator.height(), 2);
}
