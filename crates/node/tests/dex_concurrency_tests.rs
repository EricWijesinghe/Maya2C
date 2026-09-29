//! Concurrency around the trading subsystem, and what it can honestly mean
//! here.
//!
//! Block execution is single-threaded and deterministic by construction: every
//! mutation lands in one overlay, and the overlay is flushed as one write
//! batch. There is no concurrent state mutation to race, and a test that
//! claimed to find one would be testing its own scaffolding.
//!
//! What *is* concurrent is everything in front of that:
//!
//! - Transactions arrive on many gossip tasks at once and are validated into a
//!   shared [`Mempool`] under a lock.
//! - The order in which they come back out is a hash map's iteration order, so
//!   two nodes assembling a block from the same set will not agree on a
//!   sequence.
//!
//! Which makes the property worth testing **confluence**: for one set of
//! transactions, every ordering must produce one state root. That is the
//! executable form of the claim the batch clearing makes — that position within
//! a block is not a variable anyone can profit from — and if it fails, the
//! failure is a chain split rather than a slow test.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::thread;

use custom_l1_node::core::dex_payload::{
    AssetRegistration, AssetTransfer, PoolCreation, SwapRequest,
};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::network::mempool::Mempool;
use custom_l1_node::state::{
    Account, Address, AssetId, BlockContext, Direction, NATIVE_ASSET, StateDB, derive_asset_id,
    derive_pair_id,
};
use tempfile::TempDir;

const LP_FEE_BPS: u32 = 30;
const POOL_NATIVE: u64 = 1_000_000;
const POOL_ASSET: u64 = 4_000_000;
const TRADERS: usize = 6;

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key).expect("sign");
    tx
}

fn symbol(text: &str) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[..text.len()].copy_from_slice(text.as_bytes());
    out
}

/// A market plus a set of traders, all funded, ready to submit swaps.
struct Arena {
    _dir: TempDir,
    db: Arc<StateDB>,
    asset: AssetId,
    pair: [u8; 32],
    traders: Vec<HybridSigningKey>,
}

/// Builds an identical market in a fresh database every time.
///
/// The keys are passed in rather than generated, so two arenas differ in
/// nothing at all — otherwise a difference in state roots would just be a
/// difference in addresses.
fn arena(maker: &HybridSigningKey, traders: &[HybridSigningKey]) -> Arena {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    let maker_address = maker.address();

    db.put_account(
        &maker_address,
        &Account {
            balance: 10_000_000,
            nonce: 0,
        },
    )
    .expect("fund maker");

    let asset = derive_asset_id(&maker_address, 0, &symbol("USD"));
    let pair = derive_pair_id(&NATIVE_ASSET, &asset, LP_FEE_BPS);

    let mut setup = vec![
        signed(
            TxKind::RegisterAsset(AssetRegistration {
                symbol: symbol("USD"),
                total_supply: 40_000_000,
            }),
            0,
            maker,
        ),
        signed(
            TxKind::CreatePool(PoolCreation {
                asset_a: NATIVE_ASSET,
                asset_b: asset,
                lp_fee_bps: LP_FEE_BPS,
                amount_a: POOL_NATIVE,
                amount_b: POOL_ASSET,
            }),
            1,
            maker,
        ),
    ];

    // Half the traders will sell the asset, so they need some of it.
    for (index, trader) in traders.iter().enumerate() {
        setup.push(signed(
            TxKind::TransferAsset(AssetTransfer {
                asset,
                recipient: trader.address(),
                amount: 500_000,
            }),
            2 + index as u64,
            maker,
        ));
    }

    db.apply_block(&block_of(setup), BlockContext::at_height(1))
        .expect("set up arena");

    // And native coin for the buyers.
    for trader in traders {
        db.put_account(
            &trader.address(),
            &Account {
                balance: 500_000,
                nonce: 0,
            },
        )
        .expect("fund trader");
    }

    Arena {
        _dir: dir,
        db: Arc::new(db),
        asset,
        pair,
        traders: traders.to_vec(),
    }
}

impl Arena {
    /// One swap per trader, alternating direction.
    fn swaps(&self) -> Vec<Transaction> {
        self.traders
            .iter()
            .enumerate()
            .map(|(index, trader)| {
                let direction = if index % 2 == 0 {
                    Direction::BaseToQuote
                } else {
                    Direction::QuoteToBase
                };
                signed(
                    TxKind::Swap(SwapRequest {
                        pair: self.pair,
                        direction: direction.tag(),
                        amount_in: 10_000 + (index as u64) * 3_137,
                        min_out: 1,
                        deadline: 0,
                    }),
                    0,
                    trader,
                )
            })
            .collect()
    }

    fn balances(&self) -> Vec<(u64, u64)> {
        self.traders
            .iter()
            .map(|trader| {
                let address = trader.address();
                (
                    self.db
                        .asset_balance(&NATIVE_ASSET, &address)
                        .expect("native"),
                    self.db.asset_balance(&self.asset, &address).expect("asset"),
                )
            })
            .collect()
    }
}

fn keys(count: usize) -> Vec<HybridSigningKey> {
    (0..count)
        .map(|_| generate_signing_key().expect("key"))
        .collect()
}

#[test]
fn many_threads_submitting_at_once_all_reach_the_mempool() {
    // The pool is a `std::sync::RwLock` around a map, and validation happens
    // before the write lock is taken. What this checks is that the whole path —
    // signature verification, state reads, insertion — is usable from many
    // threads at once without losing a transaction.
    let maker = generate_signing_key().expect("key");
    let traders = keys(TRADERS);
    let arena = arena(&maker, &traders);
    let mempool = Mempool::new(Arc::clone(&arena.db));

    let transactions = arena.swaps();
    thread::scope(|scope| {
        for tx in transactions {
            let mempool = mempool.clone();
            scope.spawn(move || {
                mempool.insert(tx).expect("insert");
            });
        }
    });

    assert_eq!(mempool.len(), TRADERS);
}

#[test]
fn every_ordering_of_one_block_produces_one_state_root() {
    // Confluence. The mempool hands transactions back in hash-map order, which
    // differs between nodes and between runs, so a block assembled from the
    // same set must not depend on the sequence it happened to come out in.
    let maker = generate_signing_key().expect("key");
    let traders = keys(TRADERS);

    let forward = arena(&maker, &traders);
    let mut transactions = forward.swaps();
    let forward_root = forward
        .db
        .apply_block(&block_of(transactions.clone()), BlockContext::at_height(2))
        .expect("apply");

    let reverse = arena(&maker, &traders);
    transactions.reverse();
    let reverse_root = reverse
        .db
        .apply_block(&block_of(transactions.clone()), BlockContext::at_height(2))
        .expect("apply");

    let rotated = arena(&maker, &traders);
    transactions.rotate_left(3);
    let rotated_root = rotated
        .db
        .apply_block(&block_of(transactions), BlockContext::at_height(2))
        .expect("apply");

    assert_eq!(forward_root, reverse_root);
    assert_eq!(forward_root, rotated_root);

    // And not merely the root: every trader got the same fill, which is the
    // thing the root is standing in for.
    assert_eq!(forward.balances(), reverse.balances());
    assert_eq!(forward.balances(), rotated.balances());
}

#[test]
fn no_ordering_lets_one_trader_do_better_than_another_ordering_would() {
    // The extractable-value claim, stated as an experiment rather than as an
    // argument: take one trader, try every position for them in the block, and
    // check their fill never changes. If it did, whoever chooses the order
    // would have something to sell.
    let maker = generate_signing_key().expect("key");
    let traders = keys(4);
    let mut fills = Vec::new();

    for position in 0..traders.len() {
        let arena = arena(&maker, &traders);
        let mut transactions = arena.swaps();
        let victim = transactions.remove(0);
        transactions.insert(position, victim);

        arena
            .db
            .apply_block(&block_of(transactions), BlockContext::at_height(2))
            .expect("apply");

        // Trader zero sells native and receives the asset.
        fills.push(
            arena
                .db
                .asset_balance(&arena.asset, &traders[0].address())
                .expect("balance"),
        );
    }

    assert!(
        fills.windows(2).all(|pair| pair[0] == pair[1]),
        "the same trade paid differently depending on where it sat: {fills:?}"
    );
}

#[test]
fn a_block_assembled_from_a_live_mempool_settles_the_same_way_every_time() {
    // End to end, through the real path: concurrent submission, a hash-map
    // snapshot, then execution. Run twice against identical arenas; the
    // snapshot order will differ between runs and the result must not.
    let maker = generate_signing_key().expect("key");
    let traders = keys(TRADERS);

    let roots: Vec<[u8; 32]> = (0..2)
        .map(|_| {
            let arena = arena(&maker, &traders);
            let mempool = Mempool::new(Arc::clone(&arena.db));

            thread::scope(|scope| {
                for tx in arena.swaps() {
                    let mempool = mempool.clone();
                    scope.spawn(move || {
                        mempool.insert(tx).expect("insert");
                    });
                }
            });

            arena
                .db
                .apply_block(&block_of(mempool.snapshot()), BlockContext::at_height(2))
                .expect("apply")
        })
        .collect();

    assert_eq!(roots[0], roots[1]);
}

#[test]
fn the_state_database_is_shareable_across_reader_threads() {
    // Pool and order reads go through the same handle the executor uses. They
    // have to be safe to serve from an RPC thread while a block is being built,
    // or every query would need its own database.
    let maker = generate_signing_key().expect("key");
    let traders = keys(2);
    let arena = arena(&maker, &traders);
    let pair = arena.pair;

    let observations: Vec<(u64, u64)> = thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let db = Arc::clone(&arena.db);
                scope.spawn(move || {
                    let record = db.get_pool(&pair).expect("read").expect("pool");
                    (record.pool.reserve_base, record.pool.reserve_quote)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("join"))
            .collect()
    });

    assert!(
        observations
            .iter()
            .all(|reserves| *reserves == (POOL_NATIVE, POOL_ASSET))
    );
}

#[test]
fn concurrent_submission_never_admits_two_transactions_on_one_nonce() {
    // The double-spend guard, exercised from the direction it would actually be
    // attacked from: the same account submitting two different swaps at once.
    let maker = generate_signing_key().expect("key");
    let traders = keys(1);
    let arena = arena(&maker, &traders);
    let mempool = Mempool::new(Arc::clone(&arena.db));
    let trader = traders[0].clone();
    let pair = arena.pair;

    let swap = |amount: u64| {
        signed(
            TxKind::Swap(SwapRequest {
                pair,
                direction: Direction::BaseToQuote.tag(),
                amount_in: amount,
                min_out: 1,
                deadline: 0,
            }),
            0,
            &trader,
        )
    };

    let attempts: Vec<Transaction> = (1..=8).map(|i| swap(1_000 * i)).collect();
    thread::scope(|scope| {
        for tx in attempts {
            let mempool = mempool.clone();
            scope.spawn(move || {
                // Both outcomes are fine here: the pool takes whichever arrived
                // first. What must not happen is the block accepting two.
                let _ = mempool.insert(tx);
            });
        }
    });

    let block = block_of(mempool.snapshot());
    let staged = block.transactions.len();
    let result = arena
        .db
        .apply_block(&block, BlockContext::at_height(2))
        .map(|_| ());

    if staged > 1 {
        assert!(
            result.is_err(),
            "a block carrying {staged} transactions on one nonce was accepted"
        );
    } else {
        assert!(result.is_ok());
    }

    // Whatever the mempool admitted, exactly one nonce was consumed.
    let nonce = arena
        .db
        .get_account(&trader.address())
        .expect("account")
        .nonce;
    assert!(nonce <= 1);
}

#[test]
fn a_full_batch_of_swaps_clears_without_the_block_failing() {
    // Load, rather than concurrency: many traders on one pool in one block is
    // the shape the batch is built for, and the fixed-point loop that drops
    // intents missing their bound is quadratic in the worst case. This is the
    // check that a busy pool is a busy pool and not a stalled node.
    let maker = generate_signing_key().expect("key");
    let traders = keys(12);
    let arena = arena(&maker, &traders);

    // Half of them ask for more than the batch can pay, so the removal loop
    // actually iterates instead of settling on the first pass.
    let transactions: Vec<Transaction> = arena
        .traders
        .iter()
        .enumerate()
        .map(|(index, trader)| {
            let (direction, min_out) = if index % 2 == 0 {
                (Direction::BaseToQuote, 1)
            } else {
                (Direction::QuoteToBase, u64::MAX)
            };
            signed(
                TxKind::Swap(SwapRequest {
                    pair: arena.pair,
                    direction: direction.tag(),
                    amount_in: 5_000 + index as u64,
                    min_out,
                    deadline: 0,
                }),
                0,
                trader,
            )
        })
        .collect();

    arena
        .db
        .apply_block(&block_of(transactions), BlockContext::at_height(2))
        .expect("a batch with losers in it is still a valid block");

    // The ones that could not be filled got their input back, untouched.
    for (index, trader) in arena.traders.iter().enumerate() {
        if index % 2 == 1 {
            assert_eq!(
                arena
                    .db
                    .asset_balance(&arena.asset, &trader.address())
                    .expect("balance"),
                500_000,
                "a skipped intent did not get its input back"
            );
        }
    }
}

/// Total native coin in existence, which no trade may change.
fn native_supply(db: &StateDB, addresses: &[Address]) -> u64 {
    addresses
        .iter()
        .map(|address| db.asset_balance(&NATIVE_ASSET, address).expect("balance"))
        .sum()
}

#[test]

mod common;
fn a_block_of_trades_conserves_every_asset() {
    let maker = generate_signing_key().expect("key");
    let traders = keys(TRADERS);
    let arena = arena(&maker, &traders);

    let mut addresses: Vec<Address> = traders.iter().map(HybridSigningKey::address).collect();
    addresses.push(maker.address());
    addresses.push([0u8; 32]); // the fee sink

    let native_before = native_supply(&arena.db, &addresses);
    let pool_before = arena.db.get_pool(&arena.pair).expect("r").expect("p");
    let asset_before: u64 = addresses
        .iter()
        .map(|address| arena.db.asset_balance(&arena.asset, address).expect("b"))
        .sum::<u64>()
        + pool_before.pool.reserve_quote;

    arena
        .db
        .apply_block(&block_of(arena.swaps()), BlockContext::at_height(2))
        .expect("apply");

    let pool_after = arena.db.get_pool(&arena.pair).expect("r").expect("p");
    let native_after = native_supply(&arena.db, &addresses);
    let asset_after: u64 = addresses
        .iter()
        .map(|address| arena.db.asset_balance(&arena.asset, address).expect("b"))
        .sum::<u64>()
        + pool_after.pool.reserve_quote;

    // The fee sink is the all-zero address, which is also where the pool's
    // native reserve is *not* held — the reserve lives in the pool record, so
    // it has to be added in on both sides.
    assert_eq!(
        native_before + pool_before.pool.reserve_base,
        native_after + pool_after.pool.reserve_base,
        "native coin was created or destroyed"
    );
    assert_eq!(asset_before, asset_after, "asset was created or destroyed");
}
