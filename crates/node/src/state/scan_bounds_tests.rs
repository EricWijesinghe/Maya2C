//! Per-block work against stored state: what each end-of-block pass and the
//! state root cost as the records they read grow, on the test profile.
//!
//! In-crate because the passes are `pub(crate)`. Every timing test is
//! `#[ignore]`d: run them with
//! `cargo test -p custom-l1-node --lib scan_bounds_tests -- --ignored --nocapture --test-threads=1`.
//! The figures are debug-crate figures (node at opt-level 0, dependencies at
//! opt-level 3) on a shared machine, and say how the cost grows, not what a
//! release node pays.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation
)]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use maya_dex::Amount;
use maya_dex::amm::Pool;
use maya_dex::book::{Order, Side};
use maya_dex::fees::FeeSchedule;
use primitive_types::U256;
use tempfile::TempDir;

use crate::consensus::bft::builder::seal;
use crate::core::SealedEnvelope;
use crate::core::block::BlockHeader;
use crate::core::staking_payload::StakingAction;
use crate::error::NodeError;
use crate::sealed::{CommitteeRecord, EnvelopeRecord, MAX_SEALED_PER_REVEAL_HEIGHT, envelope_key};
use crate::state::account::Account;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};
use crate::state::dex::{OrderRecord, PoolRecord, order_count_key, order_key, pool_key};
use crate::state::dex_exec::MAX_BOOK_ORDERS_PER_BLOCK;
use crate::state::staking::{STATE_KEY, StakingRecord};
use crate::state::staking_exec::{MAX_STAKING_RECORD_BYTES, MAX_STAKING_TX_PER_BLOCK};

fn open() -> (StateDB, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    (StateDB::open(dir.path()).expect("open"), dir)
}

fn id_of(index: u64) -> [u8; 32] {
    *blake3::hash(&index.to_le_bytes()).as_bytes()
}

const RUNS: usize = 3;

fn median(mut run: impl FnMut() -> Duration) -> Duration {
    let mut samples: Vec<Duration> = (0..RUNS).map(|_| run()).collect();
    samples.sort();
    samples[RUNS / 2]
}

fn timed(f: impl FnOnce()) -> Duration {
    let start = Instant::now();
    f();
    start.elapsed()
}

// ------------------------------------------------------------- state root

/// The state root re-reads and re-hashes every account and every committed
/// record on every block (`StateDB::root_with_overlay`).
#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn state_root_cost_against_stored_state() {
    for n in [1_000_u64, 10_000, 100_000] {
        let (db, _dir) = open();
        for i in 0..n {
            let account = Account {
                balance: U256::one(),
                nonce: 0,
            };
            db.put_account(&id_of(i), &account).unwrap();
        }
        let t = median(|| {
            timed(|| {
                db.state_root().unwrap();
            })
        });
        println!("state-root accounts={n:>7}: median={t:?}");
    }
    for n in [1_000_u64, 10_000, 100_000] {
        let (db, _dir) = open();
        for i in 0..n {
            let key = [crate::governance::GOVERNANCE_PREFIX, &id_of(i)].concat();
            db.raw_put(&key, &[7; 200]).unwrap();
        }
        let t = median(|| {
            timed(|| {
                db.state_root().unwrap();
            })
        });
        println!("state-root records ={n:>7} (200 B each): median={t:?}");
    }
}

// ----------------------------------------------------------------- sealed

fn sealed_db(at_height: u64, envelopes: u64) -> (StateDB, TempDir) {
    let (db, dir) = open();
    db.seed_sealed_committee(&CommitteeRecord {
        encryption_key: [0; 32],
        threshold: 1,
        members: vec![(1, [0; 32])],
    })
    .unwrap();
    for i in 0..envelopes {
        let record = EnvelopeRecord {
            sender: id_of(i),
            ciphertext: vec![0; 256],
        };
        db.raw_put(&envelope_key(at_height, &id_of(i)), &record.encode())
            .unwrap();
    }
    (db, dir)
}

/// `settle_sealed` visits every envelope due at the height. With 128 a block
/// and a reveal delay of up to 256, 255 blocks can aim 32,640 at one height.
#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn sealed_settle_cost_against_one_reveal_height() {
    let height = 1_000;
    for n in [128_u64, 1_000, 10_000, 32_640] {
        let (db, _dir) = sealed_db(height, n);
        let t = median(|| {
            let mut overlay = Overlay::default();
            timed(|| {
                db.settle_sealed(&mut overlay, BlockContext::at_height(height))
                    .unwrap();
            })
        });
        println!("sealed-settle due={n:>6}: median={t:?}");
    }
}

/// One reveal height takes at most `MAX_SEALED_PER_REVEAL_HEIGHT` envelopes,
/// counting those committed by earlier blocks, and a full height leaves the
/// others open. Fails on the per-block cap alone, which accepted the
/// envelope past the height's cap.
#[test]
fn a_reveal_height_holds_at_most_its_cap() {
    let reveal = 1_000;
    let cap = MAX_SEALED_PER_REVEAL_HEIGHT as u64;
    let (db, _dir) = sealed_db(reveal, cap - 1);
    let context = BlockContext::at_height(900);
    let at = |reveal_height| SealedEnvelope {
        reveal_height,
        ciphertext: vec![0; 64],
    };
    let mut overlay = Overlay::default();

    db.stage_envelope(&mut overlay, &[1; 32], 0, &at(reveal), context)
        .expect("the height's last slot");
    let refused = db
        .stage_envelope(&mut overlay, &[1; 32], 1, &at(reveal), context)
        .expect_err("the height is full");
    assert!(
        matches!(refused, NodeError::SealedBlockLimit { .. }),
        "{refused:?}"
    );
    db.stage_envelope(&mut overlay, &[1; 32], 2, &at(reveal + 1), context)
        .expect("another height is unaffected");
}

// -------------------------------------------------------------------- dex

fn pair_of(index: u64) -> [u8; 32] {
    id_of(1_000_000 + index)
}

/// Writes a pool and `orders` resting asks that cross nothing.
fn seed_book(db: &StateDB, pair: &[u8; 32], orders: u64) {
    let pool = PoolRecord {
        base_asset: [1; 32],
        quote_asset: [2; 32],
        pool: Pool {
            reserve_base: Amount::from_u64(1_000_000),
            reserve_quote: Amount::from_u64(1_000_000),
            total_shares: Amount::from_u64(1_000_000),
            fees: FeeSchedule {
                lp_bps: 30,
                protocol_bps: 0,
            },
        },
    };
    db.raw_put(&pool_key(pair), &pool.encode()).unwrap();
    for i in 0..orders {
        let price = Amount::from_u64(1_000 + i);
        let order = Order {
            id: id_of(i ^ u64::from_le_bytes(pair[..8].try_into().unwrap())),
            owner: [3; 32],
            side: Side::Ask,
            price,
            amount: Amount::from_u64(10),
            remaining: Amount::from_u64(10),
            sequence: i,
            expiry: 0,
        };
        let record = OrderRecord {
            pair: *pair,
            order,
            escrow: Amount::from_u64(10),
        };
        db.raw_put(&order_key(pair, Side::Ask, price, i), &record.encode())
            .unwrap();
    }
}

/// `settle_trading` rebuilds the whole book of every pair the block touched,
/// whether or not anything crosses.
#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn dex_book_cost_against_resting_orders() {
    for (pairs, orders) in [(1_u64, 1_000_u64), (1, 4_096), (4, 4_096), (16, 4_096)] {
        let (db, _dir) = open();
        for p in 0..pairs {
            seed_book(&db, &pair_of(p), orders);
        }
        let t = median(|| {
            let mut overlay = Overlay::default();
            for p in 0..pairs {
                overlay.touched_pairs.insert(pair_of(p));
            }
            timed(|| {
                db.settle_trading(&mut overlay, BlockContext::at_height(5))
                    .unwrap();
            })
        });
        println!(
            "dex-settle touched_pairs={pairs:>3} orders_each={orders:>5}: median={t:?} ({:?}/order)",
            t / u32::try_from(pairs * orders).unwrap()
        );
    }
}

// ---------------------------------------------------------------- staking

fn staking_record(delegations: u64) -> StakingRecord {
    let mut staking = maya_staking::Staking::new(maya_staking::Params::DEVNET).unwrap();
    for i in 0..delegations {
        staking
            .delegations
            .insert((id_of(i), [9; 32]), Amount::from_u64(10));
    }
    staking.active = vec![[9; 32]];
    StakingRecord {
        staking,
        epoch_blocks: 3_600,
        last_round: 0,
        expected: BTreeMap::new(),
        authored: BTreeMap::new(),
    }
}

/// The staking record is one value: every staking transaction and every
/// block decodes and re-encodes it whole.
#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn staking_record_cost_against_delegations() {
    for n in [1_000_u64, 10_000, 87_000] {
        let bytes = staking_record(n).encode();
        let codec = median(|| timed(|| drop(StakingRecord::decode(&bytes).unwrap().encode())));
        let (db, _dir) = open();
        db.raw_put(STATE_KEY, &bytes).unwrap();
        let header = BlockHeader {
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 0,
            nonce: seal(0, 2),
            difficulty_target: [0xff; 32],
            tx_root: [0; 32],
            version: crate::core::block::HEADER_VERSION,
        };
        let settle = median(|| {
            let mut overlay = Overlay::default();
            timed(|| {
                db.settle_staking(&mut overlay, &header, BlockContext::at_height(1))
                    .unwrap();
            })
        });
        let mut overlay = Overlay::default();
        overlay
            .records
            .insert(STATE_KEY.to_vec(), Some(bytes.clone()));
        let clone = median(|| timed(|| drop(overlay.clone())));
        println!(
            "staking delegations={n:>6} record={:>8} B: decode+encode={codec:?} settle_staking={settle:?} overlay_clone={clone:?}",
            bytes.len()
        );
    }
}

/// `count_slots` walks every anchor slot a round gap skipped, up to
/// `MAX_SLOTS_PER_BLOCK`.
#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn staking_slot_count_at_its_cap() {
    let (db, _dir) = open();
    db.raw_put(STATE_KEY, &staking_record(0).encode()).unwrap();
    let header = BlockHeader {
        prev_hash: [0; 32],
        state_root: [0; 32],
        timestamp: 0,
        nonce: seal(0, 2_000_000),
        difficulty_target: [0xff; 32],
        tx_root: [0; 32],
        version: crate::core::block::HEADER_VERSION,
    };
    let t = median(|| {
        let mut overlay = Overlay::default();
        timed(|| {
            db.settle_staking(&mut overlay, &header, BlockContext::at_height(1))
                .unwrap();
        })
    });
    println!("staking count_slots slots=1000000: median={t:?}");
}

// ------------------------------------------------------------- behaviour

/// The books a block touches hold at most `MAX_BOOK_ORDERS_PER_BLOCK`
/// resting orders between them; the first book and empty books are always
/// admitted. Fails without the budget, which touched every pair.
#[test]
fn the_books_a_block_touches_stay_within_the_budget() {
    let (db, _dir) = open();
    let full = crate::state::dex_exec::MAX_ORDERS_PER_BOOK;
    for p in 0..5 {
        db.raw_put(&order_count_key(&pair_of(p)), &full.to_le_bytes())
            .unwrap();
    }
    let mut overlay = Overlay::default();
    for p in 0..4 {
        db.touch_pair(&mut overlay, &pair_of(p))
            .expect("within the budget");
    }
    assert_eq!(overlay.book_orders, MAX_BOOK_ORDERS_PER_BLOCK);
    let refused = db
        .touch_pair(&mut overlay, &pair_of(4))
        .expect_err("a fifth full book is past the budget");
    assert!(matches!(refused, NodeError::Trade { .. }), "{refused:?}");
    assert!(!overlay.touched_pairs.contains(&pair_of(4)));
    db.touch_pair(&mut overlay, &pair_of(0))
        .expect("a pair already touched costs nothing");
    db.touch_pair(&mut overlay, &pair_of(9))
        .expect("an empty book costs nothing");

    // A book larger than the whole budget is still reachable on its own.
    db.raw_put(
        &order_count_key(&pair_of(7)),
        &(MAX_BOOK_ORDERS_PER_BLOCK + 1).to_le_bytes(),
    )
    .unwrap();
    db.touch_pair(&mut Overlay::default(), &pair_of(7))
        .expect("the first book is always admitted");
}

fn staked_db(delegations: u64) -> (StateDB, TempDir) {
    let (db, dir) = open();
    let mut record = staking_record(delegations);
    record.staking.validators.insert(
        [9; 32],
        maya_staking::ValidatorRecord {
            operator: [8; 32],
            self_bond: Amount::from_u64(1_000),
            delegated: Amount::ZERO,
            commission_bps: 0,
            status: maya_staking::Status::Bonded,
        },
    );
    db.raw_put(STATE_KEY, &record.encode()).unwrap();
    (db, dir)
}

fn delegate(db: &StateDB, overlay: &mut Overlay, sender: u64) -> crate::error::Result<()> {
    let address = id_of(2_000_000 + sender);
    db.put_account(
        &address,
        &Account {
            balance: U256::from(1_000_u64),
            nonce: 0,
        },
    )
    .unwrap();
    db.apply_staking(
        overlay,
        &address,
        &StakingAction::Delegate {
            validator: [9; 32],
            amount: U256::from(10_u64),
        },
    )
}

/// A block carries at most `MAX_STAKING_TX_PER_BLOCK` staking transactions.
/// Fails without the count, which accepted every one.
#[test]
fn a_block_carries_at_most_its_staking_transactions() {
    let (db, _dir) = staked_db(0);
    let mut overlay = Overlay::default();
    for sender in 0..MAX_STAKING_TX_PER_BLOCK as u64 {
        delegate(&db, &mut overlay, sender).expect("within the count");
    }
    let refused = delegate(&db, &mut overlay, 99).expect_err("past the count");
    assert!(
        refused.to_string().contains("staking transactions"),
        "{refused}"
    );
    delegate(&db, &mut Overlay::default(), 99).expect("the next block takes it");
}

/// A delegation that would grow the record past `MAX_STAKING_RECORD_BYTES`
/// is refused. Fails at the old cap of 8 MiB.
#[test]
fn the_staking_record_stops_growing_at_its_cap() {
    let per_delegation = 96;
    let (db, _dir) = staked_db((MAX_STAKING_RECORD_BYTES / per_delegation) as u64);
    let refused = delegate(&db, &mut Overlay::default(), 0).expect_err("the record is full");
    assert!(refused.to_string().contains("record would be"), "{refused}");
}

/// `merged_records` as it was before `Overlay::records_under`: a filter over
/// every staged record. Kept to compare against.
fn merged_records_full_scan(
    db: &StateDB,
    overlay: &Overlay,
    prefix: &[u8],
) -> BTreeMap<Vec<u8>, Vec<u8>> {
    let mut merged: BTreeMap<Vec<u8>, Vec<u8>> =
        db.scan_prefix(prefix).unwrap().into_iter().collect();
    for (key, value) in &overlay.records {
        if !key.starts_with(prefix) {
            continue;
        }
        match value {
            Some(bytes) => merged.insert(key.clone(), bytes.clone()),
            None => merged.remove(key),
        };
    }
    merged
}

/// A small deterministic generator: the test must replay identically.
struct XorShift(u64);

impl XorShift {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % bound
    }

    /// A key over a four-byte alphabet, so prefixes nest and share bytes.
    fn key(&mut self) -> Vec<u8> {
        let len = 1 + self.below(5) as usize;
        (0..len)
            .map(|_| b"ab:\xff"[self.below(4) as usize])
            .collect()
    }
}

/// The ranged read returns what the full filter returned.
#[test]
fn the_ranged_merge_matches_the_full_scan() {
    let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
    for _ in 0..20 {
        let (db, _dir) = open();
        let mut overlay = Overlay::default();
        for _ in 0..60 {
            let key = rng.key();
            db.raw_put(&key, &[rng.below(256) as u8]).unwrap();
        }
        for _ in 0..60 {
            let key = rng.key();
            let value = (rng.below(3) != 0).then(|| vec![rng.below(256) as u8]);
            overlay.records.insert(key, value);
        }
        let mut prefixes: Vec<Vec<u8>> = (0..30).map(|_| rng.key()).collect();
        prefixes.push(Vec::new());
        for prefix in prefixes {
            assert_eq!(
                db.merged_records(&overlay, &prefix).unwrap(),
                merged_records_full_scan(&db, &overlay, &prefix),
                "prefix {prefix:?}"
            );
        }
    }
}
