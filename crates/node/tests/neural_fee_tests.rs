//! The neural base-fee gain across its two halves.
//!
//! `src/neural_gas/` reads blocks and must not name `maya-fee-market`
//! (`tests/fee_market_tests.rs` enforces that). `crates/fee-market/src/model/` holds
//! the network. Nothing but this file joins them, so this file is where their
//! shared constants and feature order are pinned — the arrangement
//! `tests/custody_parity_tests.rs` uses for a duplicated derivation.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::{Block, BlockHeader, ContractCall, Transaction, TxKind, TxOutput};
use custom_l1_node::crypto::hybrid::signing_key_from_seed;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::neural_gas::{
    self, BlockFeatures, FEATURE_COUNT, FEATURE_FRAC_BITS, FEATURE_LIMIT, FEATURE_ONE, extract,
};

use maya_blockgraph::shard_of;
use maya_fee_market::model::{self, Feature};
use maya_fee_market::{Features, FeeRule, MODEL_V1, Model, next_base_fee_by_rule};

const TARGET: u64 = 1024 * 1024;

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_789_200_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

/// An unsigned transfer from the key `seed` derives, to `recipients`.
fn transfer(seed: u8, recipients: &[[u8; 32]]) -> Transaction {
    let key = signing_key_from_seed(&[seed; 32]).expect("key");
    let outputs = recipients
        .iter()
        .map(|recipient| TxOutput {
            amount: 1,
            recipient: *recipient,
        })
        .collect();
    let mut tx = Transaction::new(Vec::new(), outputs, 0);
    tx.public_key = Box::new(key.public_key());
    tx
}

fn features(block: &Block, size: u64, previous: u64) -> BlockFeatures {
    extract(block, size, previous, TARGET)
}

fn value(features: &BlockFeatures, feature: Feature) -> i64 {
    features.values[feature as usize]
}

/// A recipient in the same shard as `address`, or in a different one.
fn recipient_in(address: &[u8; 32], same_shard: bool) -> [u8; 32] {
    (0u8..=255)
        .map(|byte| [byte; 32])
        .find(|candidate| (shard_of(candidate) == shard_of(address)) == same_shard)
        .expect("64 shards and 256 candidates")
}

// ---------------------------------------------------------------------------
// the two halves agree
// ---------------------------------------------------------------------------

#[test]
fn the_extractor_and_the_model_share_their_fixed_point() {
    assert_eq!(FEATURE_COUNT, model::INPUTS);
    assert_eq!(FEATURE_FRAC_BITS, model::FEATURE_FRAC_BITS);
    assert_eq!(FEATURE_ONE, model::FEATURE_ONE);
    assert_eq!(FEATURE_LIMIT, model::FEATURE_LIMIT);
    assert_eq!(neural_gas::MEAN_TX_SCALE_BYTES, 16 * 1024);
    assert_eq!(neural_gas::FUEL_PER_BYTE_SCALE, 1_000);
}

#[test]
fn the_feature_order_is_the_models_order() {
    // One block, each feature driven to a distinct value, read back by the
    // model's own index names.
    let sender = signing_key_from_seed(&[1; 32]).expect("key").address();
    let tx = transfer(1, &[recipient_in(&sender, false)]);
    let block = block_of(vec![tx]);
    let size = 16 * 1024 * 3 / 2;
    let f = features(&block, size, 0);
    // One transaction of 24 KiB: 1.5 of the 16 KiB scale.
    assert_eq!(
        value(&f, Feature::Fullness),
        (size << FEATURE_FRAC_BITS) as i64 / TARGET as i64
    );
    assert_eq!(value(&f, Feature::MeanTxSize), FEATURE_ONE * 3 / 2);
    assert_eq!(value(&f, Feature::AccessOverlap), 0);
    assert_eq!(value(&f, Feature::FuelPerByte), 0);
    assert_eq!(value(&f, Feature::CrossShard), FEATURE_ONE);
    // From an empty parent, the trend is the fullness itself.
    assert_eq!(value(&f, Feature::SizeTrend), value(&f, Feature::Fullness));
}

#[test]
fn the_committed_model_is_a_trained_network() {
    assert_ne!(
        MODEL_V1,
        Model::ZERO,
        "run `cargo run --release -p maya-neural-gas-trainer`"
    );
}

// ---------------------------------------------------------------------------
// extraction
// ---------------------------------------------------------------------------

#[test]
fn an_empty_block_has_only_size_features() {
    let f = features(&block_of(Vec::new()), 0, TARGET);
    assert_eq!(f.values, [0, 0, 0, 0, 0, -FEATURE_ONE]);
}

#[test]
fn size_features_clamp_at_four() {
    let f = features(&block_of(Vec::new()), 10 * TARGET, 0);
    assert_eq!(value(&f, Feature::Fullness), FEATURE_LIMIT);
    assert_eq!(value(&f, Feature::SizeTrend), FEATURE_LIMIT);
}

#[test]
fn overlap_counts_transactions_naming_a_shared_account() {
    let alice = signing_key_from_seed(&[2; 32]).expect("key").address();
    // Two senders paying two unrelated recipients: nothing shared.
    let apart = block_of(vec![transfer(2, &[[0xA0; 32]]), transfer(3, &[[0xB0; 32]])]);
    assert_eq!(
        value(&features(&apart, 1_000, 1_000), Feature::AccessOverlap),
        0
    );

    // The second pays the first's sender: both name Alice.
    let shared = block_of(vec![transfer(2, &[[0xA0; 32]]), transfer(3, &[alice])]);
    assert_eq!(
        value(&features(&shared, 1_000, 1_000), Feature::AccessOverlap),
        FEATURE_ONE
    );

    // A third, unrelated: two of three.
    let mostly = block_of(vec![
        transfer(2, &[[0xA0; 32]]),
        transfer(3, &[alice]),
        transfer(4, &[[0xC0; 32]]),
    ]);
    assert_eq!(
        value(&features(&mostly, 1_000, 1_000), Feature::AccessOverlap),
        (2 * FEATURE_ONE) / 3
    );
}

#[test]
fn a_transaction_within_one_shard_is_not_cross_shard() {
    let sender = signing_key_from_seed(&[5; 32]).expect("key").address();
    let block = block_of(vec![transfer(5, &[recipient_in(&sender, true)])]);
    assert_eq!(
        value(&features(&block, 1_000, 1_000), Feature::CrossShard),
        0
    );
}

#[test]
fn declared_fuel_is_read_per_byte() {
    let call = Transaction::with_kind(
        TxKind::CallContract(ContractCall {
            contract: [9; 32],
            input: Vec::new(),
            gas_limit: 1_000_000,
        }),
        0,
    );
    // 1,000,000 fuel over 1,000 bytes is 1,000 per byte: 1.0.
    let f = features(&block_of(vec![call]), 1_000, 1_000);
    assert_eq!(value(&f, Feature::FuelPerByte), FEATURE_ONE);
}

#[test]
fn extraction_is_a_function_of_the_block() {
    let block = block_of(vec![
        transfer(6, &[[1; 32], [2; 32]]),
        transfer(7, &[[1; 32]]),
    ]);
    assert_eq!(
        features(&block, 30_000, 20_000),
        features(&block, 30_000, 20_000)
    );
}

// ---------------------------------------------------------------------------
// the pipeline, block to fee
// ---------------------------------------------------------------------------

#[test]
fn padding_a_full_block_cannot_lower_the_fee() {
    // The producer's lever: pad an over-target block with cheap, distinct,
    // single-shard transfers to drag overlap, fuel and cross-shard to zero,
    // hoping the network answers with a lower fee. The envelope holds for
    // every block shape and every parent fee.
    let padding: Vec<Transaction> = (10u8..40)
        .map(|seed| {
            let sender = signing_key_from_seed(&[seed; 32]).expect("key").address();
            transfer(seed, &[recipient_in(&sender, true)])
        })
        .collect();
    let block = block_of(padding);
    for size in [TARGET + 1, TARGET * 3 / 2, 2 * TARGET] {
        for previous in [0, TARGET, 2 * TARGET] {
            let f = Features::new(features(&block, size, previous).values);
            for parent in [1, 8, 1_000, 1_000_000] {
                let next = next_base_fee_by_rule(
                    FeeRule::Neural(&MODEL_V1),
                    parent,
                    size,
                    TARGET,
                    8,
                    1,
                    &f,
                );
                let linear = maya_fee_market::next_base_fee(parent, size, TARGET, 8, 1);
                assert!(next > parent, "size {size} parent {parent}: {next}");
                // The padding bought the producer nothing: never below EIP-1559.
                assert!(
                    next >= linear,
                    "size {size} parent {parent}: {next} < {linear}"
                );
            }
        }
    }
}
