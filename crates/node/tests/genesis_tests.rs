//! Genesis configuration tests.
//!
//! The property that matters for a multi-node deployment is reproducibility:
//! the same config must yield the same genesis block and state root on every
//! machine, regardless of how the JSON happens to be ordered or formatted.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::{Chain, ChainConfig};
use custom_l1_node::genesis::{Allocation, GenesisConfig};
use custom_l1_node::state::{Account, StateDB};

use tempfile::TempDir;

fn allocation(byte: u8, balance: u64) -> Allocation {
    Allocation {
        address: hex::encode([byte; 32]),
        balance,
    }
}

fn config() -> GenesisConfig {
    GenesisConfig {
        chain_id: "l1-testnet-1".to_string(),
        timestamp: 1_756_252_800,
        difficulty_bits: 12,
        pow_limit_bits: 12,
        allocations: vec![allocation(1, 1_000_000), allocation(2, 500_000)],
        // No oracle: these tests pin the state root a network without one has,
        // which is exactly the property the oracle layer must not disturb.
        oracle: None,
        // Nor a sealed-mempool committee, for the same reason: a chain without
        // one must have the state root it had before the subsystem existed.
        sealed: None,
        // Nor a treasury. Same reason again, and it is the reason these tests
        // are worth having: the state root pinned below is the one a chain
        // configured without any of these three has, and every one of them
        // must be able to be absent without disturbing it.
        treasury: None,
        protocol_upgrades: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: None,
    }
}

fn open_state() -> (Arc<StateDB>, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open state");
    (Arc::new(db), dir)
}

// ---------------------------------------------------------------------------
// reproducibility
// ---------------------------------------------------------------------------

#[test]
fn allocation_order_does_not_affect_the_genesis_block() {
    let forward = config();
    let mut reversed = config();
    reversed.allocations.reverse();

    // Operators edit this file by hand; ordering must not matter.
    assert_eq!(
        forward.state_root().expect("root"),
        reversed.state_root().expect("root")
    );
    assert_eq!(
        forward.genesis_block().expect("block").header.id(),
        reversed.genesis_block().expect("block").header.id()
    );
}

#[test]
fn json_round_trip_preserves_the_genesis_block() {
    let original = config();
    let json = original.to_json().expect("serialize");
    let parsed = GenesisConfig::from_json(&json).expect("parse");

    assert_eq!(parsed, original);
    assert_eq!(
        parsed.genesis_block().expect("block").header.id(),
        original.genesis_block().expect("block").header.id()
    );
}

#[test]
fn different_chain_ids_produce_different_genesis_blocks() {
    let first = config();
    let mut second = config();
    second.chain_id = "l1-testnet-2".to_string();

    // Identical allocations and timestamp; only the chain id differs. The two
    // networks must not share a genesis block.
    assert_eq!(
        first.state_root().expect("root"),
        second.state_root().expect("root"),
        "state is identical"
    );
    assert_ne!(
        first.genesis_block().expect("block").header.id(),
        second.genesis_block().expect("block").header.id(),
        "chain id must be bound into the genesis hash"
    );
}

#[test]
fn the_shielded_activation_is_bound_into_the_genesis_only_when_named() {
    // ADR-037. Absent, the id is what it was before the field existed, so
    // the running testnet keeps its genesis; named, it is committed, so two
    // operators who disagree about the pool disagree about block zero.
    let id = |c: &GenesisConfig| c.genesis_block().expect("block").header.id();
    let unnamed = config();
    let mut zero = config();
    zero.shielded_activation_height = Some(0);
    let mut never = config();
    never.shielded_activation_height = Some(u64::MAX);

    assert_eq!(unnamed.shielded_activation(), 0);
    assert_eq!(never.shielded_activation(), u64::MAX);
    assert_ne!(id(&unnamed), id(&never));
    assert_ne!(id(&zero), id(&never));
    assert_ne!(
        id(&unnamed),
        id(&zero),
        "naming it at all is a different genesis"
    );

    // And it survives the JSON a genesis ceremony writes.
    let json = serde_json::to_string(&never).expect("json");
    let parsed: GenesisConfig = serde_json::from_str(&json).expect("parse");
    assert_eq!(id(&parsed), id(&never));
    assert!(
        !serde_json::to_string(&unnamed)
            .expect("json")
            .contains("shielded")
    );
}

#[test]
fn changing_an_allocation_changes_the_genesis_block() {
    let base = config();
    let mut altered = config();
    altered.allocations[0].balance += 1;

    assert_ne!(
        base.state_root().expect("root"),
        altered.state_root().expect("root")
    );
    assert_ne!(
        base.genesis_block().expect("block").header.id(),
        altered.genesis_block().expect("block").header.id()
    );
}

// ---------------------------------------------------------------------------
// validation
// ---------------------------------------------------------------------------

#[test]
fn a_difficulty_floor_harder_than_genesis_is_rejected() {
    let mut invalid = config();
    // A floor of 20 bits with a 12-bit start would make genesis itself violate
    // the network's own rules.
    invalid.pow_limit_bits = 20;
    assert!(invalid.validate().is_err());
}

#[test]
fn a_dag_bft_genesis_with_any_difficulty_is_rejected() {
    // Each DAG-BFT block adds 2^difficulty_bits of work and nothing verifies
    // it, so a hard target only brings total work closer to saturation, the
    // failure that halted maya-testnet-1 (ADR-035).
    let bft = serde_json::from_value(serde_json::json!({ "validators": [] })).unwrap();
    let mut invalid = config();
    invalid.bft = Some(bft);
    let error = invalid
        .validate()
        .expect_err("12-bit DAG-BFT genesis accepted");
    assert!(error.to_string().contains("difficulty_bits"), "{error}");

    // At zero the difficulty rule has nothing to say; any other complaint
    // (here, the empty committee) is not about difficulty.
    invalid.difficulty_bits = 0;
    invalid.pow_limit_bits = 0;
    if let Err(other) = invalid.validate() {
        assert!(!other.to_string().contains("difficulty_bits"), "{other}");
    }
}

#[test]
fn duplicate_allocations_are_rejected() {
    let mut invalid = config();
    invalid.allocations.push(allocation(1, 42));
    // Two entries for one address would make the balance order-dependent.
    assert!(invalid.validate().is_err());
}

#[test]
fn a_malformed_address_is_rejected() {
    let mut invalid = config();
    invalid.allocations[0].address = "not-hex".to_string();
    assert!(invalid.validate().is_err());

    let mut short = config();
    short.allocations[0].address = "aabb".to_string();
    assert!(short.validate().is_err());
}

#[test]
fn an_empty_chain_id_is_rejected() {
    let mut invalid = config();
    invalid.chain_id = String::new();
    assert!(invalid.validate().is_err());
}

#[test]
fn an_overflowing_total_allocation_is_rejected() {
    let mut invalid = config();
    invalid.allocations = vec![allocation(1, u64::MAX), allocation(2, 1)];
    assert!(invalid.validate().is_err());
}

#[test]
fn malformed_json_is_rejected() {
    assert!(GenesisConfig::from_json("{").is_err());
    assert!(GenesisConfig::from_json("{}").is_err());
}

// ---------------------------------------------------------------------------
// seeding
// ---------------------------------------------------------------------------

#[test]
fn seeding_writes_allocations_and_matches_the_declared_root() {
    let (state, _dir) = open_state();
    let config = config();

    let root = config.seed_state(&state).expect("seed");

    assert_eq!(root, config.state_root().expect("root"));
    assert_eq!(
        state.get_account(&[1u8; 32]),
        Ok(Account {
            balance: 1_000_000,
            nonce: 0
        })
    );
    assert_eq!(
        state.get_account(&[2u8; 32]),
        Ok(Account {
            balance: 500_000,
            nonce: 0
        })
    );
}

#[test]
fn seeding_is_idempotent_across_restarts() {
    let (state, _dir) = open_state();
    let config = config();

    let first = config.seed_state(&state).expect("first seed");
    // A node restart re-runs seeding; it must not double-credit anyone.
    let second = config.seed_state(&state).expect("second seed");

    assert_eq!(first, second);
    assert_eq!(
        state.get_account(&[1u8; 32]).map(|a| a.balance),
        Ok(1_000_000)
    );
}

#[test]
fn an_empty_allocation_set_is_valid() {
    let mut empty = config();
    empty.allocations.clear();

    assert!(empty.validate().is_ok());
    assert_eq!(empty.state_root().expect("root"), [0u8; 32]);
}

// ---------------------------------------------------------------------------
// integration with the chain
// ---------------------------------------------------------------------------

#[test]
fn a_chain_built_from_genesis_starts_at_height_zero() {
    let (state, _dir) = open_state();
    let config = config();
    config.seed_state(&state).expect("seed");

    let genesis_block = config.genesis_block().expect("block");
    let genesis_id = genesis_block.header.id();

    let chain = Chain::open(
        Arc::clone(&state),
        genesis_block,
        ChainConfig::with_pow_limit(config.pow_limit()),
    )
    .expect("open chain");

    assert_eq!(chain.height(), 0);
    assert_eq!(chain.tip(), genesis_id);
    assert_eq!(chain.genesis(), genesis_id);

    // The header commits to the state actually on disk.
    assert_eq!(
        chain.get(&genesis_id).expect("record").header.state_root,
        state.state_root().expect("root")
    );
}

#[test]
fn the_next_target_after_genesis_is_the_genesis_target() {
    let (state, _dir) = open_state();
    let config = config();
    config.seed_state(&state).expect("seed");

    let genesis_block = config.genesis_block().expect("block");
    let expected = genesis_block.header.difficulty_target;
    let chain = Chain::open(
        state,
        genesis_block,
        ChainConfig::with_pow_limit(config.pow_limit()),
    )
    .expect("open chain");

    // Difficulty is inherited until the first retarget height.
    assert_eq!(chain.next_target(&chain.tip()), Ok(expected));
}

#[test]
fn a_chain_runs_every_block_under_its_genesis_shielded_activation() {
    use custom_l1_node::consensus::chain::{Chain, ChainConfig};

    let mut never = config();
    never.shielded_activation_height = Some(u64::MAX);
    let (state, _dir) = open_state();
    never.seed_state(&state).expect("seed");
    let chain = Chain::open(
        state,
        never.genesis_block().expect("block"),
        ChainConfig::without_pow_verification()
            .with_shielded_activation(never.shielded_activation()),
    )
    .expect("open");
    for height in [1, 1_000, u64::MAX - 1] {
        assert!(
            !chain.context_at(height).shielded_active(),
            "height {height}"
        );
    }
}

#[test]
fn a_bft_genesis_keeps_the_fixed_target_and_its_shielded_activation() {
    // The node and the CLI both take their rules from `chain_config`. A merge
    // once nearly replaced ADR-035's fixed target with plain
    // `without_pow_verification` there, which would have brought back the
    // total-work saturation that halted maya-testnet-1 at 12,530.
    let mut bft = config();
    bft.bft = Some(custom_l1_node::genesis::BftGenesis::with_validators(Vec::new()));
    bft.shielded_activation_height = Some(u64::MAX);
    let rules = bft.chain_config().expect("rules");
    assert!(rules.fixed_target, "DAG-BFT must keep the genesis target");
    assert!(!rules.verify_pow);
    assert_eq!(rules.shielded_activation, u64::MAX);

    let pow = config().chain_config().expect("rules");
    assert!(!pow.fixed_target && pow.verify_pow);
}
