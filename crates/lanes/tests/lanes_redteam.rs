//! Red team: the fee step bound, the per-app cap and lanes under hostile input.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use maya_amount::Amount;
use maya_lanes::{Bid, Fees, Lane, Params, Tx, auction, build};

const P: Params = Params {
    capacity: 10,
    app_target: 2,
    global_target: 5,
    denom: 8,
    floor: Amount::from_u64(1),
    app_cap: 4,
};

fn amt(n: u64) -> Amount {
    Amount::from_u64(n)
}

fn tx(app: u32, fee: u64, lane: Option<u32>, arrived: u64) -> Tx {
    Tx {
        app,
        max_fee: amt(fee),
        lane,
        arrived,
    }
}

/// Found by this file: an app already holding a local fee was listed twice in
/// `update` (once from the fee map, once from the usage), so its fee took
/// two steps per block instead of one.
#[test]
fn an_app_takes_exactly_one_fee_step_per_block() {
    let params = Params {
        floor: amt(1_000),
        ..P
    };
    let mut fees = Fees::new(params);
    fees.local.insert(1, amt(1_000)); // seen before
    let usage = BTreeMap::from([(1, 10), (2, 10)]); // app 2 is new
    fees.update(&usage);
    // used 10, target 2, denom 8: one step is +1000·8/16 = +500.
    assert_eq!(fees.local[&2], amt(1_500));
    assert_eq!(fees.local[&1], fees.local[&2], "same fee, same usage");
}

#[test]
fn a_fee_never_moves_more_than_one_denominator_step_per_block() {
    let mut fees = Fees::new(P);
    let usage = BTreeMap::from([(7, 10)]);
    for _ in 0..50 {
        let before = fees.price(7).saturating_sub(fees.global);
        fees.update(&usage);
        let after = fees.local[&7];
        // `before · (10 − 2) / (2 · 8)`, at least 1.
        let step = before.mul_div(amt(8), amt(16)).unwrap().max(amt(1));
        assert!(after <= before.saturating_add(step), "{before} -> {after}");
    }
}

#[test]
fn an_idle_app_decays_to_the_floor_and_stays_there() {
    // Idle, the down-step is fee/denom; a fee under `denom` (8) rounds that to
    // zero and parks there, so the floor here sits above it.
    let params = Params {
        floor: amt(100),
        ..P
    };
    let mut fees = Fees::new(params);
    fees.local.insert(3, amt(1_000_000));
    for _ in 0..500 {
        fees.update(&BTreeMap::new());
    }
    assert_eq!(fees.local[&3], params.floor);
    assert_eq!(fees.global, params.floor);
}

#[test]
fn the_per_app_cap_holds_even_when_the_app_outbids_everyone() {
    let fees = Fees::new(P);
    let mut pool: Vec<Tx> = (0..20).map(|i| tx(1, 1_000_000, None, i)).collect();
    pool.push(tx(2, 2, None, 0));
    let block = build(&mut pool, &[], &fees);
    assert_eq!(block.iter().filter(|t| t.app == 1).count(), 4);
    assert!(block.iter().any(|t| t.app == 2), "the cheap app still fits");
}

#[test]
fn claiming_a_lane_you_do_not_hold_buys_no_reserved_space() {
    let fees = Fees::new(P);
    let lanes = [Lane {
        holder: 9,
        slots: 2,
        price: amt(1),
    }];
    // Holder 5 has no lane and offers less than the price: not included.
    let mut pool = vec![tx(1, 0, Some(5), 0), tx(1, 0, Some(5), 1)];
    let block = build(&mut pool, &lanes, &fees);
    assert!(block.is_empty());
    assert_eq!(pool.len(), 2);
}

#[test]
fn a_lane_carries_no_more_than_its_slots() {
    let fees = Fees::new(P);
    let lanes = [Lane {
        holder: 9,
        slots: 2,
        price: amt(1),
    }];
    // Lane traffic past its slots competes in the general pool and must pay.
    let mut pool: Vec<Tx> = (0..5).map(|i| tx(1, 0, Some(9), i)).collect();
    let block = build(&mut pool, &lanes, &fees);
    assert_eq!(block.len(), 2);
}

#[test]
fn the_auction_never_sells_more_than_it_has_or_a_holder_more_than_its_cap() {
    // One holder splits its demand across many bids to dodge the cap.
    let mut bids: Vec<Bid> = (0..10)
        .map(|i| Bid {
            holder: 1,
            slots: 3,
            price: amt(100 + i),
        })
        .collect();
    bids.push(Bid {
        holder: 2,
        slots: 100,
        price: amt(1),
    });
    let lanes = auction(bids, 8, 5);
    let held = |h| {
        lanes
            .iter()
            .filter(|l| l.holder == h)
            .map(|l| l.slots)
            .sum::<u64>()
    };
    assert_eq!(held(1), 5);
    assert_eq!(held(2), 3);
    assert_eq!(lanes.iter().map(|l| l.slots).sum::<u64>(), 8);
}

#[test]
fn a_zero_target_or_denominator_is_clamped_not_a_division_by_zero() {
    let mut fees = Fees::new(Params {
        app_target: 0,
        global_target: 0,
        denom: 0,
        ..P
    });
    fees.update(&BTreeMap::from([(1, 3)]));
    assert!(fees.local[&1] > P.floor);
}
