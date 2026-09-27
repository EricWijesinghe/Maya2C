//! Measured DAG-BFT throughput through the node's own code (Master Prompts 4
//! and 12): transactions that are signature-verified, executed, committed to
//! `RocksDB` and final, per second of wall time.
//!
//! Four validators in one process, each with its own chain and state, frames
//! passed through an in-memory mesh. Pre-signed ML-DSA-65 (v7) transfers from
//! many accounts, submitted to the validator whose share they are.
//!
//! What the number is and is not:
//! - **Is:** every transaction is verified, executed and committed by *all
//!   four* nodes, sharing one machine. A node on its own box would do a
//!   quarter of this work.
//! - **Is not:** network-bound. There is no link latency; round pacing is
//!   off. It is the CPU and storage ceiling of the execution path.
//!
//! ```text
//! cargo run --release -p custom-l1-node --example bft_tps -- [accounts] [txs_per_account]
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use custom_l1_node::consensus::bft::{BftDriver, BftSetup};
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::keys::{self, VerifyingKey};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::account::Account;
use custom_l1_node::state::db::StateDB;
use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite};
use maya_dag_bft::Params;

const VALIDATORS: usize = 4;

fn genesis() -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        Vec::new(),
    )
}

/// Signs `per_account` transfers for each of `accounts` keys, on every core.
fn sign_all(accounts: usize, per_account: u64) -> Vec<Transaction> {
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let chunks: Vec<Vec<usize>> = (0..threads)
        .map(|t| (t..accounts).step_by(threads).collect())
        .collect();
    let handles: Vec<_> = chunks
        .into_iter()
        .map(|mine| {
            std::thread::spawn(move || {
                let mut out = Vec::new();
                for a in mine {
                    let mut seed = [0u8; 32];
                    seed[..8].copy_from_slice(&(a as u64 + 1).to_le_bytes());
                    let key = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes(seed));
                    for nonce in 0..per_account {
                        let mut tx = Transaction::new(
                            vec![],
                            vec![TxOutput {
                                amount: 1,
                                recipient: [0x77; 32],
                            }],
                            nonce,
                        );
                        tx.sign_with_suite::<MlDsa65>(&key).unwrap();
                        out.push(tx);
                    }
                }
                out
            })
        })
        .collect();
    handles
        .into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect()
}

fn main() {
    let args: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let accounts = args.first().copied().unwrap_or(2_000);
    let per_account = args.get(1).copied().unwrap_or(5) as u64;

    let t_sign = Instant::now();
    let txs = sign_all(accounts, per_account);
    println!(
        "signed {} ML-DSA-65 transfers in {:.1} s",
        txs.len(),
        t_sign.elapsed().as_secs_f64()
    );

    let root = tempfile::TempDir::new_in(std::env::var("TPS_DIR").unwrap_or_else(|_| ".".into()))
        .expect("temp dir");
    let signers: Vec<_> = (0..VALIDATORS)
        .map(|i| {
            Arc::new(keys::signing_key_from_seed(&[u8::try_from(i).unwrap() + 1; 32]).unwrap())
        })
        .collect();
    let committee: Arc<[VerifyingKey]> = signers.iter().map(|k| k.verifying_key()).collect();
    let senders: Vec<[u8; 32]> = txs.iter().map(Transaction::sender).collect();

    let mut chains = Vec::new();
    let mut drivers = Vec::new();
    let mut queue: VecDeque<(usize, Vec<u8>)> = VecDeque::new();
    for (i, signer) in signers.iter().enumerate() {
        let state = Arc::new(StateDB::open(root.path().join(format!("s{i}"))).unwrap());
        for s in &senders {
            state
                .put_account(
                    s,
                    &Account {
                        balance: 1_000_000,
                        nonce: 0,
                    },
                )
                .unwrap();
        }
        let mut chain =
            Chain::open(state, genesis(), ChainConfig::without_pow_verification()).unwrap();
        let setup = BftSetup {
            epoch: 0,
            committee: Arc::clone(&committee),
            signer: Some(Arc::clone(signer)),
            // One round per 50 ms tick of virtual time: without pacing,
            // rounds race inside a single pump and the loop never yields.
            params: Params {
                batch_size: 1_000,
                min_round_interval_ms: 50,
                resend_interval_ms: 500,
                ..Params::default()
            },
        };
        let (driver, step) =
            BftDriver::open(&setup, &root.path().join(format!("b{i}")), &mut chain, 0).unwrap();
        queue.extend(step.frames.into_iter().map(|f| (i, f)));
        chains.push(chain);
        drivers.push(driver);
    }

    for tx in &txs {
        let owner = usize::from(tx.txid()[0]) % VALIDATORS;
        drivers[owner].submit(tx);
    }

    let total = txs.len();
    let mut included = [0usize; VALIDATORS];
    let mut blocks = [0usize; VALIDATORS];
    let mut build = std::time::Duration::ZERO;
    let started = Instant::now();
    let mut now = 0u64;
    while included.iter().any(|n| *n < total) && started.elapsed().as_secs() < 600 {
        now += 50;
        if now.is_multiple_of(5_000) {
            eprintln!(
                "t={now} heights {:?} included {included:?} queue {}",
                chains.iter().map(Chain::height).collect::<Vec<_>>(),
                queue.len()
            );
        }
        for i in 0..VALIDATORS {
            let step = drivers[i].on_tick(&mut chains[i], now).unwrap();
            for tx in &step.dropped {
                drivers[i].submit(tx);
            }
            included[i] += step.included.len();
            blocks[i] += step.blocks.len();
            build += step.build_time;
            queue.extend(step.frames.into_iter().map(|f| (i, f)));
        }
        for _ in 0..2_000 {
            let Some((from, frame)) = queue.pop_front() else {
                break;
            };
            for i in (0..VALIDATORS).filter(|i| *i != from) {
                let step = drivers[i].on_frame(&mut chains[i], now, &frame).unwrap();
                for tx in &step.dropped {
                    drivers[i].submit(tx);
                }
                included[i] += step.included.len();
                blocks[i] += step.blocks.len();
                build += step.build_time;
                queue.extend(step.frames.into_iter().map(|f| (i, f)));
            }
        }
    }
    let secs = started.elapsed().as_secs_f64();
    let tips: Vec<_> = chains
        .iter()
        .map(|c| c.state().state_root().unwrap())
        .collect();
    let agree = chains.iter().all(|c| c.height() == chains[0].height())
        && tips.iter().all(|t| *t == tips[0]);
    println!(
        "dag-bft tps: {} validators in one process; {total} transfers committed on every node in {secs:.2} s \
         = {:.0} tx/s finalized per network ({:.0} verify+execute+commit per node-second across 4 nodes); \
         {} blocks, {:.0} tx/block; chains agree: {agree}",
        VALIDATORS,
        total as f64 / secs,
        4.0 * total as f64 / secs,
        blocks[0],
        total as f64 / blocks[0].max(1) as f64,
    );
    println!(
        "  of which building + inserting blocks (select, execute, state root, commit), summed over the 4 nodes: {:.2} s",
        build.as_secs_f64()
    );
    let (reused, recomputed) = chains.iter().fold((0, 0), |(r, c), chain| {
        let (hit, miss) = chain.state().preview_stats();
        (r + hit, c + miss)
    });
    println!(
        "  block applies that reused the builder's preview: {reused}, staged afresh: {recomputed}"
    );
    assert!(
        included.iter().all(|n| *n == total),
        "not every node included every transfer: {included:?}"
    );
    assert!(agree, "chains diverged");
}
