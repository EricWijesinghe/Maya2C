//! Red team: the orderings and repeat calls an attacker or a sloppy caller
//! would try against the spending limit and the vesting schedule.
#![allow(clippy::unwrap_used)]

use maya_amount::Amount;
use maya_treasury::spending::{SpendError, Status, Treasury};
use maya_treasury::vesting::{Grant, VestingError};

fn amt(n: u64) -> Amount {
    Amount::from_u64(n)
}

/// Found by this file: terminating returned the unvested part, and a later
/// clawback returned `total − claimed` again, so the treasury got back more
/// than the grant held.
#[test]
fn clawback_after_termination_returns_only_what_termination_left() {
    let mut g = Grant::new(amt(1_000), 0, 0, 1_000, true).unwrap();
    g.claim(amt(100), 400).unwrap();
    let unvested = g.terminate(400).unwrap();
    let rest = g.claw_back().unwrap();
    assert_eq!(unvested, amt(600));
    assert_eq!(rest, amt(300), "vested 400, claimed 100");
    assert_eq!(
        unvested.saturating_add(rest).saturating_add(g.claimed),
        amt(1_000),
        "returned plus claimed is exactly the grant"
    );
    assert_eq!(g.claimable(10_000), amt(0));
}

#[test]
fn a_closed_grant_cannot_be_closed_again_in_either_order() {
    let mut a = Grant::new(amt(1_000), 0, 0, 1_000, true).unwrap();
    a.claw_back().unwrap();
    assert_eq!(a.terminate(500), Err(VestingError::AlreadyClosed));
    let mut b = Grant::new(amt(1_000), 0, 0, 1_000, true).unwrap();
    b.terminate(500).unwrap();
    assert_eq!(b.terminate(600), Err(VestingError::AlreadyClosed));
}

#[test]
fn a_claim_after_clawback_is_refused_even_for_vested_tokens() {
    let mut g = Grant::new(amt(1_000), 0, 0, 1_000, true).unwrap();
    g.claw_back().unwrap();
    assert_eq!(g.claim(amt(1), 1_000), Err(VestingError::NotVested));
}

#[test]
fn termination_before_the_cliff_returns_everything() {
    let mut g = Grant::new(amt(1_000), 100, 200, 1_000, false).unwrap();
    assert_eq!(g.terminate(250).unwrap(), amt(1_000));
    assert_eq!(g.claimable(5_000), amt(0), "frozen before the cliff");
}

#[test]
fn an_impossible_schedule_is_refused() {
    assert_eq!(
        Grant::new(amt(1), 0, 0, 0, false),
        Err(VestingError::BadSchedule)
    );
    assert_eq!(
        Grant::new(amt(1), 0, 11, 10, false),
        Err(VestingError::BadSchedule)
    );
}

#[test]
fn vesting_near_the_end_of_the_height_range_does_not_overflow() {
    let g = Grant::new(amt(1_000), u64::MAX - 10, 0, 1_000, false).unwrap();
    assert_eq!(g.vested(u64::MAX), amt(10));
}

/// Found by this file: the limit tracked one epoch, so an approval at an
/// earlier height reset it and the newer epoch could be paid out twice.
#[test]
fn an_approval_at_an_earlier_epoch_cannot_reset_the_limit() {
    let mut t = Treasury::new(amt(10_000), 100, amt(1_000), 1);
    let a = t.propose([1; 32], amt(1_000), "a", 250);
    t.approve(a, 0, 250).unwrap();
    let b = t.propose([2; 32], amt(1_000), "b", 250);
    assert_eq!(t.approve(b, 0, 150), Err(SpendError::PastEpoch));
    let c = t.propose([3; 32], amt(1_000), "c", 260);
    assert_eq!(t.approve(c, 0, 260), Err(SpendError::OverEpochLimit));
    assert_eq!(t.balance, amt(9_000), "epoch 2 paid once");
    assert_eq!(t.spend(b).unwrap().status, Status::Pending { next: 0 });
}

#[test]
fn a_stage_cannot_approve_twice_or_skip_ahead() {
    let mut t = Treasury::new(amt(10_000), 100, amt(5_000), 3);
    let s = t.propose([1; 32], amt(10), "x", 1);
    assert_eq!(t.approve(s, 1, 1), Err(SpendError::OutOfOrder), "skip");
    t.approve(s, 0, 1).unwrap();
    assert_eq!(t.approve(s, 0, 1), Err(SpendError::OutOfOrder), "repeat");
    assert_eq!(t.reject(s, 2, 1), Err(SpendError::OutOfOrder), "late veto");
    assert_eq!(t.balance, amt(10_000));
}

#[test]
fn a_refused_transition_writes_nothing_to_the_public_ledger() {
    let mut t = Treasury::new(amt(100), 100, amt(1_000), 1);
    let s = t.propose([1; 32], amt(500), "too big", 1);
    let before = t.ledger().len();
    assert_eq!(t.approve(s, 0, 1), Err(SpendError::Insufficient));
    assert_eq!(t.approve(99, 0, 1), Err(SpendError::Unknown));
    assert_eq!(t.reject(99, 0, 1), Err(SpendError::Unknown));
    assert_eq!(t.ledger().len(), before);
}

#[test]
fn zero_stages_and_zero_epoch_length_are_clamped_not_a_panic() {
    let mut t = Treasury::new(amt(100), 0, amt(100), 0);
    let s = t.propose([1; 32], amt(100), "x", 7);
    assert_eq!(t.approve(s, 0, 7).unwrap(), &Status::Executed { height: 7 });
}
