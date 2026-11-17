//! v3 P1: `StateDB::apply_block` with the parallel phase off and on, on the
//! same signed blocks.
//!
//! Two cases, because the phase can only save work the cache has not done:
//!
//! - **cold**: a fresh database whose verified-signature cache is empty, as on
//!   a node replaying or catching up on blocks it never saw in its mempool;
//! - **warm**: the block's transactions went through `select_applicable` first,
//!   as on a DAG-BFT node whose mempool and builder already verified them.
//!
//! Each case runs `REPS` times, off and on alternating, each on a fresh
//! database, and prints every run and the median. Nothing is asserted; a
//! missing gain is printed as such.
//! `cargo bench -p custom-l1-node --bench parallel_apply` (add `--profile dev`
//! to reuse a test build; the output says which profile ran).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use primitive_types::U256;
use std::time::Instant;

use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::{Account, BlockContext, StateDB};

mod common;

const SENDERS: usize = 32;
const PER_SENDER: usize = 16;
const BLOCK_TXS: usize = 256;
const REPS: usize = 5;

fn block_of(transactions: Vec<Transaction>) -> Block {
    let header = BlockHeader {
        version: 0,
        prev_hash: [0u8; 32],
        state_root: [0u8; 32],
        timestamp: 1_756_252_800,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    };
    Block::new(header, transactions)
}

/// Every sender's transfers, round-robin by nonce, signed on every core.
fn signed(keys: &[HybridSigningKey]) -> Vec<Transaction> {
    let chain = common::test_chain();
    let per_key: Vec<Vec<Transaction>> = std::thread::scope(|s| {
        let handles: Vec<_> = keys
            .iter()
            .enumerate()
            .map(|(i, key)| {
                s.spawn(move || {
                    (0..PER_SENDER as u64)
                        .map(|nonce| {
                            let to = [u8::try_from(i).unwrap().wrapping_add(0x80); 32];
                            let output = TxOutput {
                                amount: U256::from(1),
                                recipient: to,
                            };
                            let mut tx = Transaction::new(vec![], vec![output], nonce);
                            tx.sign(key, &chain).unwrap();
                            tx
                        })
                        .collect()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    (0..PER_SENDER)
        .flat_map(|n| per_key.iter().map(move |txs| txs[n].clone()))
        .collect()
}

fn fresh(keys: &[HybridSigningKey]) -> (StateDB, tempfile::TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("parallel-apply-bench")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    db.bind_chain(common::test_chain()).unwrap();
    for k in keys {
        let account = Account {
            balance: U256::from(1_000_000u64),
            nonce: 0,
        };
        db.put_account(&k.address(), &account).unwrap();
    }
    (db, dir)
}

/// Seconds spent in `apply_block` alone over every block, and the last root.
fn run(
    keys: &[HybridSigningKey],
    blocks: &[Vec<Transaction>],
    parallel: bool,
    warm: bool,
) -> (f64, [u8; 32]) {
    let (db, _dir) = fresh(keys);
    let mut spent = 0.0;
    let mut root = [0u8; 32];
    for (i, txs) in blocks.iter().enumerate() {
        let height = u64::try_from(i).unwrap() + 1;
        let off = BlockContext::at_height(height);
        let context = if parallel {
            off.with_parallel_apply_activation(0)
        } else {
            off
        };
        if warm {
            let kept = db
                .select_applicable(txs.clone(), context, target_from_leading_zero_bits(0))
                .unwrap();
            assert_eq!(kept.len(), txs.len(), "every bench transaction executes");
        }
        let block = block_of(txs.clone());
        let t = Instant::now();
        root = db.apply_block(&block, context).unwrap();
        spent += t.elapsed().as_secs_f64();
    }
    (spent, root)
}

fn median(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(f64::total_cmp);
    xs[xs.len() / 2]
}

fn case(keys: &[HybridSigningKey], blocks: &[Vec<Transaction>], warm: bool, n: f64) {
    let label = if warm { "warm" } else { "cold" };
    let (mut off, mut on) = (Vec::new(), Vec::new());
    for rep in 0..REPS {
        let (a, ra) = run(keys, blocks, false, warm);
        let (b, rb) = run(keys, blocks, true, warm);
        assert_eq!(ra, rb, "parallel apply diverged from serial");
        println!(
            "  {label} rep {rep}: off {:>9.3} ms  on {:>9.3} ms",
            a * 1e3,
            b * 1e3
        );
        off.push(a);
        on.push(b);
    }
    let (m_off, m_on) = (median(off), median(on));
    println!(
        "{label}: median off {:>9.3} ms ({:.0} tx/s)  on {:>9.3} ms ({:.0} tx/s)  off/on x{:.2}",
        m_off * 1e3,
        n / m_off,
        m_on * 1e3,
        n / m_on,
        m_off / m_on
    );
    if m_on >= m_off {
        println!("{label}: NO GAIN measured: the parallel phase was not faster here");
    }
}

fn main() {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let profile = if cfg!(debug_assertions) {
        "debug-assertions ON (not an optimized build)"
    } else {
        "optimized"
    };
    let total = SENDERS * PER_SENDER;
    println!(
        "parallel_apply: {total} hybrid transfers, {BLOCK_TXS}/block, {REPS} reps, {cores} cores, {profile}"
    );
    let keys: Vec<HybridSigningKey> = (0..SENDERS)
        .map(|i| signing_key_from_seed(&[u8::try_from(i).unwrap() + 1; 32]).unwrap())
        .collect();
    let txs = signed(&keys);
    let blocks: Vec<Vec<Transaction>> =
        txs.chunks(BLOCK_TXS).map(<[Transaction]>::to_vec).collect();
    let n = total as f64;
    case(&keys, &blocks, false, n);
    case(&keys, &blocks, true, n);
}
