//! Hybrid ML-DSA-65 + SLH-DSA-SHA2-128s signing and verification benchmark.
//!
//! Exists to answer, with data rather than assertion, the question the hybrid
//! scheme's whole design rests on: *who pays, and how much?*
//!
//! Each scheme is measured in isolation and then together, at each of the three
//! operations, because the two have opposite cost profiles and the totals hide
//! that. ML-DSA is fast to sign and fast to verify. SLH-DSA-SHA2-128s is
//! roughly four hundred times slower to sign and under twice as slow to verify.
//! A single "hybrid signing costs X" number would obscure the only thing that
//! makes the trade acceptable — that the expensive operation happens once, in a
//! wallet, while the cheap one happens on every node forever.
//!
//! The last group answers requirement 4 directly: signature cost as it actually
//! lands during block execution, measured through `StateDB::apply_block` rather
//! than by multiplying a microbenchmark by a block size.
//!
//! Run with:
//! ```text
//! cargo bench --bench hybrid_signing
//! ```
//!
//! Memory is measured separately, by `benches/hybrid_footprint.rs`. Criterion
//! and a counting global allocator do not coexist usefully: criterion's own
//! allocations would dominate the count.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::keys;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_crypto_pq::sig as slh;
use tempfile::TempDir;
mod common;


/// Transactions per block in the throughput group.
///
/// Not a round number for its own sake: at ~13.2 KB per signed transaction this
/// is ~845 KB, comfortably inside the 8 MiB gossip ceiling and large enough that
/// per-block fixed costs do not dominate the per-transaction figure.
const BLOCK_TRANSACTIONS: usize = 64;

/// A deterministic key, so a bench run is comparable to the one before it.
fn key(seed: u8) -> HybridSigningKey {
    signing_key_from_seed(&[seed; 32]).expect("derive")
}

/// The message shape a real signature covers: a one-output transfer's signing
/// bytes, not an arbitrary short string.
fn signing_message() -> Vec<u8> {
    Transaction::new(
        vec![],
        vec![TxOutput {
            amount: 1_000,
            recipient: [7u8; 32],
        }],
        0,
    )
    .signing_bytes()
}

// ---------------------------------------------------------------------------
// per-scheme costs
// ---------------------------------------------------------------------------

fn bench_signing(c: &mut Criterion) {
    let hybrid = key(1);
    let lattice = keys::signing_key_from_seed(&[2u8; 32]).expect("derive");
    let hash_based = slh::signing_key_from_seed(&[3u8; 32]);
    let message = signing_message();

    let mut group = c.benchmark_group("sign");
    // A single SLH-DSA-128s signature is ~200 ms, so criterion's default 100
    // samples would run for half a minute per function for no extra precision.
    group.sample_size(10);

    group.bench_function("ml_dsa_65", |b| {
        b.iter(|| lattice.sign(black_box(&message, &common::test_chain())).expect("sign"));
    });

    group.bench_function("slh_dsa_sha2_128s", |b| {
        b.iter(|| hash_based.sign(black_box(&message, &common::test_chain())));
    });

    // The sum, and the number a wallet user actually waits for.
    group.bench_function("hybrid", |b| {
        b.iter(|| hybrid.sign(black_box(&message, &common::test_chain())).expect("sign"));
    });

    group.finish();
}

fn bench_verification(c: &mut Criterion) {
    let hybrid = key(4);
    let message = signing_message();
    let signature = hybrid.sign(&message).expect("sign");
    let verifying = hybrid.verifying_key();

    let lattice = keys::signing_key_from_seed(&[5u8; 32]).expect("derive");
    let lattice_signature = lattice.sign(&message).expect("sign");
    let lattice_verifying = lattice.verifying_key();

    let hash_based = slh::signing_key_from_seed(&[6u8; 32]);
    let hash_signature = hash_based.sign(&message, &common::test_chain());
    let hash_verifying = hash_based.verifying_key();

    // This is the group that matters for consensus. Verification is what every
    // node does for every transaction in every block, forever.
    let mut group = c.benchmark_group("verify");

    group.bench_function("ml_dsa_65", |b| {
        b.iter(|| {
            lattice_verifying
                .verify(black_box(&message), &lattice_signature)
                .expect("verify")
        });
    });

    group.bench_function("slh_dsa_sha2_128s", |b| {
        b.iter(|| {
            hash_verifying
                .verify(black_box(&message), &hash_signature)
                .expect("verify")
        });
    });

    group.bench_function("hybrid", |b| {
        b.iter(|| {
            verifying
                .verify(black_box(&message), &signature)
                .expect("verify")
        });
    });

    // The rejection path, which is what an attacker controls the rate of. The
    // lattice half is checked first precisely so that a garbage transaction
    // costs a node the cheaper of the two verifications rather than the sum;
    // this measures whether that ordering actually buys anything.
    let mut forged = signature.clone();
    forged.lattice[0] ^= 0x01;
    group.bench_function("hybrid_reject_at_lattice", |b| {
        b.iter(|| verifying.verify(black_box(&message), &forged).is_err());
    });

    let mut forged_hash = signature.clone();
    forged_hash.hash_based[0] ^= 0x01;
    group.bench_function("hybrid_reject_at_hash", |b| {
        b.iter(|| verifying.verify(black_box(&message), &forged_hash).is_err());
    });

    group.finish();
}

fn bench_keygen(c: &mut Criterion) {
    let mut group = c.benchmark_group("keygen");
    group.sample_size(10);

    group.bench_function("ml_dsa_65", |b| {
        b.iter(|| keys::signing_key_from_seed(black_box(&[7u8; 32])).expect("derive"));
    });

    // ~26 ms, two orders of magnitude above the lattice half. Worth knowing
    // because a wallet creating an account pays it, and because it is the one
    // SLH-DSA cost that is not obviously dominated by signing.
    group.bench_function("slh_dsa_sha2_128s", |b| {
        b.iter(|| slh::signing_key_from_seed(black_box(&[8u8; 32])));
    });

    group.bench_function("hybrid", |b| {
        b.iter(|| signing_key_from_seed(black_box(&[9u8; 32])).expect("derive"));
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// block execution
// ---------------------------------------------------------------------------

fn header() -> BlockHeader {
    BlockHeader {
        prev_hash: [0u8; 32],
        state_root: [0u8; 32],
        timestamp: 1_756_252_800,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    }
}

/// Signs `count` transfers from `sender`, one per nonce.
///
/// Hoisted out of every measured closure below. At ~200 ms per signature this
/// would otherwise be three orders of magnitude larger than the verification it
/// is meant to isolate, and the resulting number would measure signing with a
/// rounding error of block execution attached.
fn signed_block(sender: &HybridSigningKey, recipient: &Address, count: usize) -> Block {
    let transactions = (0..count)
        .map(|nonce| {
            let mut tx = Transaction::new(
                vec![],
                vec![TxOutput {
                    amount: 1,
                    recipient: *recipient,
                }],
                nonce as u64,
            );
            tx.sign(sender).expect("sign");
            tx
        })
        .collect();

    Block::new(header(), transactions)
}

fn bench_block_execution(c: &mut Criterion) {
    let sender = key(10);
    let recipient = key(11).address();
    let block = signed_block(&sender, &recipient, BLOCK_TRANSACTIONS);

    let encoded: usize = block
        .transactions
        .iter()
        .map(|tx| tx.to_bytes().len())
        .sum();

    let mut group = c.benchmark_group("block_execution");
    group.sample_size(10);
    group.throughput(criterion::Throughput::Elements(BLOCK_TRANSACTIONS as u64));

    // A fresh database per iteration. Reusing one would leave the sender's
    // nonce advanced past the block's, so every transaction after the first
    // iteration would be rejected at the nonce check — before reaching the
    // signature verification this is here to measure.
    group.bench_function(format!("apply_block_{BLOCK_TRANSACTIONS}_tx"), |b| {
        b.iter_batched(
            || {
                let dir = TempDir::new().expect("temp dir");
                let db = StateDB::open(dir.path()).expect("open");
                db.put_account(
                    &sender.address(),
                    &Account {
                        balance: 1_000_000,
                        nonce: 0,
                    },
                )
                .expect("fund");
                (db, dir)
            },
            |(db, dir)| {
                db.apply_block(black_box(&block), BlockContext::at_height(1))
                    .expect("apply");
                // Returned so the temp dir outlives the measured work rather
                // than being dropped — and deleted — inside it.
                (db, dir)
            },
            BatchSize::LargeInput,
        );
    });

    group.finish();

    // Not a measurement, but the number every block-size decision is made
    // against, and cheaper to print here than to recompute by hand.
    println!(
        "\nblock of {BLOCK_TRANSACTIONS} hybrid-signed transfers: {encoded} bytes \
         ({} bytes/tx, {} tx per 8 MiB gossip message)",
        encoded / BLOCK_TRANSACTIONS,
        (8 * 1024 * 1024) / (encoded / BLOCK_TRANSACTIONS)
    );
}

criterion_group!(
    benches,
    bench_signing,
    bench_verification,
    bench_keygen,
    bench_block_execution
);
criterion_main!(benches);
