//! STK-23 (dual-quorum review, issue 1): at most `entry_limit` validators
//! join the committee per epoch, so seats that never come online are jailed
//! at the next boundary before they can hold more than a third of the seats.
//!
//! Under the dual quorum (CON-20, Eric 2026-11-24 third round 3) a third of
//! the seats stops finality whatever their stake, and a chain with no commits
//! reaches no epoch boundary to jail them. Without the limit, four seats
//! bonded at the minimum against a seven-seat genesis halt it for good.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use maya_staking::{Amount, MIN_COMMITTEE, Params, Participation, Staking, entry_limit};

fn a(n: u64) -> Amount {
    Amount::from_u64(n)
}

fn id(n: u8) -> [u8; 32] {
    [n; 32]
}

/// A distinct payout address per validator: grouping by operator (the
/// weight cap's grouping) would not tell these apart from honest strangers.
fn account(n: u8) -> [u8; 32] {
    [0xA0 ^ n; 32]
}

/// Seats the dual quorum tolerates being faulty: ⌊(n − 1) ÷ 3⌋.
fn seat_faults(n: usize) -> usize {
    n.saturating_sub(1) / 3
}

/// `n` validators at 10,000 each, seated as a genesis committee.
fn genesis(n: u8) -> Staking {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    for i in 1..=n {
        s.register(account(i), id(i), a(10_000), 0).unwrap();
    }
    s.active = (1..=n).map(id).collect();
    s
}

/// An epoch in which every validator in `online` led and committed all of
/// its 10 slots, and every other member led 10 and committed none.
fn epoch_where(s: &mut Staking, online: impl Fn(&[u8; 32]) -> bool) {
    let authored: BTreeMap<_, _> = s
        .active
        .iter()
        .map(|v| (*v, if online(v) { 10 } else { 0 }))
        .collect();
    let expected = s.active.iter().map(|v| (*v, 10)).collect();
    s.end_epoch(
        &Participation {
            rounds: 10,
            expected,
            authored,
        },
        Amount::ZERO,
    )
    .unwrap();
}

#[test]
fn absent_minimum_bond_seats_never_pass_a_third_of_the_committee() {
    // The review's attack: seven genesis validators at 10,000, four seats
    // bonded at the 1,000 minimum by keys nobody runs. Without the limit all
    // four are elected at once: 4 of 11 seats is past f_n = 3 and the chain
    // halts before the boundary that would jail them.
    let mut s = genesis(7);
    for i in 20..24 {
        s.register(account(i), id(i), a(1_000), 0).unwrap();
    }
    let honest = |v: &[u8; 32]| v[0] <= 7;
    for epoch in 0..8 {
        epoch_where(&mut s, honest);
        let absent = s.active.iter().filter(|v| !honest(v)).count();
        assert!(
            absent <= seat_faults(s.active.len()) / 2,
            "epoch {epoch}: {absent} absent seats of {}, at most half of f_n allowed",
            s.active.len()
        );
    }
    // Every absent seat entered, was jailed for it, and paid the slash.
    for i in 20..24 {
        assert!(
            s.validators[&id(i)].self_bond < a(1_000),
            "seat {i} was never jailed"
        );
    }
}

#[test]
fn the_entry_limit_keeps_half_the_seat_faults_for_honest_ones() {
    assert_eq!(entry_limit(7), 1);
    assert_eq!(entry_limit(12), 1);
    assert_eq!(entry_limit(13), 2);
    assert_eq!(entry_limit(100), 16);
    for n in MIN_COMMITTEE..=300 {
        let l = entry_limit(n);
        assert!(l >= 1, "n = {n}: the committee could never grow");
        // `l` absent newcomers in a committee of `n + l` leave at least one
        // seat fault for an honest outage once n ≥ 7.
        if n >= 7 {
            assert!(2 * l <= seat_faults(n + l), "n = {n}, l = {l}");
        }
    }
}

#[test]
fn a_committee_below_the_floor_refills_past_the_limit() {
    // Seven seated; five are jailed in one boundary (eligible stand-ins
    // exist, so the S-033 floor does not keep them). Two incumbents remain,
    // and the limit of one entry would leave three seats: newcomers fill to
    // MIN_COMMITTEE instead.
    let mut s = genesis(7);
    for i in 20..25 {
        s.register(account(i), id(i), a(10_000), 0).unwrap();
    }
    epoch_where(&mut s, |v| v[0] <= 2 || v[0] >= 20);
    assert_eq!(s.active.len(), MIN_COMMITTEE);
    assert!(s.active.contains(&id(1)) && s.active.contains(&id(2)));
}

#[test]
fn a_first_committee_is_not_limited() {
    // A chain with no committee yet (the simulator, a test harness) seats
    // the top `max_validators` at once, exactly as before STK-23.
    let mut s = Staking::new(Params::DEVNET).unwrap();
    for i in 1..=10 {
        s.register(account(i), id(i), a(10_000), 0).unwrap();
    }
    s.end_epoch(&Participation::default(), Amount::ZERO)
        .unwrap();
    assert_eq!(s.active.len(), 10);
}

#[test]
fn a_richer_newcomer_displaces_at_most_the_limit_from_a_full_committee() {
    let params = Params {
        max_validators: 7,
        ..Params::DEVNET
    };
    let mut s = Staking::new(params).unwrap();
    for i in 1..=7 {
        s.register(account(i), id(i), a(10_000), 0).unwrap();
    }
    s.active = (1..=7).map(id).collect();
    for i in 20..23 {
        s.register(account(i), id(i), a(50_000), 0).unwrap();
    }
    epoch_where(&mut s, |_| true);
    assert_eq!(s.active.len(), 7);
    let entered = s.active.iter().filter(|v| v[0] >= 20).count();
    assert_eq!(entered, entry_limit(7));
    assert!(s.active.contains(&id(20)), "the newcomers enter by stake");
}

/// What the limit does not do, measured (reported to Eric with the review):
/// an attacker that keeps its seats online gains `entry_limit` seats per
/// epoch, and once it holds more than a third it can go dark and halt the
/// chain. The limit turns one boundary into several, each seat earning its
/// place by authoring through a boundary; it does not price the seats.
#[test]
fn a_patient_attacker_needs_several_epochs_of_service_to_hold_a_third() {
    let mut s = genesis(7);
    for i in 20..60 {
        s.register(account(i), id(i), a(1_000), 0).unwrap();
    }
    let mut epochs = 0;
    loop {
        epoch_where(&mut s, |_| true);
        epochs += 1;
        let attacker = s.active.iter().filter(|v| v[0] >= 20).count();
        if attacker > seat_faults(s.active.len()) {
            println!(
                "STK-23 measured: an online attacker held {attacker} of {} seats, past f_n, after {epochs} epochs at {} per seat",
                s.active.len(),
                Params::DEVNET.min_self_bond
            );
            break;
        }
        assert!(epochs < 50, "never reached a third");
    }
    assert!(
        epochs >= 4,
        "without the limit this is one epoch; with it, {epochs}"
    );
}
