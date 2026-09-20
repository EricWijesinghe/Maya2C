//! The in-tree fuzzing gate: seeded mutations through the decoders and the apply
//! path, asserting no panic and no non-determinism.
//!
//! This runs on the node's **stable** toolchain in the main workspace, so it
//! cannot link LibAFL or ASan — the coverage-guided search lives in the
//! out-of-workspace `offsec-sandbox` crate. What it *can* be is a fast, hermetic
//! regression gate with no extra machinery: a seeded PRNG mutates encoded
//! transactions, and each mutant is driven through `Transaction::from_bytes` and
//! through two independent states, checking:
//!
//! - **no panic** — a decoder or apply path that unwinds on hostile bytes is a
//!   denial of service;
//! - **determinism** — a block that applies at all applies to the same state
//!   root on two independent states (invariant 24, execution directive 2).
//!
//! # What this does and does not claim
//!
//! It asserts *no crash across N inputs*, never "100% crash resistance". A clean
//! run is evidence, not a proof: fuzzing shows a crash was not *found*, never
//! that none *exists*. The default N is 50,000 for CI time; set
//! `MAYA_FUZZ_ITERS=1000000` for a soak.
//!
//! The two properties run at different volumes on purpose. Decoding and the
//! round-trip are cheap, so every one of the N mutants is checked. Applying a
//! block opens two fresh RocksDB states — tens of milliseconds — so the
//! determinism check runs on a bounded sample (`MAYA_FUZZ_APPLIES`, default
//! 200) rather than every mutant, which keeps a 50,000-input run to seconds
//! while still exercising the apply path on structurally varied blocks.
//!
//! The mutation logic here is deliberately small and duplicated rather than
//! pulled from `offsec-sandbox` — pulling it would drag LibAFL's graph into the
//! node's test build, which is the whole thing the separate workspace prevents.
//! The richer, structure-aware mutators live there.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::panic::{self, AssertUnwindSafe};

use custom_l1_node::core::{Block, BlockHeader, Transaction, TxInput, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::target_from_leading_zero_bits;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use tempfile::TempDir;

/// The value of a `u64` env var, or `default`.
fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// `u64` boundary values plus full-range, the edges that break arithmetic.
fn boundary_u64(rng: &mut ChaCha20Rng) -> u64 {
    const EDGES: [u64; 6] = [0, 1, u64::MAX, u64::MAX - 1, i64::MAX as u64, 1 << 32];
    if rng.next_u32() & 1 == 0 {
        EDGES[(rng.next_u32() as usize) % EDGES.len()]
    } else {
        rng.next_u64()
    }
}

/// A seed transaction, signed or not, encoded.
fn seed(rng: &mut ChaCha20Rng, key: &HybridSigningKey) -> Vec<u8> {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: boundary_u64(rng),
            recipient: [(rng.next_u32() & 0xff) as u8; 32],
        }],
        rng.next_u64(),
    );
    if rng.next_u32() & 1 == 0 {
        let _ = tx.sign(key);
    }
    tx.to_bytes()
}

/// One structure-aware mutation of an encoded transaction: decode, perturb a
/// field, re-encode. Falls back to a byte flip if the seed does not decode.
fn mutate(bytes: &[u8], rng: &mut ChaCha20Rng) -> Vec<u8> {
    let Ok(mut tx) = Transaction::from_bytes(bytes) else {
        let mut out = bytes.to_vec();
        if !out.is_empty() {
            let i = (rng.next_u32() as usize) % out.len();
            out[i] ^= 1 << (rng.next_u32() % 8);
        }
        return out;
    };
    match rng.next_u32() % 5 {
        0 => {
            if let Some(o) = tx.outputs.first_mut() {
                o.amount = boundary_u64(rng);
            }
        }
        1 => tx.nonce = boundary_u64(rng),
        2 => {
            let n = (rng.next_u32() % 2048) as usize;
            tx.outputs = (0..n)
                .map(|i| TxOutput {
                    amount: boundary_u64(rng),
                    recipient: [i as u8; 32],
                })
                .collect();
        }
        3 => {
            let n = (rng.next_u32() % 2048) as usize;
            tx.inputs = (0..n)
                .map(|i| TxInput {
                    prev_tx: [i as u8; 32],
                    index: rng.next_u32(),
                })
                .collect();
        }
        _ => tx.signature = None,
    }
    tx.to_bytes()
}

/// Catches a panic and returns its message, so the harness reports the input
/// rather than aborting the whole run.
fn guard<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        p.downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic".to_string())
    })
}

/// A fresh RocksDB state funding one account, identical each call.
fn fresh_state(owner: &Address) -> (StateDB, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let state = StateDB::open(dir.path()).expect("open state");
    state
        .put_account(
            owner,
            &Account {
                balance: 1_000_000,
                nonce: 0,
            },
        )
        .expect("fund");
    (state, dir)
}

fn block_of(tx: Transaction) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0u8; 32],
        },
        vec![tx],
    )
}

#[test]
fn seeded_mutations_never_panic_the_decoder_or_apply_path_and_stay_deterministic() {
    // A quiet hook: the harness reports the offending input itself, and a
    // backtrace per caught panic would drown a soak run.
    panic::set_hook(Box::new(|_| {}));

    // A fixed seed, so a failure names an input a rerun reproduces.
    let mut rng = ChaCha20Rng::seed_from_u64(0x_2C_FA_11);
    let key = generate_signing_key().expect("keygen");
    let owner = key.address();
    let iters = env_u64("MAYA_FUZZ_ITERS", 50_000);
    let apply_budget = env_u64("MAYA_FUZZ_APPLIES", 200);
    // Space the (expensive) apply checks evenly across the run.
    let apply_every = (iters / apply_budget.max(1)).max(1);

    let mut corpus: Vec<Vec<u8>> = (0..32).map(|_| seed(&mut rng, &key)).collect();

    for i in 0..iters {
        let base = corpus[(rng.next_u32() as usize) % corpus.len()].clone();
        let input = mutate(&base, &mut rng);

        // 1. Every mutant: decoding, and re-encoding what decoded, must not
        //    panic, and a decoded transaction must round-trip.
        let decoded = match guard(|| Transaction::from_bytes(&input)) {
            Ok(result) => result,
            Err(message) => panic!(
                "decode panicked on input {}: {message}",
                hex::encode(&input)
            ),
        };
        let Ok(tx) = decoded else { continue };
        let re = tx.to_bytes();
        assert!(
            Transaction::from_bytes(&re).is_ok_and(|back| back == tx),
            "transaction did not round-trip: {}",
            hex::encode(&input)
        );

        // Feed a fraction back into the corpus so mutations compound.
        if i % 97 == 0 && corpus.len() < 256 {
            corpus.push(input.clone());
        }

        // 2. A bounded sample: applied to two independent, identically funded
        //    states, the block must not panic and must reach the same root.
        if i % apply_every != 0 {
            continue;
        }
        let block = block_of(tx);
        let apply = || {
            let (state, _dir) = fresh_state(&owner);
            state.apply_block(&block, BlockContext::at_height(1)).ok()
        };
        let left = guard(apply)
            .unwrap_or_else(|m| panic!("apply panicked on input {}: {m}", hex::encode(&input)));
        let right = guard(apply)
            .unwrap_or_else(|m| panic!("apply panicked on input {}: {m}", hex::encode(&input)));
        assert_eq!(
            left,
            right,
            "non-deterministic apply on input {}",
            hex::encode(&input)
        );
    }
}
