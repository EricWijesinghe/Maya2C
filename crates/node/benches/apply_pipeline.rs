//! The node's transaction path, stage by stage (Master Prompt 12 §0 and §4).
//!
//! Signs `SENDERS × PER_SENDER` real hybrid transfers, then measures, on the
//! real `StateDB` over `RocksDB`:
//!
//! - **verify**: `Transaction::verify` per transaction, on one thread and on
//!   every core (`std::thread::scope`, no pool);
//! - **apply**: `StateDB::apply_block` per block — which verifies again inside
//!   `stage_transaction`, so `apply − verify` is the state work alone;
//! - **allocations** per transaction in each stage, by a counting allocator;
//! - **write amplification**: bytes the process sent to the block layer
//!   (`/proc/self/io` `write_bytes`) over the serialized bytes applied;
//! - **peak RSS** (`VmHWM`).
//!
//! Nothing here asserts a figure. It prints what it measured, the profile it
//! ran under, and what it could not measure (`/proc/self/io` is Linux-only).
//! `cargo bench -p custom-l1-node --bench apply_pipeline`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::{Account, BlockContext, StateDB};

const SENDERS: usize = 16;
const PER_SENDER: usize = 25;
const BLOCK_TXS: usize = 100;
/// An account record as the state root hashes it: address, balance, nonce.
const ACCOUNT_RECORD_BYTES: usize = 32 + 8 + 8;

/// Allocation calls since start.
static ALLOCS: AtomicU64 = AtomicU64::new(0);

struct Counting;

// SAFETY: every method forwards to `System` unchanged and returns what it
// returns; the counter is an atomic that touches no allocator memory, so this
// is exactly as sound as the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `layout` is forwarded unchanged to `System`, whose pointer is
        // returned unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` and `layout` come from a matching `alloc` on this
        // allocator, which forwarded them to `System`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocs() -> u64 {
    ALLOCS.load(Ordering::Relaxed)
}

/// A `/proc/self/*` field, if this platform has it.
fn proc_field(file: &str, key: &str) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/self/{file}")).ok()?;
    let line = text.lines().find(|l| l.starts_with(key))?;
    line[key.len()..]
        .trim()
        .trim_end_matches(" kB")
        .trim()
        .parse()
        .ok()
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    let header = BlockHeader {
        prev_hash: [0u8; 32],
        state_root: [0u8; 32],
        timestamp: 1_756_252_800,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    };
    Block::new(header, transactions)
}

/// Every sender's transfers, signed in parallel (signing is not measured).
fn signed(keys: &[HybridSigningKey]) -> Vec<Transaction> {
    let per_key: Vec<Vec<Transaction>> = std::thread::scope(|s| {
        let handles: Vec<_> = keys
            .iter()
            .enumerate()
            .map(|(i, key)| {
                s.spawn(move || {
                    (0..PER_SENDER as u64)
                        .map(|nonce| {
                            let to = [u8::try_from(i).unwrap().wrapping_add(0x80); 32];
                            let mut tx = Transaction::new(
                                vec![],
                                vec![TxOutput {
                                    amount: 1,
                                    recipient: to,
                                }],
                                nonce,
                            );
                            tx.sign(key).unwrap();
                            tx
                        })
                        .collect()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    // Round-robin by nonce so every block holds each sender's next nonces in order.
    (0..PER_SENDER)
        .flat_map(|n| per_key.iter().map(move |txs| txs[n].clone()))
        .collect()
}

/// Distinct accounts a block rewrites.
fn touched(block: &[Transaction]) -> usize {
    let mut set = std::collections::BTreeSet::new();
    for tx in block {
        set.insert(tx.sender());
        set.extend(tx.outputs.iter().map(|o| o.recipient));
    }
    set.len()
}

fn verify_all(txs: &[Transaction], threads: usize) -> f64 {
    let t = Instant::now();
    std::thread::scope(|s| {
        for part in txs.chunks(txs.len().div_ceil(threads)) {
            s.spawn(move || part.iter().for_each(|tx| tx.verify().unwrap()));
        }
    });
    t.elapsed().as_secs_f64()
}

fn main() {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let profile = if cfg!(debug_assertions) {
        "debug-assertions ON (not an optimized build)"
    } else {
        "optimized"
    };
    println!(
        "apply_pipeline: {} hybrid transfers, {BLOCK_TXS}/block, {cores} cores, {profile}",
        SENDERS * PER_SENDER
    );
    let keys: Vec<HybridSigningKey> = (0..SENDERS)
        .map(|i| signing_key_from_seed(&[u8::try_from(i).unwrap() + 1; 32]).unwrap())
        .collect();
    let txs = signed(&keys);
    // Real wire bytes for the compression measurement in reports/13-pq-weight.md.
    if let Ok(dir) = std::env::var("MAYA_DUMP_TXS") {
        std::fs::create_dir_all(&dir).unwrap();
        for (i, tx) in txs.iter().enumerate() {
            std::fs::write(format!("{dir}/{i:04}.tx"), tx.to_bytes()).unwrap();
        }
    }
    let n = txs.len() as f64;
    let wire: usize = txs.iter().map(|tx| tx.to_bytes().len()).sum();

    let a0 = allocs();
    let v1 = verify_all(&txs, 1);
    let verify_allocs = (allocs() - a0) as f64 / n;
    let vn = verify_all(&txs, cores);
    println!(
        "verify   1 thread : {:>8.3} ms/tx  {:>8.0} tx/s  {verify_allocs:.0} allocs/tx",
        v1 / n * 1e3,
        n / v1
    );
    println!(
        "verify {cores:>2} threads: {:>8.3} ms/tx  {:>8.0} tx/s  (x{:.2})",
        vn / n * 1e3,
        n / vn,
        v1 / vn
    );

    let dir = tempfile::Builder::new()
        .prefix("apply-bench")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let io0 = proc_field("io", "write_bytes:");
    let db = StateDB::open(dir.path()).unwrap();
    for k in &keys {
        db.put_account(
            &k.address(),
            &Account {
                balance: 1_000_000,
                nonce: 0,
            },
        )
        .unwrap();
    }
    let (a1, t) = (allocs(), Instant::now());
    for chunk in txs.chunks(BLOCK_TXS) {
        db.apply_block(&block_of(chunk.to_vec()), BlockContext::GENESIS)
            .unwrap();
    }
    let apply = t.elapsed().as_secs_f64();
    let apply_allocs = (allocs() - a1) as f64 / n;
    drop(db); // flush memtables so their bytes reach the block layer
    let io1 = proc_field("io", "write_bytes:");
    println!(
        "apply    1 thread : {:>8.3} ms/tx  {:>8.0} tx/s  {apply_allocs:.0} allocs/tx (includes verify)",
        apply / n * 1e3,
        n / apply
    );
    println!(
        "  state work alone: {:>8.3} ms/tx  ({:.1}% of apply; {:.0} allocs/tx)",
        (apply - v1) / n * 1e3,
        (apply - v1) / apply * 100.0,
        apply_allocs - verify_allocs
    );
    match (io0, io1) {
        (Some(a), Some(b)) => {
            // StateDB stores accounts, not transaction bodies (the block store
            // is the Chain's), so the logical bytes are the account records a
            // block rewrites: every sender and recipient, 32-byte key + balance + nonce.
            let logical = txs.chunks(BLOCK_TXS).map(touched).sum::<usize>() * ACCOUNT_RECORD_BYTES;
            println!(
                "write amplification: {} B to the block layer / {logical} B of account records rewritten = {:.1}x ({wire} B of transactions applied)",
                b - a,
                (b - a) as f64 / logical as f64
            );
        }
        _ => println!(
            "write amplification: NOT MEASURED (/proc/self/io unavailable on this platform)"
        ),
    }
    match proc_field("status", "VmHWM:") {
        Some(kb) => println!("peak RSS: {:.1} MiB", kb as f64 / 1024.0),
        None => println!("peak RSS: NOT MEASURED (/proc/self/status unavailable)"),
    }
}
