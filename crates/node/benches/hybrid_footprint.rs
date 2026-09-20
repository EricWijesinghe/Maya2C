//! Memory footprint of hybrid signatures during block execution.
//!
//! The other half of requirement 4. `benches/hybrid_signing.rs` measures time;
//! this measures bytes, which criterion cannot do — it has no memory
//! instrumentation, and installing a counting global allocator underneath it
//! would count criterion's own bookkeeping alongside the work.
//!
//! So this is a plain `harness = false` binary with a counting allocator and a
//! printed table. It reports three things a block-size or mempool-capacity
//! decision actually needs:
//!
//! 1. **Static size** — what a `Transaction` costs on the stack and on the heap.
//!    The heap half is the interesting one: the key pair and signature pair are
//!    boxed precisely so a `Vec<Transaction>` is a vector of pointers rather
//!    than of 13 KB values.
//! 2. **Peak during execution** — the high-water mark while a full block is
//!    verified and committed, which is what bounds a node's working set.
//! 3. **The ML-DSA-only comparison** — the same figures with the hash-based
//!    half excluded, so the cost of the second scheme is a number rather than
//!    an impression.
//!
//! Run with:
//! ```text
//! cargo bench --bench hybrid_footprint
//! ```
//!
//! ## What the allocator can and cannot see
//!
//! It counts every allocation this process makes, which during `apply_block`
//! includes RocksDB's write batch and any temporary buffers the codec builds.
//! That is the right scope — those are real costs of executing a block — but it
//! does mean the peak is *not* attributable to signatures alone. The
//! ML-DSA-only column exists so the difference between the two runs isolates
//! the part this change is responsible for.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{
    HYBRID_PUBLIC_KEY_LEN, HYBRID_SIGNATURE_LENGTH, HybridPublicKey, HybridSignature,
    HybridSigningKey, signing_key_from_seed,
};
use custom_l1_node::crypto::keys::{PUBLIC_KEY_LEN, SIGNATURE_LENGTH};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use tempfile::TempDir;

/// Transactions per block, matching `benches/hybrid_signing.rs`.
const BLOCK_TRANSACTIONS: usize = 64;

/// The node's gossip ceiling, from `src/network/behaviour.rs`.
const MAX_GOSSIP_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
// counting allocator
// ---------------------------------------------------------------------------

/// Live bytes currently allocated.
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// High-water mark of [`LIVE`] since the last reset.
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Total bytes ever allocated since the last reset, ignoring frees.
static TOTAL: AtomicUsize = AtomicUsize::new(0);

/// Wraps the system allocator to track live, peak, and cumulative bytes.
///
/// Relaxed ordering throughout: these counters are diagnostics, not
/// synchronization. A benchmark that paid for sequential consistency on every
/// allocation would be measuring the instrumentation.
struct Counting;

// SAFETY: every method forwards to `System` unchanged and returns what it
// returns; the counters are `AtomicUsize` updates that touch no memory the
// allocator owns. So this allocator is exactly as sound as the system one.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` is forwarded unchanged to the system allocator,
        // which is the only allocator that ever sees it, and the pointer it
        // returns is returned unchanged to the caller.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let size = layout.size();
            TOTAL.fetch_add(size, Ordering::Relaxed);
            let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: `ptr` and `layout` come from a matching `alloc` call on this
        // same allocator, which forwarded them to `System`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Zeroes the counters and returns a token for [`measure`].
fn reset() {
    TOTAL.store(0, Ordering::Relaxed);
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// Runs `f`, then reports `(peak_live, total_allocated)` for that span.
///
/// Peak is measured against whatever was live at [`reset`], so it is the
/// *additional* high-water mark of `f` rather than a process-wide figure.
fn measure<T>(f: impl FnOnce() -> T) -> (T, usize, usize) {
    let baseline = LIVE.load(Ordering::Relaxed);
    reset();
    let value = f();
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    let total = TOTAL.load(Ordering::Relaxed);
    (value, peak, total)
}

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

fn key(seed: u8) -> HybridSigningKey {
    signing_key_from_seed(&[seed; 32]).expect("derive")
}

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

// ---------------------------------------------------------------------------
// report
// ---------------------------------------------------------------------------

fn row(label: &str, hybrid: usize, lattice_only: usize) {
    let ratio = if lattice_only == 0 {
        0.0
    } else {
        hybrid as f64 / lattice_only as f64
    };
    println!("  {label:<38} {hybrid:>12} {lattice_only:>14} {ratio:>8.2}x");
}

fn main() {
    println!("=== Hybrid signature memory footprint ===\n");
    println!(
        "  {:<38} {:>12} {:>14} {:>8}",
        "", "hybrid", "ML-DSA only", "ratio"
    );

    // --- static sizes -----------------------------------------------------
    //
    // `size_of` is the stack cost. The heap cost is the boxed key and signature
    // pairs, which is where nearly all of it lives — and deliberately so.
    println!("\nstatic sizes (bytes)");
    row(
        "size_of::<Transaction>() [stack]",
        size_of::<Transaction>(),
        size_of::<Transaction>(),
    );
    row(
        "public key pair [heap]",
        size_of::<HybridPublicKey>(),
        PUBLIC_KEY_LEN,
    );
    row(
        "signature pair [heap]",
        size_of::<HybridSignature>(),
        SIGNATURE_LENGTH,
    );
    row(
        "signed transfer, encoded",
        {
            let mut tx = Transaction::new(
                vec![],
                vec![TxOutput {
                    amount: 1,
                    recipient: [0u8; 32],
                }],
                0,
            );
            tx.sign(&key(1)).expect("sign");
            tx.to_bytes().len()
        },
        // The same transfer without the hash-based half: subtract the SLH-DSA
        // key and signature, which is exactly what this change added to the
        // wire.
        {
            let mut tx = Transaction::new(
                vec![],
                vec![TxOutput {
                    amount: 1,
                    recipient: [0u8; 32],
                }],
                0,
            );
            tx.sign(&key(1)).expect("sign");
            tx.to_bytes().len()
                - (HYBRID_PUBLIC_KEY_LEN - PUBLIC_KEY_LEN)
                - (HYBRID_SIGNATURE_LENGTH - SIGNATURE_LENGTH)
        },
    );

    // --- a whole block in memory -----------------------------------------
    let sender = key(2);
    let recipient = key(3).address();

    let (block, build_peak, build_total) =
        measure(|| signed_block(&sender, &recipient, BLOCK_TRANSACTIONS));

    let encoded: usize = block
        .transactions
        .iter()
        .map(|tx| tx.to_bytes().len())
        .sum();

    println!("\nblock of {BLOCK_TRANSACTIONS} signed transfers (bytes)");
    println!("  {:<38} {encoded:>12}", "encoded size");
    println!(
        "  {:<38} {:>12}",
        "encoded per transaction",
        encoded / BLOCK_TRANSACTIONS
    );
    println!("  {:<38} {build_peak:>12}", "peak while building");
    println!(
        "  {:<38} {build_total:>12}",
        "total allocated while building"
    );

    // --- execution --------------------------------------------------------
    //
    // The figure that bounds a validating node's working set. Includes RocksDB's
    // write batch and the codec's temporaries, which are real costs of executing
    // a block even though they are not signature bytes.
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

    let (result, exec_peak, exec_total) =
        measure(|| db.apply_block(&block, BlockContext::at_height(1)));
    result.expect("apply block");

    // Read these as *incremental* over a block that is already resident. The
    // block was built and measured above; what these show is the extra working
    // set execution needs on top of it, which is the useful decomposition: the
    // signatures cost memory when the block arrives, not while it runs.
    println!("\napply_block (bytes, incremental over the resident block)");
    println!("  {:<38} {exec_peak:>12}", "peak live during execution");
    println!(
        "  {:<38} {exec_total:>12}",
        "total allocated during execution"
    );
    println!(
        "  {:<38} {:>12}",
        "peak per transaction",
        exec_peak / BLOCK_TRANSACTIONS
    );
    println!(
        "  {:<38} {:>12}",
        "resident block + execution peak",
        encoded + exec_peak
    );

    // --- what it means for block capacity ---------------------------------
    let per_tx = encoded / BLOCK_TRANSACTIONS;
    println!("\ncapacity against the 8 MiB gossip ceiling");
    println!(
        "  {:<38} {:>12}",
        "hybrid transactions per message",
        MAX_GOSSIP_MESSAGE_BYTES / per_tx
    );
    println!(
        "  {:<38} {:>12}",
        "ML-DSA-only transactions per message",
        MAX_GOSSIP_MESSAGE_BYTES
            / (per_tx
                - (HYBRID_PUBLIC_KEY_LEN - PUBLIC_KEY_LEN)
                - (HYBRID_SIGNATURE_LENGTH - SIGNATURE_LENGTH))
    );
    println!();
}
