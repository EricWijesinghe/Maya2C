//! CON-07 (Eric 2026-11-24): no exit shrinks the committee below
//! `MIN_COMMITTEE`. A whole-bond unbond or a tombstone that would break the
//! four-seat floor waits for a replacement; the slash itself never waits.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_staking::{
    Amount, Effect, MIN_COMMITTEE, Params, Participation, StakeError, Staking, Status,
};

fn a(n: u64) -> Amount {
    Amount::from_u64(n)
}

fn id(n: u8) -> [u8; 32] {
    [n; 32]
}

fn account(n: u8) -> [u8; 32] {
    [0xA0 | n; 32]
}

/// `n` validators at 10,000 each, settled into the committee.
fn seated(n: u8) -> Staking {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    for i in 1..=n {
        s.register(account(i), id(i), a(10_000), 0).unwrap();
    }
    boundary(&mut s);
    assert_eq!(s.active.len(), usize::from(n));
    s
}

fn boundary(s: &mut Staking) -> Vec<Effect> {
    s.end_epoch(&Participation::default(), Amount::ZERO)
        .unwrap()
        .effects
}

#[test]
fn a_whole_bond_exit_at_the_floor_waits_for_a_replacement() {
    let mut s = seated(4);
    s.unbond(account(4), id(4), a(10_000)).unwrap();
    // Waiting, not refused: still bonded, seated and slashable.
    assert_eq!(s.validators[&id(4)].self_bond, a(10_000));
    assert!(s.unbonding.is_empty());
    assert_eq!(
        s.unbond(account(4), id(4), a(10_000)),
        Err(StakeError::Leaving)
    );
    for _ in 0..3 {
        boundary(&mut s);
        assert_eq!(s.active.len(), MIN_COMMITTEE);
        assert!(s.active.contains(&id(4)));
    }
    // A replacement bonds: the exit starts at the next boundary.
    s.register(account(5), id(5), a(10_000), 0).unwrap();
    boundary(&mut s);
    assert_eq!(s.active.len(), MIN_COMMITTEE);
    assert!(!s.active.contains(&id(4)) && s.active.contains(&id(5)));
    assert!(s.exits.is_empty());
    assert_eq!(s.validators[&id(4)].self_bond, Amount::ZERO);
    let entry = &s.unbonding[0];
    assert_eq!((entry.owner, entry.amount), (account(4), a(10_000)));
    // The funds still wait out the evidence window from the exit, then return.
    let mut returned = Amount::ZERO;
    for _ in 0..Params::DEVNET.unbonding_epochs {
        for e in boundary(&mut s) {
            if let Effect::Credit(to, amount) = e
                && to == account(4)
            {
                returned = returned.saturating_add(amount);
            }
        }
    }
    assert_eq!(returned, a(10_000));
}

#[test]
fn an_exit_above_the_floor_leaves_at_once() {
    let mut s = seated(5);
    s.unbond(account(5), id(5), a(10_000)).unwrap();
    assert_eq!(s.validators[&id(5)].self_bond, Amount::ZERO);
    assert_eq!(s.unbonding.len(), 1);
    // The fifth seat was spare; the next whole exit is the one that waits.
    s.unbond(account(4), id(4), a(10_000)).unwrap();
    assert_eq!(s.validators[&id(4)].self_bond, a(10_000));
    assert!(s.exits.contains(&id(4)));
    boundary(&mut s);
    assert_eq!(s.active.len(), MIN_COMMITTEE);
}

#[test]
fn a_partial_unbond_at_the_floor_is_not_delayed() {
    let mut s = seated(4);
    s.unbond(account(4), id(4), a(9_000)).unwrap();
    assert_eq!(s.validators[&id(4)].self_bond, a(1_000));
    assert_eq!(s.unbonding.len(), 1);
    assert!(s.exits.is_empty());
}

#[test]
fn a_tombstone_at_the_floor_is_slashed_now_and_keeps_its_seat_until_a_replacement() {
    let mut s = seated(4);
    let effects = s.slash_double_sign(id(4)).unwrap();
    assert_eq!(
        effects,
        vec![Effect::Burn(a(5_000))],
        "the slash never waits"
    );
    assert_eq!(s.validators[&id(4)].status, Status::Tombstoned);
    boundary(&mut s);
    assert_eq!(s.active.len(), MIN_COMMITTEE);
    assert!(s.active.contains(&id(4)));
    // Held, but never paid.
    let reward = s
        .end_epoch(&Participation::default(), a(4_000))
        .unwrap()
        .effects;
    assert!(
        !reward
            .iter()
            .any(|e| matches!(e, Effect::Credit(to, _) if *to == account(4))),
        "{reward:?}"
    );
    s.register(account(5), id(5), a(10_000), 0).unwrap();
    boundary(&mut s);
    assert!(!s.active.contains(&id(4)) && s.active.contains(&id(5)));
    assert_eq!(s.active.len(), MIN_COMMITTEE);
}

#[test]
fn the_floor_never_holds_more_tombstones_than_the_one_fault_it_survives() {
    let mut s = seated(4);
    s.slash_double_sign(id(3)).unwrap();
    s.slash_double_sign(id(4)).unwrap();
    boundary(&mut s);
    let held = s
        .active
        .iter()
        .filter(|v| s.validators[*v].status == Status::Tombstoned)
        .count();
    assert_eq!(held, maya_staking::MAX_HELD_TOMBSTONES);
    assert_eq!(held, 1);
    assert_eq!(
        s.active.len(),
        3,
        "safety first: a second equivocator is not seated"
    );
}

#[test]
fn a_waiting_exit_that_is_then_tombstoned_is_dropped_from_the_queue() {
    let mut s = seated(4);
    s.unbond(account(4), id(4), a(10_000)).unwrap();
    s.slash_double_sign(id(4)).unwrap();
    assert!(s.exits.is_empty());
}
