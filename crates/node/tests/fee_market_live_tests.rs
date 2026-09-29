//! The fee market live on a chain whose genesis configures it (ADR-029).
//!
//! `fee_market_tests.rs` pins that a chain *without* fees is untouched. This
//! file pins the other side: with `bft.fees` in genesis, an unpaid transfer is
//! refused, a paid one lands, the base fee burns to the sink, the tip stays in
//! the collector, the base fee steps with block size, and value is conserved.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::keys;
use custom_l1_node::error::NodeError;
use custom_l1_node::genesis::{Allocation, BftGenesis, FeesGenesis, GenesisConfig};
use custom_l1_node::state::db::StateDB;
use custom_l1_node::state::fees::FEE_COLLECTOR;
use custom_l1_node::state::shielded::FEE_SINK;
use tempfile::TempDir;

const BASE_FEE: u64 = 1_000;
const START: u64 = 100_000_000;

fn user() -> HybridSigningKey {
    signing_key_from_seed(&[0x31; 32]).unwrap()
}

fn chain(dir: &TempDir) -> Chain {
    let validator = keys::signing_key_from_seed(&[1; 32]).unwrap();
    let config = GenesisConfig {
        chain_id: "fee-live".to_string(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: vec![Allocation {
            address: hex::encode(user().address()),
            balance: START,
        }],
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: Vec::new(),
        security_council: None,
        bft: Some(BftGenesis {
            validators: vec![hex::encode(validator.verifying_key().to_bytes())],
            anchor_timeout_ms: 1_000,
            batch_size: 100,
            round_interval_ms: 500,
            staking: None,
            fees: Some(FeesGenesis {
                initial_base_fee: BASE_FEE,
                min_base_fee: 1,
                target_block_bytes: 100_000,
                change_denominator: 8,
            }),
        }),
    };
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    config.seed_state(&state).unwrap();
    Chain::open(
        state,
        config.genesis_block().unwrap(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap()
}

fn transfer(amount: u64, fee: Option<u64>, nonce: u64) -> Transaction {
    let mut outputs = vec![TxOutput {
        amount,
        recipient: [0x77; 32],
    }];
    if let Some(fee) = fee {
        outputs.push(TxOutput {
            amount: fee,
            recipient: FEE_COLLECTOR,
        });
    }
    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.sign(&user(, &common::test_chain())).unwrap();
    tx
}

fn balance(c: &Chain, who: &[u8; 32]) -> u64 {
    c.state().get_account(who).unwrap().balance
}

#[test]
fn an_unpaid_transfer_is_refused_and_a_paid_one_burns_its_base_fee() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir);

    let unpaid = transfer(100, None, 0);
    let err = c.candidate_block(1_000_001, vec![unpaid]).unwrap_err();
    assert!(matches!(err, NodeError::FeeTooLow { .. }), "{err:?}");

    // Size is independent of the fee amount, so measure with a placeholder.
    let size = u64::try_from(transfer(100, Some(0), 0).to_bytes().len()).unwrap();
    let required = BASE_FEE * size;
    let short = transfer(100, Some(required - 1), 0);
    assert!(matches!(
        c.candidate_block(1_000_001, vec![short]).unwrap_err(),
        NodeError::FeeTooLow { .. }
    ));

    let tip = 500;
    let paid = transfer(100, Some(required + tip), 0);
    let block = c.candidate_block(1_000_001, vec![paid]).unwrap();
    c.insert_block(block).unwrap();

    assert_eq!(balance(&c, &[0x77; 32]), 100);
    assert_eq!(balance(&c, &FEE_SINK), required, "the base fee burns");
    assert_eq!(
        balance(&c, &FEE_COLLECTOR),
        tip,
        "the tip waits for the epoch"
    );
    assert_eq!(balance(&c, &user().address()), START - 100 - required - tip);
    let total = balance(&c, &[0x77; 32])
        + balance(&c, &FEE_SINK)
        + balance(&c, &FEE_COLLECTOR)
        + balance(&c, &user().address());
    assert_eq!(total, START, "value conserved");
}

#[test]
fn the_base_fee_falls_on_empty_blocks_and_never_below_its_floor() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir);
    let mut last = c.state().committed_fees().unwrap().unwrap().base_fee;
    assert_eq!(last, BASE_FEE);
    for h in 1..=20u64 {
        let block = c.candidate_block(1_000_000 + h, Vec::new()).unwrap();
        c.insert_block(block).unwrap();
        let now = c.state().committed_fees().unwrap().unwrap().base_fee;
        assert!(now <= last && now >= 1, "base fee {now} after {last}");
        last = now;
    }
    assert!(
        last < BASE_FEE / 2,
        "twenty empty blocks left the base fee at {last}"
    );
}

/// The sizes the fee parameters are derived from (reports/18-economics.md §3a).
#[test]
mod common;

fn measured_transfer_sizes_for_the_fee_derivation() {
    use maya_crypto_pq::suite::SignatureSuite as _;
    let hybrid = transfer(1, Some(1), 0).to_bytes().len();
    let key = maya_crypto_pq::suite::MlDsa65::signing_key_from_seed(
        &maya_crypto_pq::suite::MasterSeed::from_bytes([3; 32]),
    );
    let mut v7 = Transaction::new(
        vec![],
        vec![
            TxOutput {
                amount: 1,
                recipient: [9; 32],
            },
            TxOutput {
                amount: 1,
                recipient: FEE_COLLECTOR,
            },
        ],
        0,
    );
    v7.sign_with_suite::<maya_crypto_pq::suite::MlDsa65>(&key)
        .unwrap();
    let suite = v7.to_bytes().len();
    println!(
        "fee derivation sizes: hybrid (v5, ML-DSA-65 + SLH-DSA) transfer with fee output = {hybrid} B; ML-DSA-65 suite (v7) transfer with fee output = {suite} B"
    );
    assert!(suite < hybrid);
}
