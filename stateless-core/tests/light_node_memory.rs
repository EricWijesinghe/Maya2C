//! A stateless light node validates 10,000 state transitions in under 10 MiB.
//!
//! ## What is measured
//!
//! Peak *additional* live heap during validation, counted by a global
//! allocator, above whatever was live when validation began. Not RSS: the
//! binary, the stack and the allocator's own slack are not in it. Setup — the
//! full-state reference that produces the blocks and witnesses — runs first,
//! writes everything to a file, and is dropped before measurement starts, so
//! the validator holds only a 32-byte root between blocks and one block's
//! witness at a time.
//!
//! Signatures are not in this test: this crate executes transfers whose
//! senders a caller has already authenticated. The signed version, through
//! the node's types, is `light-client/tests/stateless_memory.rs`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

use maya_stateless_core::{AccountState, Blake3, Key, PartialTree, apply_transfer, sparse};

const TRANSITIONS: usize = 10_000;
const TRANSFERS_PER_BLOCK: usize = 100;
const ACCOUNTS: u64 = 10_000;
const MEMORY_BUDGET: usize = 10 * 1024 * 1024;
const TRANSFER_BYTES: usize = 32 + 8 + 32 + 8;

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

fn key(index: u64) -> Key {
    *blake3::hash(&index.to_le_bytes()).as_bytes()
}

struct Transfer {
    sender: Key,
    recipient: Key,
    amount: u64,
}

/// Writes every block as: post root, transfer count, transfers, witness.
///
/// Returns the genesis root. Every fourth transfer pays an account that does
/// not exist yet, so the witnesses carry non-membership openings and inserts,
/// not only balance updates.
fn write_chain(file: &mut impl Write) -> [u8; 32] {
    let mut state: BTreeMap<Key, AccountState> = (0..ACCOUNTS)
        .map(|index| {
            (
                key(index),
                AccountState {
                    balance: 1_000_000,
                    nonce: 0,
                },
            )
        })
        .collect();
    let leaves = |state: &BTreeMap<Key, AccountState>| -> Vec<([u8; 32], [u8; 16])> {
        state.iter().map(|(k, v)| (*k, v.encode())).collect()
    };
    let genesis = sparse::root(&Blake3, &leaves(&state)).expect("root");

    let mut next_fresh = ACCOUNTS;
    for block in 0..TRANSITIONS / TRANSFERS_PER_BLOCK {
        let transfers: Vec<Transfer> = (0..TRANSFERS_PER_BLOCK)
            .map(|slot| {
                let index = (block * TRANSFERS_PER_BLOCK + slot) as u64;
                let sender = key(index % ACCOUNTS);
                let recipient = if index.is_multiple_of(4) {
                    next_fresh += 1;
                    key(next_fresh)
                } else {
                    key((index * 7 + 3) % ACCOUNTS)
                };
                Transfer {
                    sender,
                    recipient,
                    amount: 1 + index % 5,
                }
            })
            .collect();

        let touched: Vec<Key> = transfers
            .iter()
            .flat_map(|t| [t.sender, t.recipient])
            .collect();
        let witness = sparse::open(&Blake3, &leaves(&state), &touched).expect("open");

        let mut body = Vec::with_capacity(transfers.len() * TRANSFER_BYTES);
        for transfer in &transfers {
            let sender = state.entry(transfer.sender).or_default();
            sender.balance -= transfer.amount;
            let nonce = sender.nonce;
            sender.nonce += 1;
            state.entry(transfer.recipient).or_default().balance += transfer.amount;
            body.extend_from_slice(&transfer.sender);
            body.extend_from_slice(&nonce.to_le_bytes());
            body.extend_from_slice(&transfer.recipient);
            body.extend_from_slice(&transfer.amount.to_le_bytes());
        }

        let mut encoded = Vec::new();
        witness.encode::<Blake3>(&mut encoded);
        let post = sparse::root(&Blake3, &leaves(&state)).expect("root");

        file.write_all(&post).expect("write");
        file.write_all(&(transfers.len() as u32).to_le_bytes())
            .expect("write");
        file.write_all(&body).expect("write");
        file.write_all(&(encoded.len() as u32).to_le_bytes())
            .expect("write");
        file.write_all(&encoded).expect("write");
    }
    genesis
}

fn read_u32(reader: &mut impl Read) -> Option<usize> {
    let mut bytes = [0u8; 4];
    reader.read_exact(&mut bytes).ok()?;
    Some(u32::from_le_bytes(bytes) as usize)
}

/// The light node: a root, a reader, two reused buffers.
fn validate(reader: &mut impl Read, genesis: [u8; 32]) -> usize {
    let mut root = genesis;
    let mut body = Vec::new();
    let mut witness = Vec::new();
    let mut applied = 0;
    let mut post = [0u8; 32];

    while reader.read_exact(&mut post).is_ok() {
        let count = read_u32(reader).expect("count");
        body.resize(count * TRANSFER_BYTES, 0);
        reader.read_exact(&mut body).expect("body");
        witness.resize(read_u32(reader).expect("witness length"), 0);
        reader.read_exact(&mut witness).expect("witness");

        let mut tree = PartialTree::decode(&Blake3, &witness)
            .expect("decode")
            .verify(&Blake3, |digest| *digest == root)
            .expect("witness matches the parent root");

        for record in body.as_chunks::<TRANSFER_BYTES>().0 {
            let (sender, rest) = record.split_at(32);
            let (nonce, rest) = rest.split_at(8);
            let (recipient, amount) = rest.split_at(32);
            let recipient: Key = recipient.try_into().expect("key");
            let amount = u64::from_le_bytes(amount.try_into().expect("amount"));
            apply_transfer(
                &mut tree,
                sender.try_into().expect("key"),
                u64::from_le_bytes(nonce.try_into().expect("nonce")),
                [(recipient, amount)],
            )
            .expect("valid transfer");
            applied += 1;
        }

        root = tree.digest(&Blake3);
        assert_eq!(root, post, "block {} root", applied / TRANSFERS_PER_BLOCK);
    }
    applied
}

#[test]
fn a_stateless_light_node_validates_ten_thousand_transitions_in_under_ten_mib() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("chain.bin");
    let genesis = {
        let mut writer = BufWriter::new(File::create(&path).expect("create"));
        let genesis = write_chain(&mut writer);
        writer.flush().expect("flush");
        genesis
    };

    let mut reader = BufReader::new(File::open(&path).expect("open"));
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let applied = validate(&mut reader, genesis);
    let peak = PEAK.load(Ordering::Relaxed) - baseline;

    assert_eq!(applied, TRANSITIONS);
    assert!(peak < MEMORY_BUDGET, "peak validation heap {peak} bytes");
}
