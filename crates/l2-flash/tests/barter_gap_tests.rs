//! Barter overflow, atomicity and conservation (test-gaps item 6).

#![allow(clippy::unwrap_used)]

use l2_flash::barter::{BarterError, Market, Resource, Sla};
use primitive_types::{U256, U512};
use proptest::prelude::*;

fn who(i: u8) -> [u8; 32] {
    [i; 32]
}

fn sla(provider: u8, consumer: u8, price: u64, collateral: u64) -> Sla {
    Sla {
        provider: who(provider),
        consumer: who(consumer),
        resource: Resource::Compute,
        price_per_unit: U256::from(price),
        committed_per_tick: U256::from(10),
        floor_bps: 10_000,
        slash_per_breach: U256::from(7),
        collateral: U256::from(collateral),
    }
}

#[test]
fn funding_past_the_ceiling_overflows_and_leaves_the_balance() {
    let mut m = Market::default();
    m.fund(who(1), U256::MAX).unwrap();
    assert_eq!(m.fund(who(1), U256::one()), Err(BarterError::Overflow));
    assert_eq!(m.balance(&who(1)), U256::MAX);
}

#[test]
fn a_credit_at_the_ceiling_overflows_without_taking_the_payers_money() {
    let mut m = Market::default();
    m.fund(who(1), U256::MAX).unwrap(); // provider already at the ceiling
    m.fund(who(2), U256::from(100)).unwrap(); // consumer
    let id = m.open(sla(1, 2, 1, 0)).unwrap();
    assert_eq!(m.tick(id, U256::from(10)), Err(BarterError::Overflow));
    // Atomic: the consumer was not charged for a payment that never landed.
    assert_eq!(m.balance(&who(2)), U256::from(100));
    assert_eq!(m.balance(&who(1)), U256::MAX);
    // And the books still balance: nothing half-recorded to settle.
    let s = m.settle().unwrap();
    assert!(s.deltas.is_empty());
    assert_eq!(s.updates, 0);
}

#[test]
fn a_price_times_units_overflow_is_refused_untouched() {
    let mut m = Market::default();
    m.fund(who(1), U256::from(10)).unwrap();
    m.fund(who(2), U256::from(10)).unwrap();
    let mut s = sla(1, 2, 0, 0);
    s.price_per_unit = U256::MAX;
    let id = m.open(s).unwrap();
    assert_eq!(m.tick(id, U256::from(2)), Err(BarterError::Overflow));
    assert_eq!(m.balance(&who(2)), U256::from(10));
}

#[test]
fn a_failed_slash_credit_leaves_no_half_applied_tick() {
    // Consumer sits at the ceiling: the slash credit to it overflows.
    let mut m = Market::default();
    m.fund(who(1), U256::from(1_000)).unwrap();
    m.fund(who(2), U256::MAX).unwrap();
    let id = m.open(sla(1, 2, 1, 100)).unwrap();
    let held_before = m.collateral_held();
    // Delivered 0 of 10 committed: breach, owed 0, slash 7 -> overflow.
    assert_eq!(m.tick(id, U256::zero()), Err(BarterError::Overflow));
    assert_eq!(m.collateral_held(), held_before);
    assert_eq!(m.balance(&who(2)), U256::MAX);
    assert_eq!(m.settle().unwrap().breaches, 0);
}

#[test]
fn a_clean_tick_nets_to_the_exact_payment_and_settle_resets() {
    let mut m = Market::default();
    m.fund(who(1), U256::from(1_000)).unwrap();
    m.fund(who(2), U256::from(1_000)).unwrap();
    let id = m.open(sla(1, 2, 3, 100)).unwrap();
    assert_eq!(m.tick(id, U256::from(10)), Ok(false));
    let s = m.settle().unwrap();
    assert_eq!(s.updates, 1);
    let delta_of = |a: u8| {
        s.deltas
            .iter()
            .find(|(addr, _)| *addr == who(a))
            .map(|(_, d)| (d.negative, d.magnitude))
    };
    // Provider: -100 collateral posted, +30 paid. Consumer: -30.
    assert_eq!(delta_of(1), Some((true, U256::from(70))));
    assert_eq!(delta_of(2), Some((true, U256::from(30))));
    assert!(m.settle().unwrap().deltas.is_empty());
}

#[test]
fn a_breach_slashes_exactly_the_configured_amount_to_the_consumer() {
    let mut m = Market::default();
    m.fund(who(1), U256::from(1_000)).unwrap();
    m.fund(who(2), U256::from(1_000)).unwrap();
    let id = m.open(sla(1, 2, 3, 100)).unwrap();
    assert_eq!(m.tick(id, U256::from(9)), Ok(true)); // floor is 10 of 10
    assert_eq!(m.collateral_held(), U256::from(93));
    assert_eq!(m.balance(&who(2)), U256::from(1_000 - 27 + 7));
    assert_eq!(m.settle().unwrap().breaches, 1);
}

/// Value the market accounts for: balances plus held collateral.
fn total(m: &Market) -> U512 {
    (1..=4u8).fold(U512::from(m.collateral_held()), |acc, i| {
        acc + U512::from(m.balance(&who(i)))
    })
}

#[derive(Clone, Debug)]
enum Op {
    Fund(u8, U256),
    Open(u8, u8, U256, U256),
    Tick(usize, U256),
    Close(usize),
    Settle,
}

fn amount() -> impl Strategy<Value = U256> {
    prop_oneof![
        4 => (0u64..1_000).prop_map(U256::from),
        1 => (0u64..4).prop_map(|d| U256::MAX - U256::from(d)),
        1 => (0u64..4).prop_map(|d| (U256::MAX >> 1) + U256::from(d)),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (1u8..=4, amount()).prop_map(|(a, v)| Op::Fund(a, v)),
        (
            1u8..=4,
            1u8..=4,
            amount(),
            (0u64..1_000).prop_map(U256::from)
        )
            .prop_map(|(p, c, pr, col)| Op::Open(p, c, pr, col)),
        (0usize..4, amount()).prop_map(|(i, d)| Op::Tick(i, d)),
        (0usize..4).prop_map(Op::Close),
        Just(Op::Settle),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Accepted or refused, a step changes the market's total only by an
    /// accepted funding, and refused steps change nothing at all.
    #[test]
    fn value_is_conserved_across_random_accepted_and_refused_steps(
        ops in proptest::collection::vec(op(), 1..40)
    ) {
        let mut m = Market::default();
        let mut slas = 0usize;
        for o in ops {
            let before = total(&m);
            let mut funded = U512::zero();
            match o {
                Op::Fund(a, v) => {
                    if m.fund(who(a), v).is_ok() { funded = U512::from(v); }
                }
                Op::Open(p, c, price, col) => {
                    let s = Sla { provider: who(p), consumer: who(c), resource: Resource::Power,
                        price_per_unit: price, committed_per_tick: U256::from(10), floor_bps: 5_000,
                        slash_per_breach: U256::from(7), collateral: col };
                    if m.open(s).is_ok() { slas += 1; }
                }
                Op::Tick(i, d) => { let _ = m.tick(i % slas.max(1), d); }
                Op::Close(i) => { let _ = m.close(i % slas.max(1)); }
                // A refused settle is the conservation check firing; an accepted
                // one must not move balances.
                Op::Settle => { let _ = m.settle(); }
            }
            prop_assert_eq!(total(&m), before + funded);
        }
    }

    /// Every settlement that is accepted balances: what accounts gained, plus
    /// collateral released, equals what they lost, plus collateral posted.
    #[test]
    fn accepted_settlements_balance_against_escrow(
        ops in proptest::collection::vec(op(), 1..40)
    ) {
        let mut m = Market::default();
        let mut slas = 0usize;
        for o in ops {
            match o {
                Op::Fund(a, v) => { let _ = m.fund(who(a), v); }
                Op::Open(p, c, price, col) => {
                    let s = Sla { provider: who(p), consumer: who(c), resource: Resource::Power,
                        price_per_unit: price, committed_per_tick: U256::from(10), floor_bps: 5_000,
                        slash_per_breach: U256::from(7), collateral: col };
                    if m.open(s).is_ok() { slas += 1; }
                }
                Op::Tick(i, d) => { let _ = m.tick(i % slas.max(1), d); }
                Op::Close(i) => { let _ = m.close(i % slas.max(1)); }
                Op::Settle => {}
            }
        }
        // With atomic steps, nothing reachable through the public API can unbalance
        // the books, so the batch must always be accepted.
        prop_assert!(m.settle().is_ok());
    }
}
