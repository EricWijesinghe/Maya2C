//! What the block builder's per-frame staging costs as a block grows
//! (V4-V1 finding M3).
//!
//! `StateDB::select_applicable_within` stages a plain transfer in place and
//! undoes it on failure, but stages every other frame on a copy of the whole
//! overlay. A copy per frame is linear in the state the block has touched so
//! far, so a block of such frames is quadratic. This times one block of
//! plain transfers and one of sponsored (v13) transfers at several sizes,
//! every frame from its own sender to its own recipient, so the overlay
//! grows with the block as it does on a live chain. Signatures are verified
//! once before timing (the cache), so the figures are staging alone.
//!
//! ```text
//! cargo run --profile perf -p custom-l1-node --example builder_copy_cost
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use primitive_types::U256;
use std::time::Instant;

use custom_l1_node::core::Transaction;
use custom_l1_node::core::transaction::{ChainTag, TxOutput};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::fees::{FEE_COLLECTOR, FEE_KEY, FeeRecord};
use custom_l1_node::state::{Account, BlockContext, StateDB};
use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite};
use maya_dex::Amount;

const SIZES: &[u32] = &[250, 500, 1_000, 2_000];
/// The fastest of this many runs: the machine is shared, so the minimum is
/// the figure least inflated by other load.
const REPEATS: u32 = 7;
const HEIGHT: u64 = 10;
const BASE_FEE: u64 = 2;
const CHAIN: ChainTag = ChainTag::from_genesis([42; 32]);
const FUNDS: u64 = 1_000_000_000;

fn key(i: u32, salt: u8) -> <MlDsa65 as SignatureSuite>::SigningKey {
    let mut seed = [salt; 32];
    seed[..8].copy_from_slice(&u64::from(i).to_le_bytes());
    MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes(seed))
}

fn recipient(i: u32) -> [u8; 32] {
    let mut address = [0xEE; 32];
    address[..8].copy_from_slice(&u64::from(i).to_le_bytes());
    address
}

/// A signed v7 transfer of 1 unit; `fee` adds a collector output.
fn transfer(i: u32, fee: Option<U256>) -> Transaction {
    let mut outputs = vec![TxOutput {
        amount: U256::one(),
        recipient: recipient(i),
    }];
    if let Some(fee) = fee {
        outputs.push(TxOutput {
            amount: fee,
            recipient: FEE_COLLECTOR,
        });
    }
    let mut tx = Transaction::new(vec![], outputs, 0);
    tx.sign_with_suite::<MlDsa65>(&key(i, 0x11), &CHAIN)
        .unwrap();
    tx
}

fn required(tx: &Transaction) -> U256 {
    U256::from(BASE_FEE) * U256::from(tx.to_bytes().len())
}

fn plain(i: u32) -> Transaction {
    // A fee's value does not change the frame's size.
    transfer(i, Some(required(&transfer(i, Some(U256::zero())))))
}

fn sponsored(i: u32) -> Transaction {
    let mut tx = transfer(i, None);
    tx.sponsor_with::<MlDsa65>(&key(0, 0x22), U256::from(FUNDS), HEIGHT + 100, &CHAIN)
        .unwrap();
    tx
}

fn state(frames: &[Transaction]) -> (tempfile::TempDir, StateDB) {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    db.bind_chain(CHAIN).unwrap();
    let fees = FeeRecord {
        base_fee: Amount::from_u64(BASE_FEE),
        min_base_fee: Amount::from_u64(BASE_FEE),
        target_block_bytes: 1_000_000_000,
        change_denominator: 8,
    };
    db.raw_put_for_test(FEE_KEY, &fees.encode()).unwrap();
    let funded = Account {
        balance: U256::from(FUNDS),
        nonce: 0,
    };
    for tx in frames {
        db.put_account(&tx.sender(), &funded).unwrap();
        if let Some(sponsorship) = &tx.sponsor {
            db.put_account(&sponsorship.address(), &funded).unwrap();
        }
    }
    (dir, db)
}

/// Milliseconds to select a block of `frames` (fastest of [`REPEATS`]), all
/// of which must be kept.
fn select_ms(frames: &[Transaction]) -> f64 {
    let (_dir, db) = state(frames);
    let context = BlockContext::at_height(HEIGHT).with_sponsored_fees_activation(HEIGHT);
    let target = target_from_leading_zero_bits(0);
    let select = || {
        db.select_applicable_within(frames.to_vec(), context, target, u64::MAX)
            .unwrap()
    };
    assert_eq!(select().len(), frames.len(), "every frame is kept");
    (0..REPEATS)
        .map(|_| {
            let started = Instant::now();
            select();
            started.elapsed().as_secs_f64() * 1e3
        })
        .fold(f64::INFINITY, f64::min)
}

fn main() {
    for &n in SIZES {
        let plain_frames: Vec<Transaction> = (0..n).map(plain).collect();
        let sponsored_frames: Vec<Transaction> = (0..n).map(sponsored).collect();
        let (p, s) = (select_ms(&plain_frames), select_ms(&sponsored_frames));
        let per = |ms: f64| ms * 1e3 / f64::from(n);
        println!(
            "{n:>4} frames: plain {p:8.2} ms ({:6.1} µs/frame), sponsored {s:8.2} ms ({:6.1} µs/frame)",
            per(p),
            per(s)
        );
    }
}
