//! What the neural base-fee gain costs, next to what a burst actually costs.
//!
//! ## Why this measures rather than asserts "sub-second under 100,000 tx/s"
//!
//! A hybrid signature is 11,165 bytes and about 0.18 ms to verify. 100,000
//! transactions a second is 1.1 GB/s of signatures and ~18 CPU-seconds of
//! verification per second of burst, before any fee is computed; an 8 MiB block
//! holds about 650 of them. No fee rule changes that, and a benchmark asserting
//! it would be measuring something that is not this chain
//! (`docs/blockgraph.md`, "The throughput ceiling").
//!
//! So four figures, none asserted:
//!
//! | Group | Measures |
//! |---|---|
//! | `neural_fee/gain` | one inference: 112 multiply-accumulates |
//! | `neural_fee/block/{64,650}` | feature extraction plus the rule, per block |
//! | `neural_fee/burst_100k` | the same over the 154 full blocks a 100,000-transaction burst fills |
//! | `mempool/admit_signed` | real admission, signature verification included — the number that bounds a burst |
//! | `block_validation/{linear,neural}` | applying a signed block, then each rule; the gap is the rule's cost |

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::hint::black_box;
use std::sync::Arc;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tempfile::TempDir;

use custom_l1_node::core::{Block, BlockHeader, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::network::mempool::Mempool;
use custom_l1_node::neural_gas::extract;
use custom_l1_node::state::{Account, BlockContext, StateDB};
use maya_fee_market::{Features, FeeRule, MODEL_V1, next_base_fee_by_rule};

const TARGET: u64 = 1024 * 1024;
const DENOMINATOR: u64 = 8;
const FLOOR: u64 = 1;
const PARENT_FEE: u64 = 1_000;

/// Signed transactions an 8 MiB block holds, at ~12.6 KB each.
const FULL_BLOCK_TXS: usize = 650;

/// The burst the brief names.
const BURST_TXS: usize = 100_000;

/// Distinct keys the synthetic blocks cycle through.
const KEYS: usize = 64;

fn keys() -> Vec<HybridSigningKey> {
    (0..KEYS)
        .map(|i| signing_key_from_seed(&[i as u8; 32]).expect("key"))
        .collect()
}

fn header() -> BlockHeader {
    BlockHeader {
        prev_hash: [0u8; 32],
        state_root: [0u8; 32],
        timestamp: 1_789_200_000,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    }
}

/// A block of `count` unsigned transfers, as the extractor sees one.
fn synthetic_block(keys: &[HybridSigningKey], count: usize) -> Block {
    let transactions = (0..count)
        .map(|i| {
            let mut tx = Transaction::new(
                Vec::new(),
                vec![TxOutput {
                    amount: 1,
                    recipient: [(i % 251) as u8; 32],
                }],
                i as u64,
            );
            tx.public_key = Box::new(keys[i % keys.len()].public_key());
            tx
        })
        .collect();
    Block::new(header(), transactions)
}

fn neural_fee(block: &Block, size: u64) -> u64 {
    let features = Features::new(extract(block, size, TARGET, TARGET).values);
    next_base_fee_by_rule(
        FeeRule::Neural(&MODEL_V1),
        PARENT_FEE,
        size,
        TARGET,
        DENOMINATOR,
        FLOOR,
        &features,
    )
}

fn bench_rule(c: &mut Criterion) {
    let keys = keys();
    let features = Features::new([40_000, 50_000, 10_000, 0, 20_000, 3_000]);
    c.bench_function("neural_fee/gain", |b| {
        b.iter(|| MODEL_V1.gain(black_box(&features)));
    });

    let mut group = c.benchmark_group("neural_fee/block");
    for count in [64, FULL_BLOCK_TXS] {
        let block = synthetic_block(&keys, count);
        let size = block.to_bytes().len() as u64;
        group.bench_with_input(BenchmarkId::from_parameter(count), &block, |b, block| {
            b.iter(|| neural_fee(black_box(block), size));
        });
    }
    group.finish();

    let full = synthetic_block(&keys, FULL_BLOCK_TXS);
    let size = full.to_bytes().len() as u64;
    let blocks = BURST_TXS.div_ceil(FULL_BLOCK_TXS);
    let mut burst = c.benchmark_group("neural_fee/burst_100k");
    burst.throughput(Throughput::Elements(BURST_TXS as u64));
    burst.sample_size(10);
    burst.bench_function("features_and_rule", |b| {
        b.iter(|| {
            (0..blocks)
                .map(|_| neural_fee(black_box(&full), size))
                .sum::<u64>()
        });
    });
    burst.finish();
}

/// A funded state and one signed transfer per key, each at nonce 0.
fn signed_setup(keys: &[HybridSigningKey]) -> (Arc<StateDB>, TempDir, Vec<Transaction>) {
    let dir = TempDir::new().expect("dir");
    let db = StateDB::open(dir.path()).expect("open");
    let transactions = keys
        .iter()
        .map(|key| {
            db.put_account(
                &key.address(),
                &Account {
                    balance: 1_000_000,
                    nonce: 0,
                },
            )
            .expect("fund");
            let mut tx = Transaction::new(
                Vec::new(),
                vec![TxOutput {
                    amount: 1,
                    recipient: [7; 32],
                }],
                0,
            );
            tx.sign(key).expect("sign");
            tx
        })
        .collect();
    (Arc::new(db), dir, transactions)
}

fn bench_signed(c: &mut Criterion) {
    let keys = keys();
    let (state, _dir, transactions) = signed_setup(&keys);

    let mut admit = c.benchmark_group("mempool/admit_signed");
    admit.throughput(Throughput::Elements(transactions.len() as u64));
    admit.sample_size(10);
    admit.bench_function("64", |b| {
        b.iter_batched(
            || Mempool::with_capacity(Arc::clone(&state), transactions.len()),
            |pool| {
                for tx in &transactions {
                    pool.insert(tx.clone()).expect("admitted");
                }
            },
            BatchSize::PerIteration,
        );
    });
    admit.finish();

    let block = Block::new(header(), transactions);
    let size = block.to_bytes().len() as u64;
    let mut validation = c.benchmark_group("block_validation");
    validation.sample_size(10);
    for neural in [false, true] {
        let name = if neural { "neural" } else { "linear" };
        validation.bench_function(name, |b| {
            b.iter_batched(
                || signed_setup(&keys),
                |(db, _dir, _)| {
                    db.apply_block(&block, BlockContext::at_height(1))
                        .expect("valid");
                    if neural {
                        black_box(neural_fee(&block, size));
                    } else {
                        black_box(maya_fee_market::next_base_fee(
                            PARENT_FEE,
                            size,
                            TARGET,
                            DENOMINATOR,
                            FLOOR,
                        ));
                    }
                },
                BatchSize::PerIteration,
            );
        });
    }
    validation.finish();
}

criterion_group!(benches, bench_rule, bench_signed);
criterion_main!(benches);
