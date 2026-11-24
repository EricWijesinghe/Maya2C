//! DEX fairness against real block execution (ADR-154, Eric 2026-11-24).
//!
//! Two attacks, each replayed as the attacker would run it:
//!
//! - **A sandwich around a route.** Routes used to execute in place, so a
//!   miner could put a route in front of a victim's route and a second one
//!   behind it, and the victim's own price impact paid for the round trip.
//!   Every pool now clears once per block, routes included, so there is no
//!   "in front".
//! - **One account, or a flood of them, filling a pair.** A pair's batch held
//!   256 intents and refused the 257th with a block-level error, so whoever
//!   filled it first locked every honest trader out for that block. A sender
//!   now holds at most a sixteenth of a batch, and a full batch evicts its
//!   cheapest intent for a better-paying one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::dex_payload::{
    AssetRegistration, AssetTransfer, PoolCreation, RouteLeg, SwapRequest, SwapRoute,
};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::fees::{FEE_COLLECTOR, FEE_KEY, FeeRecord};
use custom_l1_node::state::{
    Account, AssetId, BlockContext, Direction, NATIVE_ASSET, StateDB, derive_asset_id,
    derive_pair_id,
};
use maya_dex::Amount;
use primitive_types::U256;
use tempfile::TempDir;

mod common;

/// Most of a pair's batch one sender may hold: `MAX_BATCH_INTENTS / 16`.
/// A literal so this file also compiles against the code before ADR-154,
/// which is how its tests were seen to fail first.
const SHARE: u64 = 16;
/// A pair's batch: `maya_dex::batch::MAX_BATCH_INTENTS`.
const BATCH: u64 = 256;
const LP_FEE_BPS: u32 = 30;
const POOL_SIDE: u64 = 1_000_000_000;
const NATIVE_FUND: u64 = 1_000_000_000_000;

struct World {
    db: StateDB,
    _dir: TempDir,
    asset: AssetId,
    pair: [u8; 32],
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            version: 0,
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

/// A transaction from `key`, paying `fee` to the collector when given.
fn tx(kind: TxKind, nonce: u64, key: &HybridSigningKey, fee: Option<u64>) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    if let Some(fee) = fee {
        tx.outputs = vec![TxOutput {
            amount: U256::from(fee),
            recipient: FEE_COLLECTOR,
        }];
    }
    tx.sign(key, &common::test_chain()).expect("sign");
    tx
}

/// A funded native/USD pool, with `asset_holders` each given `asset_each` USD.
/// Every key in `funded` holds native coin.
fn world(
    maker: &HybridSigningKey,
    funded: &[&HybridSigningKey],
    asset_holders: &[&HybridSigningKey],
    asset_each: u64,
) -> World {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    common::bind(&db);
    for key in core::iter::once(maker).chain(funded.iter().copied()) {
        db.put_account(
            &key.address(),
            &Account {
                balance: U256::from(NATIVE_FUND),
                nonce: 0,
            },
        )
        .expect("fund");
    }
    let mut symbol = [0u8; 8];
    symbol[..3].copy_from_slice(b"USD");
    let asset = derive_asset_id(&maker.address(), 0, &symbol);
    let pair = derive_pair_id(&NATIVE_ASSET, &asset, LP_FEE_BPS);
    let mut setup = vec![
        tx(
            TxKind::RegisterAsset(AssetRegistration {
                symbol,
                total_supply: U256::exp10(18),
            }),
            0,
            maker,
            None,
        ),
        tx(
            TxKind::CreatePool(PoolCreation {
                asset_a: NATIVE_ASSET,
                asset_b: asset,
                lp_fee_bps: LP_FEE_BPS,
                amount_a: U256::from(POOL_SIDE),
                amount_b: U256::from(POOL_SIDE),
            }),
            1,
            maker,
            None,
        ),
    ];
    for (offset, holder) in asset_holders.iter().enumerate() {
        setup.push(tx(
            TxKind::TransferAsset(AssetTransfer {
                asset,
                recipient: holder.address(),
                amount: U256::from(asset_each),
            }),
            2 + offset as u64,
            maker,
            None,
        ));
    }
    db.apply_block(&block_of(setup), BlockContext::at_height(1))
        .expect("setup");
    World {
        db,
        _dir: dir,
        asset,
        pair,
    }
}

impl World {
    fn balance(&self, asset: &AssetId, who: &HybridSigningKey) -> i128 {
        let value = self.db.asset_balance(asset, &who.address()).expect("read");
        i128::try_from(u128::try_from(value).expect("fits")).expect("fits")
    }

    fn route(&self, direction: Direction, amount_in: u64) -> TxKind {
        TxKind::SwapRoute(SwapRoute {
            legs: vec![RouteLeg {
                pair: self.pair,
                direction: direction.tag(),
            }],
            amount_in: U256::from(amount_in),
            min_out: U256::one(),
            deadline: 0,
        })
    }

    fn swap(&self, amount_in: u64) -> TxKind {
        TxKind::Swap(SwapRequest {
            pair: self.pair,
            direction: Direction::BaseToQuote.tag(),
            amount_in: U256::from(amount_in),
            min_out: U256::one(),
            deadline: 0,
        })
    }

    /// Fees on from here: one native unit per byte.
    fn turn_fees_on(&self) {
        let record = FeeRecord {
            base_fee: Amount::from_u64(1),
            min_base_fee: Amount::from_u64(1),
            target_block_bytes: 1_000_000,
            change_denominator: 8,
        };
        self.db
            .raw_put_for_test(FEE_KEY, &record.encode())
            .expect("fees on");
    }
}

fn key() -> HybridSigningKey {
    generate_signing_key().expect("key")
}

// ---------------------------------------------------------------------------
// the route sandwich
// ---------------------------------------------------------------------------

/// Runs front / victim / back in that block order and returns the attacker's
/// combined net change (native, asset), plus the front and victim outputs.
fn sandwich(back_with_route: bool) -> ((i128, i128), (i128, i128)) {
    let (maker, front, victim, back) = (key(), key(), key(), key());
    let world = world(&maker, &[&front, &victim, &back], &[&back], POOL_SIDE);
    let size = POOL_SIDE / 10;

    // What the back leg must sell for the in-place attack to hold no net
    // asset: exactly the front leg's in-place output.
    let pool = world
        .db
        .get_pool(&world.pair)
        .expect("read")
        .expect("pool")
        .pool;
    let front_out = pool
        .swap_exact_in(Direction::BaseToQuote, Amount::from_u64(size))
        .expect("front")
        .amount_out
        .to_u64()
        .expect("small");

    let before = |who: &HybridSigningKey| {
        (
            world.balance(&NATIVE_ASSET, who),
            world.balance(&world.asset, who),
        )
    };
    let (f0, v0, b0) = (before(&front), before(&victim), before(&back));
    let back_kind = if back_with_route {
        world.route(Direction::QuoteToBase, front_out)
    } else {
        TxKind::Swap(SwapRequest {
            pair: world.pair,
            direction: Direction::QuoteToBase.tag(),
            amount_in: U256::from(front_out),
            min_out: U256::one(),
            deadline: 0,
        })
    };
    let block = block_of(vec![
        tx(world.route(Direction::BaseToQuote, size), 0, &front, None),
        tx(world.route(Direction::BaseToQuote, size), 0, &victim, None),
        tx(back_kind, 0, &back, None),
    ]);
    world
        .db
        .apply_block(&block, BlockContext::at_height(2))
        .expect("block");

    let (f1, v1, b1) = (before(&front), before(&victim), before(&back));
    let attacker = (f1.0 - f0.0 + b1.0 - b0.0, f1.1 - f0.1 + b1.1 - b0.1);
    (attacker, (f1.1 - f0.1, v1.1 - v0.1))
}

fn assert_no_riskless_profit((native, asset): (i128, i128)) {
    assert!(
        !((native > 0 && asset >= 0) || (native >= 0 && asset > 0)),
        "the sandwich paid: native {native:+}, asset {asset:+}"
    );
}

#[test]
fn a_route_cannot_be_sandwiched_by_two_routes_in_one_block() {
    let (attacker, (front_out, victim_out)) = sandwich(true);
    assert_no_riskless_profit(attacker);
    assert!(front_out > 0, "the attack must actually trade");
    assert_eq!(
        front_out, victim_out,
        "front-runner and victim get one price"
    );
}

#[test]
fn a_route_cannot_be_fronted_by_a_route_and_unwound_through_the_batch() {
    // The old doc's claim was that nothing after the routes could unwind a
    // front-run. The swap batch ran after them and could.
    let (attacker, (front_out, victim_out)) = sandwich(false);
    assert_no_riskless_profit(attacker);
    assert_eq!(
        front_out, victim_out,
        "front-runner and victim get one price"
    );
}

#[test]
fn the_order_of_routes_and_swaps_in_a_block_does_not_change_the_state_root() {
    let (maker, a, b, c) = (key(), key(), key(), key());
    let run = |order: &[usize]| {
        let world = world(&maker, &[&a, &b, &c], &[&c], POOL_SIDE);
        let txs = [
            tx(world.route(Direction::BaseToQuote, 70_000), 0, &a, None),
            tx(world.swap(30_000), 0, &b, None),
            tx(world.route(Direction::QuoteToBase, 50_000), 0, &c, None),
        ];
        let block = block_of(order.iter().map(|&i| txs[i].clone()).collect());
        world
            .db
            .apply_block(&block, BlockContext::at_height(2))
            .expect("block")
    };
    let first = run(&[0, 1, 2]);
    assert_eq!(first, run(&[2, 1, 0]));
    assert_eq!(first, run(&[1, 2, 0]));
}

#[test]
fn a_route_that_visits_a_pool_twice_is_refused() {
    let (maker, trader) = (key(), key());
    let world = world(&maker, &[&trader], &[], 0);
    let round_trip = TxKind::SwapRoute(SwapRoute {
        legs: vec![
            RouteLeg {
                pair: world.pair,
                direction: Direction::BaseToQuote.tag(),
            },
            RouteLeg {
                pair: world.pair,
                direction: Direction::QuoteToBase.tag(),
            },
        ],
        amount_in: U256::from(10_000),
        min_out: U256::one(),
        deadline: 0,
    });
    let error = world
        .db
        .apply_block(
            &block_of(vec![tx(round_trip, 0, &trader, None)]),
            BlockContext::at_height(2),
        )
        .unwrap_err();
    assert!(
        matches!(
            error,
            NodeError::Trade {
                operation: "route",
                ..
            }
        ),
        "{error:?}"
    );
}

// ---------------------------------------------------------------------------
// the 256-intent cap
// ---------------------------------------------------------------------------

#[test]
fn one_sender_cannot_hold_more_than_a_sixteenth_of_a_pair() {
    let (maker, flooder) = (key(), key());
    let world = world(&maker, &[&flooder], &[], 0);
    let block = block_of(
        (0..=SHARE)
            .map(|nonce| tx(world.swap(1_000), nonce, &flooder, None))
            .collect(),
    );
    let error = world
        .db
        .apply_block(&block, BlockContext::at_height(2))
        .unwrap_err();
    assert!(
        matches!(error, NodeError::TooManySwaps { limit, .. } if limit as u64 == SHARE),
        "{error:?}"
    );
}

/// Sixteen flood accounts holding the whole batch at `flood_fee` each, then
/// one honest swap paying `honest_fee`. Returns the honest trader's native
/// change net of the fee, their asset change, and the flood's asset change.
fn flood_then_honest(flood_fee: u64, honest_fee: u64) -> (i128, i128, i128) {
    let maker = key();
    let honest = key();
    let flooders: Vec<HybridSigningKey> = (0..BATCH / SHARE).map(|_| key()).collect();
    let mut funded: Vec<&HybridSigningKey> = flooders.iter().collect();
    funded.push(&honest);
    let world = world(&maker, &funded, &[], 0);
    world.turn_fees_on();

    let mut txs: Vec<Transaction> = flooders
        .iter()
        .flat_map(|flooder| {
            (0..SHARE).map(|nonce| tx(world.swap(1_000), nonce, flooder, Some(flood_fee)))
        })
        .collect();
    txs.push(tx(world.swap(1_000_000), 0, &honest, Some(honest_fee)));

    let native_before = world.balance(&NATIVE_ASSET, &honest);
    let flood_before: i128 = flooders
        .iter()
        .map(|f| world.balance(&world.asset, f))
        .sum();
    world
        .db
        .apply_block(&block_of(txs), BlockContext::at_height(2))
        .expect("a full batch never fails the block");
    let flood_after: i128 = flooders
        .iter()
        .map(|f| world.balance(&world.asset, f))
        .sum();
    (
        world.balance(&NATIVE_ASSET, &honest) - native_before + i128::from(honest_fee),
        world.balance(&world.asset, &honest),
        flood_after - flood_before,
    )
}

#[test]
fn a_full_pair_takes_a_better_paying_trader_and_evicts_the_cheapest() {
    let (native, asset, flood) = flood_then_honest(50_000, 50_001);
    assert_eq!(native, -1_000_000, "the honest swap's input was taken");
    assert!(asset > 0, "the honest swap cleared");
    assert!(flood > 0, "the rest of the flood still traded");
}

#[test]
fn a_full_pair_turns_away_a_lower_bid_as_a_no_op_not_a_failed_block() {
    // One unit under the cheapest flood intent. An equal bid is decided by
    // identifier, which is fixed but not chosen here.
    let (native, asset, _) = flood_then_honest(50_000, 49_999);
    assert_eq!(
        (native, asset),
        (0, 0),
        "outbid: nothing taken, nothing paid"
    );
}
