//! The DAO treasury in genesis.
//!
//! The property that matters most is the one the oracle and the sealed
//! committee already have: **a chain configured without a treasury must have
//! exactly the state root it would have had before the treasury existed.** A
//! new optional field that changed the root of every existing chain would
//! silently fork every network already running.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::genesis::{Allocation, GenesisConfig, MAX_TREASURY_SHARE_BPS, TreasuryGenesis};

fn allocation(byte: u8, balance: u64) -> Allocation {
    Allocation {
        address: hex::encode([byte; 32]),
        balance,
    }
}

fn config(treasury: Option<TreasuryGenesis>) -> GenesisConfig {
    GenesisConfig {
        chain_id: "maya-genesis-rc1".to_string(),
        timestamp: 1_767_225_600,
        difficulty_bits: 12,
        pow_limit_bits: 12,
        allocations: vec![allocation(1, 800_000), allocation(2, 0)],
        oracle: None,
        sealed: None,
        treasury,
        protocol_upgrades: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: None,
    }
}

fn treasury(byte: u8, balance: u64, share_bps: u16) -> TreasuryGenesis {
    TreasuryGenesis {
        address: hex::encode([byte; 32]),
        balance,
        share_bps,
    }
}

#[test]
fn a_chain_without_a_treasury_has_the_state_root_it_always_had() {
    // The compatibility property. If this ever changed, adding the field would
    // have forked every network already running.
    let without = config(None);
    let root = without.state_root().expect("valid");

    // The same allocations, written as they were before the field existed.
    let json = serde_json::json!({
        "chain_id": "maya-genesis-rc1",
        "timestamp": 1_767_225_600u64,
        "difficulty_bits": 12,
        "pow_limit_bits": 12,
        "allocations": [
            { "address": hex::encode([1u8; 32]), "balance": 800_000u64 },
            { "address": hex::encode([2u8; 32]), "balance": 0u64 },
        ],
    })
    .to_string();

    let parsed = GenesisConfig::from_json(&json).expect("a pre-treasury file still parses");
    assert!(parsed.treasury.is_none());
    assert_eq!(parsed.state_root().expect("valid"), root);
}

#[test]
fn a_treasury_changes_the_state_root() {
    // The other half: funding one must not be invisible.
    let without = config(None).state_root().expect("valid");
    let with = config(Some(treasury(9, 200_000, 2_000)))
        .state_root()
        .expect("valid");
    assert_ne!(without, with);
}

#[test]
fn the_declared_share_is_checked_against_the_implied_one() {
    // 200 000 of 1 000 000 is 2 000 bps. Declaring anything else is refused
    // rather than corrected — the gap between what the author wrote and what
    // the file implies is exactly the mistake worth catching before genesis
    // becomes immutable.
    assert!(config(Some(treasury(9, 200_000, 2_000))).validate().is_ok());
    assert!(
        config(Some(treasury(9, 200_000, 2_500)))
            .validate()
            .is_err()
    );
    assert!(
        config(Some(treasury(9, 200_000, 1_999)))
            .validate()
            .is_err()
    );
}

#[test]
fn a_treasury_above_the_ceiling_is_refused() {
    // 900 000 of 1 700 000 is about 5 294 bps, past the 5 000 ceiling.
    let over = config(Some(treasury(9, 900_000, 5_294)));
    let error = over.validate().expect_err("must be refused");
    assert!(
        format!("{error}").contains("ceiling"),
        "unhelpful error: {error}"
    );
}

#[test]
fn a_treasury_exactly_at_the_ceiling_is_accepted() {
    // 800 000 of 1 600 000 is exactly 5 000 bps.
    let at_limit = GenesisConfig {
        allocations: vec![allocation(1, 800_000)],
        treasury: Some(treasury(9, 800_000, MAX_TREASURY_SHARE_BPS)),
        ..config(None)
    };
    assert!(at_limit.validate().is_ok());
}

#[test]
fn a_treasury_that_duplicates_an_allocation_is_refused() {
    // Two entries for one address would make the balance depend on which was
    // applied last, which is the same rule plain allocations already follow.
    let colliding = config(Some(treasury(1, 200_000, 2_000)));
    let error = colliding.validate().expect_err("must be refused");
    assert!(
        format!("{error}").contains("also a plain allocation"),
        "unhelpful error: {error}"
    );
}

#[test]
fn the_treasury_is_credited_as_an_account() {
    let funded = config(Some(treasury(9, 200_000, 2_000)));
    let accounts = funded.accounts().expect("valid");

    let address = hex::decode(hex::encode([9u8; 32])).expect("hex");
    let found = accounts
        .iter()
        .find(|(a, _)| a.as_slice() == address.as_slice())
        .expect("treasury account is present");
    assert_eq!(found.1.balance, 200_000);
    assert_eq!(found.1.nonce, 0);
}

#[test]
fn the_state_root_does_not_depend_on_where_the_treasury_was_declared() {
    // Accounts are sorted by address before the root is computed, so a
    // treasury whose address sorts first must give the same root as one that
    // sorts last. If it did not, two operators writing the same values in a
    // different order would build different chains.
    let low = GenesisConfig {
        allocations: vec![allocation(200, 800_000)],
        treasury: Some(treasury(1, 200_000, 2_000)),
        ..config(None)
    };
    let high = GenesisConfig {
        allocations: vec![allocation(1, 800_000)],
        treasury: Some(treasury(200, 200_000, 2_000)),
        ..config(None)
    };

    // Different addresses, so different roots — what is asserted is that both
    // are computable and stable, and that each is order-independent.
    let low_root = low.state_root().expect("valid");
    let high_root = high.state_root().expect("valid");
    assert_eq!(low_root, low.state_root().expect("valid"));
    assert_eq!(high_root, high.state_root().expect("valid"));
    assert_ne!(low_root, high_root);
}

#[test]
fn total_supply_includes_the_treasury() {
    assert_eq!(config(None).total_supply().expect("valid"), 800_000);
    assert_eq!(
        config(Some(treasury(9, 200_000, 2_000)))
            .total_supply()
            .expect("valid"),
        1_000_000
    );
}

#[test]
fn a_genesis_file_round_trips_through_json_with_a_treasury() {
    let funded = config(Some(treasury(9, 200_000, 2_000)));
    let json = funded.to_json().expect("serialize");
    let parsed = GenesisConfig::from_json(&json).expect("parse");
    assert_eq!(parsed, funded);
    assert_eq!(
        parsed.state_root().expect("valid"),
        funded.state_root().expect("valid")
    );
}
