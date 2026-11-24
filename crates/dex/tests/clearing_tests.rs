//! Block clearing: one clearing per pool per block, routes included (ADR-154).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use maya_amount::Amount;
use maya_dex::amm::Pool;
use maya_dex::batch::{SwapIntent, clear_batch};
use maya_dex::clearing::{RouteHop, RouteIntent, clear_block};
use maya_dex::fees::FeeSchedule;
use maya_dex::types::{Direction, PairId};

fn a(v: u64) -> Amount {
    Amount::from_u64(v)
}

fn w(x: Amount) -> i128 {
    i128::try_from(x.to_u128().expect("fits")).expect("fits")
}

const FEES: FeeSchedule = FeeSchedule {
    lp_bps: 30,
    protocol_bps: 0,
};

fn seeded(base: u64, quote: u64) -> Pool {
    Pool::empty(FEES)
        .add_liquidity(a(base), a(quote))
        .expect("seed")
        .pool
}

const P: PairId = [1; 32];
const Q: PairId = [2; 32];

fn route(id: u8, hops: &[(PairId, Direction)], amount_in: u64, min_out: u64) -> RouteIntent {
    RouteIntent {
        id: [id; 32],
        trader: [id; 32],
        hops: hops
            .iter()
            .map(|(pair, direction)| RouteHop {
                pair: *pair,
                direction: *direction,
            })
            .collect(),
        amount_in: a(amount_in),
        min_out: a(min_out),
        weight: Amount::ZERO,
    }
}

fn paid(clearing: &maya_dex::clearing::BlockClearing, id: u8) -> Option<Amount> {
    clearing
        .routes
        .iter()
        .find(|settled| settled.id == [id; 32])
        .and_then(|settled| settled.amount_out)
}

fn pools() -> BTreeMap<PairId, Pool> {
    BTreeMap::from([
        (P, seeded(1_000_000, 1_000_000)),
        (Q, seeded(1_000_000, 4_000_000)),
    ])
}

/// What the attacker's front and back routes would be paid in place: front
/// ahead of the victim, back behind. The input to `back` is exactly `front`'s
/// output, so the in-place attack holds no net quote at the end.
fn in_place_attack(front_in: u64, victim_in: u64) -> (u64, u64) {
    let pool = seeded(1_000_000, 1_000_000);
    let front = pool
        .swap_exact_in(Direction::BaseToQuote, a(front_in))
        .expect("front");
    let victim = front
        .pool
        .swap_exact_in(Direction::BaseToQuote, a(victim_in))
        .expect("victim");
    let back = victim
        .pool
        .swap_exact_in(Direction::QuoteToBase, front.amount_out)
        .expect("back");
    (
        front.amount_out.to_u64().expect("small"),
        back.amount_out.to_u64().expect("small"),
    )
}

#[test]
fn a_sandwich_around_a_route_extracts_nothing() {
    // In place this attack pays: front 100k, victim 100k, back sells exactly
    // what front bought and returns more base than front spent.
    let (front_out, back_out) = in_place_attack(100_000, 100_000);
    assert!(
        back_out > 100_000,
        "the in-place sandwich must pay for this test to mean anything"
    );

    // Batched: the same three routes on one pool, in the order the attacker
    // wanted. Every one of them clears in P's single batch.
    let routes = [
        route(1, &[(P, Direction::BaseToQuote)], 100_000, 1),
        route(2, &[(P, Direction::BaseToQuote)], 100_000, 1),
        route(3, &[(P, Direction::QuoteToBase)], front_out, 1),
    ];
    let clearing = clear_block(&pools(), &BTreeMap::new(), &routes).expect("clear");
    let front = w(paid(&clearing, 1).expect("front cleared"));
    let victim = w(paid(&clearing, 2).expect("victim cleared"));
    let back = w(paid(&clearing, 3).expect("back cleared"));

    // Attacker's net: base = back - 100k, quote = front - front_out.
    let base = back - 100_000;
    let quote = front - i128::from(front_out);
    assert!(
        !((base > 0 && quote >= 0) || (base >= 0 && quote > 0)),
        "riskless profit: base {base}, quote {quote}"
    );
    // The front-runner and the victim got one price.
    assert_eq!(front, victim, "same input, same batch, same output");
}

#[test]
fn a_multi_hop_victim_cannot_be_sandwiched_on_its_second_pool() {
    // Victim: P base->quote, then Q base->quote... Q's base is P's quote only
    // in the node's asset model; the engine prices by pool, which is what the
    // ordering question is about. Attackers trade Q around the victim's leg.
    let routes = [
        route(9, &[(Q, Direction::BaseToQuote)], 50_000, 1),
        route(
            5,
            &[(P, Direction::BaseToQuote), (Q, Direction::BaseToQuote)],
            50_000,
            1,
        ),
        route(7, &[(Q, Direction::QuoteToBase)], 150_000, 1),
    ];
    let clearing = clear_block(&pools(), &BTreeMap::new(), &routes).expect("clear");
    let victim = paid(&clearing, 5).expect("victim cleared");
    let front = paid(&clearing, 9).expect("front cleared");

    // The victim's second leg and the attacker's front went the same way
    // through Q in one batch: one price per unit of input.
    let leg_in = clear_batch(
        &pools()[&P],
        &[SwapIntent {
            id: [5; 32],
            trader: [5; 32],
            direction: Direction::BaseToQuote,
            amount_in: a(50_000),
            min_out: Amount::ZERO,
        }],
    )
    .expect("first leg")
    .cleared[0]
        .amount_out;
    let lhs = w(victim) * 50_000;
    let rhs = w(front) * w(leg_in);
    assert!(
        (lhs - rhs).abs() <= 50_000 + w(leg_in),
        "victim {victim:?}, front {front:?}"
    );
}

#[test]
fn the_order_routes_arrive_in_does_not_change_the_outcome() {
    let mut routes = vec![
        route(
            4,
            &[(P, Direction::BaseToQuote), (Q, Direction::BaseToQuote)],
            20_000,
            1,
        ),
        route(2, &[(Q, Direction::QuoteToBase)], 70_000, 1),
        route(8, &[(P, Direction::QuoteToBase)], 30_000, 1),
    ];
    let swaps = BTreeMap::from([(
        P,
        vec![SwapIntent {
            id: [100; 32],
            trader: [100; 32],
            direction: Direction::BaseToQuote,
            amount_in: a(10_000),
            min_out: Amount::ZERO,
        }],
    )]);
    let baseline = clear_block(&pools(), &swaps, &routes).expect("clear");
    routes.reverse();
    let reversed = clear_block(&pools(), &swaps, &routes).expect("clear");
    assert_eq!(baseline.routes, reversed.routes);
    let reserves = |c: &maya_dex::clearing::BlockClearing| {
        c.pools
            .iter()
            .map(|p| (p.pair, p.outcome.as_ref().map(|o| o.pool)))
            .collect::<Vec<_>>()
    };
    assert_eq!(reserves(&baseline), reserves(&reversed));
}

#[test]
fn a_route_that_would_need_a_pool_to_clear_twice_is_refunded() {
    // 1 fixes P before Q; 2 asks for Q before P. Equal fees: the smaller
    // identifier joins first.
    let routes = [
        route(
            1,
            &[(P, Direction::BaseToQuote), (Q, Direction::BaseToQuote)],
            10_000,
            1,
        ),
        route(
            2,
            &[(Q, Direction::QuoteToBase), (P, Direction::QuoteToBase)],
            10_000,
            1,
        ),
    ];
    let clearing = clear_block(&pools(), &BTreeMap::new(), &routes).expect("clear");
    assert!(paid(&clearing, 1).is_some());
    assert_eq!(paid(&clearing, 2), None);
}

#[test]
fn a_route_that_revisits_a_pool_is_refunded() {
    let routes = [route(
        3,
        &[(P, Direction::BaseToQuote), (P, Direction::QuoteToBase)],
        10_000,
        1,
    )];
    let clearing = clear_block(&pools(), &BTreeMap::new(), &routes).expect("clear");
    assert_eq!(paid(&clearing, 3), None);
}

#[test]
fn a_route_that_misses_its_bound_leaves_every_pool_as_if_it_never_came() {
    let honest = route(1, &[(Q, Direction::BaseToQuote)], 5_000, 1);
    let greedy = route(
        2,
        &[(P, Direction::BaseToQuote), (Q, Direction::BaseToQuote)],
        40_000,
        u64::MAX,
    );
    let with = clear_block(&pools(), &BTreeMap::new(), &[honest.clone(), greedy]).expect("clear");
    let without = clear_block(&pools(), &BTreeMap::new(), &[honest]).expect("clear");

    assert_eq!(paid(&with, 2), None, "the bound was not met");
    assert_eq!(paid(&with, 1), paid(&without, 1));
    let q = |c: &maya_dex::clearing::BlockClearing| {
        c.pools
            .iter()
            .find(|p| p.pair == Q)
            .and_then(|p| p.outcome.as_ref().map(|o| o.pool))
    };
    assert_eq!(q(&with), q(&without));
    assert!(
        with.pools.iter().all(|p| p.pair != P),
        "P untouched: the refused route is in no pass"
    );
}

// ---------------------------------------------------------------------------
// a refused route leaves no trace, and only a better-paying settler outranks
// ---------------------------------------------------------------------------

/// A doomed route: a large first leg on `P` that skews it, then a bound no
/// pool can meet, so it is refunded whole.
fn doomed(id: u8, last: PairId) -> RouteIntent {
    route(
        id,
        &[(P, Direction::BaseToQuote), (last, Direction::BaseToQuote)],
        100_000,
        u64::MAX,
    )
}

fn weighted(mut intent: RouteIntent, weight: u64) -> RouteIntent {
    intent.weight = a(weight);
    intent
}

#[test]
fn a_doomed_route_cannot_void_an_honest_route_on_a_pool_it_skewed() {
    let alone = clear_block(
        &pools(),
        &BTreeMap::new(),
        &[route(1, &[(P, Direction::BaseToQuote)], 10_000, 1)],
    )
    .expect("clear");
    let fair = paid(&alone, 1).expect("settles alone");
    let victim = route(
        1,
        &[(P, Direction::BaseToQuote)],
        10_000,
        fair.to_u64().expect("small"),
    );

    // Whichever pays more, the doomed route goes and the victim stays.
    for (victim_fee, attacker_fee) in [(0, 0), (1, 9), (9, 1)] {
        let clearing = clear_block(
            &pools(),
            &BTreeMap::new(),
            &[
                weighted(victim.clone(), victim_fee),
                weighted(doomed(2, Q), attacker_fee),
            ],
        )
        .expect("clear");
        assert_eq!(
            paid(&clearing, 1),
            Some(fair),
            "fees {victim_fee}/{attacker_fee}"
        );
        assert_eq!(paid(&clearing, 2), None);
    }
}

#[test]
fn a_doomed_route_cannot_void_an_honest_route_that_ends_after_it() {
    // The victim fails, if at all, at R, after the doomed route's failure at
    // Q. Removing the route that fails latest would remove the victim.
    let r: PairId = [3; 32];
    let mut with_r = pools();
    with_r.insert(r, seeded(1_000_000, 1_000_000));
    let path = [(P, Direction::BaseToQuote), (r, Direction::BaseToQuote)];
    let alone =
        clear_block(&with_r, &BTreeMap::new(), &[route(1, &path, 10_000, 1)]).expect("clear");
    let fair = paid(&alone, 1).expect("settles alone");
    let victim = route(1, &path, 10_000, fair.to_u64().expect("small"));

    let clearing = clear_block(&with_r, &BTreeMap::new(), &[victim, doomed(2, Q)]).expect("clear");
    assert_eq!(paid(&clearing, 1), Some(fair));
    assert_eq!(paid(&clearing, 2), None);
}

#[test]
fn a_failing_reverse_route_with_a_smaller_id_cannot_void_a_route() {
    // One unit through Q then P: the leg pays nothing, so the blocker fails.
    let victim_path = [(P, Direction::BaseToQuote), (Q, Direction::BaseToQuote)];
    let alone = clear_block(
        &pools(),
        &BTreeMap::new(),
        &[route(9, &victim_path, 10_000, 1)],
    )
    .expect("clear");
    let fair = paid(&alone, 9).expect("settles alone");
    let blocker = route(
        1,
        &[(Q, Direction::QuoteToBase), (P, Direction::QuoteToBase)],
        1,
        0,
    );

    for blocker_fee in [0, 9] {
        let clearing = clear_block(
            &pools(),
            &BTreeMap::new(),
            &[
                route(9, &victim_path, 10_000, 1),
                weighted(blocker.clone(), blocker_fee),
            ],
        )
        .expect("clear");
        assert_eq!(paid(&clearing, 9), Some(fair), "blocker fee {blocker_fee}");
        assert_eq!(paid(&clearing, 1), None);
    }
}

#[test]
fn a_higher_fee_wins_an_order_conflict_over_a_smaller_id() {
    let forward = [(P, Direction::BaseToQuote), (Q, Direction::BaseToQuote)];
    let reverse = [(Q, Direction::QuoteToBase), (P, Direction::QuoteToBase)];
    let clearing = clear_block(
        &pools(),
        &BTreeMap::new(),
        &[
            weighted(route(9, &forward, 10_000, 1), 5),
            weighted(route(1, &reverse, 10_000, 1), 4),
        ],
    )
    .expect("clear");
    assert!(paid(&clearing, 9).is_some(), "the better payer clears");
    assert_eq!(paid(&clearing, 1), None, "a ground id wins only a tie");
}

#[test]
fn a_two_hop_route_alone_pays_what_two_batches_in_sequence_pay() {
    let routes = [route(
        6,
        &[(P, Direction::BaseToQuote), (Q, Direction::BaseToQuote)],
        25_000,
        1,
    )];
    let clearing = clear_block(&pools(), &BTreeMap::new(), &routes).expect("clear");
    let leg = |pool: &Pool, amount: Amount| {
        clear_batch(
            pool,
            &[SwapIntent {
                id: [6; 32],
                trader: [6; 32],
                direction: Direction::BaseToQuote,
                amount_in: amount,
                min_out: Amount::ZERO,
            }],
        )
        .expect("leg")
        .cleared[0]
            .amount_out
    };
    let first = leg(&pools()[&P], a(25_000));
    assert_eq!(paid(&clearing, 6), Some(leg(&pools()[&Q], first)));
}

#[test]
fn a_route_through_a_pool_the_caller_could_not_price_is_refunded() {
    let unknown: PairId = [9; 32];
    let routes = [route(
        1,
        &[
            (P, Direction::BaseToQuote),
            (unknown, Direction::BaseToQuote),
        ],
        1_000,
        1,
    )];
    let clearing = clear_block(&pools(), &BTreeMap::new(), &routes).expect("clear");
    assert_eq!(paid(&clearing, 1), None);
}

#[test]
fn more_routes_than_the_ceiling_are_refused() {
    let routes: Vec<RouteIntent> = (0..=maya_dex::clearing::MAX_BLOCK_ROUTES)
        .map(|i| {
            route(
                u8::try_from(i).expect("small"),
                &[(P, Direction::BaseToQuote)],
                10,
                0,
            )
        })
        .collect();
    assert_eq!(
        clear_block(&pools(), &BTreeMap::new(), &routes).err(),
        Some(maya_dex::error::DexError::WorkLimitReached)
    );
}
