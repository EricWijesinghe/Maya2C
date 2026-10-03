//! A stateless light node validates 10,000 signed transfers in under 10 MiB.
//!
//! The same claim as `crates/stateless-core/tests/light_node_memory.rs`, through the
//! node's real types: hybrid-signed transactions, blocks with a `tx_root`,
//! state roots folded over their layers, and witnesses a `StateDB` produced.
//!
//! ## What is measured
//!
//! Peak additional live heap during validation, above what was live when it
//! began, counted by a global allocator. Not RSS. The full node that built the
//! chain has been dropped by then; RocksDB's C++ allocations never pass through
//! this allocator in any case. Validation reads one length-prefixed block and
//! its witness at a time from a file. Proof of work is not checked — that is
//! the header chain's job, and its DAG cache alone is larger than the budget.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

use custom_l1_node::core::ChainTag;
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB, StateWitness};
use maya_light_client::StatelessValidator;

/// The chain every test signature commits to (ADR-036).
const CHAIN: ChainTag = ChainTag::from_genesis([0x5A; 32]);

const SENDERS: u64 = 100;
const BLOCKS: u64 = 100;
const FUNDED_RECIPIENTS: u64 = 900;
const TRANSITIONS: usize = (SENDERS * BLOCKS) as usize;
const MEMORY_BUDGET: usize = 10 * 1024 * 1024;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Counts live bytes. Relaxed ordering: diagnostics, not synchronization.
struct Counting;

// SAFETY: every method forwards to `System` unchanged and returns what it
// returns; the counters are `AtomicUsize` updates that touch no memory the
// allocator owns. So this allocator is exactly as sound as the system one.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` is forwarded unchanged to the system allocator,
        // and the pointer it returns is returned unchanged to the caller.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
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

fn context(height: u64) -> BlockContext {
    BlockContext::at_height(height).with_stateless_activation(1)
}

fn sender_key(index: u64) -> HybridSigningKey {
    let mut seed = [0u8; 32];
    seed[..8].copy_from_slice(&index.to_le_bytes());
    signing_key_from_seed(&seed).expect("derive")
}

fn account_address(tag: u8, index: u64) -> Address {
    let mut input = [tag; 9];
    input[1..].copy_from_slice(&index.to_le_bytes());
    *blake3::hash(&input).as_bytes()
}

/// One transfer per sender, signed across every core. Every fourth pays an
/// account that does not exist yet.
fn signed_block(keys: &[HybridSigningKey], block: u64) -> Vec<Transaction> {
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let chunk = keys.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let handles: Vec<_> = keys
            .chunks(chunk)
            .enumerate()
            .map(|(offset, chunk_keys)| {
                scope.spawn(move || {
                    chunk_keys
                        .iter()
                        .enumerate()
                        .map(|(position, key)| {
                            let slot = block * SENDERS + (offset * chunk + position) as u64;
                            let recipient = if slot.is_multiple_of(4) {
                                account_address(0xFE, slot)
                            } else {
                                account_address(0xFD, slot % FUNDED_RECIPIENTS)
                            };
                            let output = TxOutput {
                                amount: 1 + slot % 3,
                                recipient,
                            };
                            let mut tx = Transaction::new(vec![], vec![output], block);
                            tx.sign(key, &CHAIN).expect("sign");
                            tx
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("signer"))
            .collect()
    })
}

fn next_header(parent: &BlockHeader) -> BlockHeader {
    BlockHeader {
        prev_hash: parent.id(),
        state_root: [0; 32],
        timestamp: parent.timestamp + 1,
        nonce: 0,
        difficulty_target: parent.difficulty_target,
        tx_root: [0; 32],
    }
}

fn write_frame(file: &mut impl Write, bytes: &[u8]) {
    file.write_all(&(bytes.len() as u64).to_le_bytes())
        .expect("write");
    file.write_all(bytes).expect("write");
}

/// Builds the chain with a full node and writes `(block, witness)` frames.
/// Returns the activation header the light node anchors to.
fn write_chain(directory: &std::path::Path, out: &mut impl Write) -> BlockHeader {
    let db = StateDB::open(directory.join("state")).expect("open");
    db.bind_chain(CHAIN).expect("bind the test chain");
    let keys: Vec<HybridSigningKey> = (0..SENDERS).map(sender_key).collect();
    for key in &keys {
        db.put_account(
            &key.address(),
            &Account {
                balance: 1_000_000,
                nonce: 0,
            },
        )
        .expect("fund");
    }
    for index in 0..FUNDED_RECIPIENTS {
        db.put_account(
            &account_address(0xFD, index),
            &Account {
                balance: 1,
                nonce: 0,
            },
        )
        .expect("fund");
    }

    let genesis = BlockHeader {
        prev_hash: [0; 32],
        state_root: db.state_root().expect("root"),
        timestamp: 1_756_252_800,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    };
    let draft = Block::new(next_header(&genesis), vec![]);
    let root = db.apply_block(&draft, context(1)).expect("activate");
    let mut tip = BlockHeader {
        state_root: root,
        ..draft.header
    };
    let anchor = tip.clone();

    for block in 0..BLOCKS {
        let draft = Block::new(next_header(&tip), signed_block(&keys, block));
        let witness = db.block_witness(&draft).expect("witness");
        let root = db.apply_block(&draft, context(block + 2)).expect("apply");
        let sealed = Block::new(
            BlockHeader {
                state_root: root,
                ..draft.header
            },
            draft.transactions,
        );
        write_frame(out, &sealed.to_bytes());
        write_frame(out, &witness.encode());
        tip = sealed.header;
    }
    anchor
}

fn read_frame(reader: &mut impl Read, buffer: &mut Vec<u8>) -> bool {
    let mut length = [0u8; 8];
    if reader.read_exact(&mut length).is_err() {
        return false;
    }
    buffer.resize(u64::from_le_bytes(length) as usize, 0);
    reader.read_exact(buffer).expect("frame");
    true
}

fn validate(reader: &mut impl Read, anchor: &BlockHeader) -> usize {
    let mut validator = StatelessValidator::new(anchor, 1, CHAIN);
    let (mut block_bytes, mut witness_bytes) = (Vec::new(), Vec::new());
    let mut applied = 0;
    while read_frame(reader, &mut block_bytes) {
        assert!(
            read_frame(reader, &mut witness_bytes),
            "block without a witness"
        );
        let block = Block::from_bytes(&block_bytes).expect("block");
        let witness = StateWitness::decode(&witness_bytes).expect("witness");
        validator.validate(&block, witness).expect("valid block");
        applied += block.transactions.len();
    }
    assert_eq!(validator.height(), BLOCKS + 1);
    applied
}

#[test]
#[ignore = "signs 10,000 SLH-DSA transfers: about four minutes; run with --run-ignored all"]
fn a_stateless_light_node_validates_ten_thousand_signed_transfers_in_under_ten_mib() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("chain.bin");
    let anchor = {
        let mut writer = BufWriter::new(File::create(&path).expect("create"));
        let anchor = write_chain(directory.path(), &mut writer);
        writer.flush().expect("flush");
        anchor
    };

    let mut reader = BufReader::new(File::open(&path).expect("open"));
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let applied = validate(&mut reader, &anchor);
    let peak = PEAK.load(Ordering::Relaxed) - baseline;

    assert_eq!(applied, TRANSITIONS);
    assert!(peak < MEMORY_BUDGET, "peak validation heap {peak} bytes");
}
