//! Spend edge cases: overspend, unknown index, double draw, limit boundary.
#![allow(clippy::unwrap_used)]

use maya_amount::Amount;
use maya_treasury::spending::{SpendError, Status, Treasury};

fn amt(n: u64) -> Amount {
    Amount::from_u64(n)
}

/// One-stage treasury so a single approval executes.
fn one_stage(balance: u64, limit: u64) -> Treasury {
    Treasury::new(amt(balance), 1_000, amt(limit), 1)
}

#[test]
fn overspend_is_insufficient_and_leaves_balance_and_spend_untouched() {
    let mut t = one_stage(100, 1_000_000);
    let i = t.propose([1; 32], amt(101), "too much", 1);
    assert_eq!(t.approve(i, 0, 2).unwrap_err(), SpendError::Insufficient);
    assert_eq!(t.balance, amt(100), "balance untouched");
    assert_eq!(
        t.spend(i).unwrap().status,
        Status::Pending { next: 0 },
        "spend stays pending, not half-executed"
    );
    assert!(
        t.ledger().iter().all(|e| e.event != "executed"),
        "nothing executed in the ledger"
    );
}

#[test]
fn a_failed_overspend_does_not_consume_the_epoch_limit() {
    let mut t = one_stage(100, 150);
    let big = t.propose([1; 32], amt(120), "big", 1);
    assert_eq!(t.approve(big, 0, 2).unwrap_err(), SpendError::Insufficient);
    let ok = t.propose([2; 32], amt(100), "ok", 3);
    assert!(t.approve(ok, 0, 4).is_ok(), "limit not charged by failure");
    assert_eq!(t.balance, amt(0));
}

#[test]
fn spending_the_exact_balance_succeeds_and_one_more_does_not() {
    let mut t = one_stage(100, 1_000);
    let i = t.propose([1; 32], amt(100), "all", 1);
    t.approve(i, 0, 2).unwrap();
    assert_eq!(t.balance, amt(0));
    let j = t.propose([2; 32], amt(1), "one more", 3);
    assert_eq!(t.approve(j, 0, 4).unwrap_err(), SpendError::Insufficient);
    assert_eq!(t.balance, amt(0));
}

#[test]
fn unknown_index_is_unknown_for_approve_and_reject() {
    let mut t = one_stage(100, 100);
    assert_eq!(t.approve(0, 0, 1).unwrap_err(), SpendError::Unknown);
    assert_eq!(t.reject(7, 0, 1), Err(SpendError::Unknown));
    assert!(t.spend(0).is_none());
    let i = t.propose([1; 32], amt(1), "x", 1);
    assert_eq!(t.approve(i + 1, 0, 2).unwrap_err(), SpendError::Unknown);
    assert_eq!(t.balance, amt(100));
}

#[test]
fn an_executed_spend_cannot_be_drawn_twice() {
    let mut t = one_stage(100, 1_000);
    let i = t.propose([1; 32], amt(40), "once", 1);
    t.approve(i, 0, 2).unwrap();
    assert_eq!(t.balance, amt(60));
    for stage in 0..3 {
        assert_eq!(t.approve(i, stage, 3).unwrap_err(), SpendError::OutOfOrder);
    }
    assert_eq!(t.reject(i, 0, 4), Err(SpendError::OutOfOrder));
    assert_eq!(t.balance, amt(60), "no second draw");
    assert_eq!(
        t.ledger().iter().filter(|e| e.event == "executed").count(),
        1
    );
}

#[test]
fn a_rejected_spend_cannot_be_executed() {
    let mut t = one_stage(100, 1_000);
    let i = t.propose([1; 32], amt(40), "no", 1);
    t.reject(i, 0, 2).unwrap();
    assert_eq!(t.approve(i, 0, 3).unwrap_err(), SpendError::OutOfOrder);
    assert_eq!(t.balance, amt(100));
}

#[test]
fn epoch_limit_boundary_is_inclusive_and_resets_next_epoch() {
    let mut t = Treasury::new(amt(10_000), 100, amt(500), 1);
    let a = t.propose([1; 32], amt(500), "exactly the limit", 1);
    t.approve(a, 0, 99).unwrap();
    let b = t.propose([2; 32], amt(1), "one over", 2);
    assert_eq!(t.approve(b, 0, 99).unwrap_err(), SpendError::OverEpochLimit);
    assert_eq!(t.balance, amt(9_500));
    assert!(t.approve(b, 0, 100).is_ok(), "height 100 is epoch 1");
}

#[test]
fn over_epoch_limit_takes_priority_and_changes_nothing() {
    let mut t = Treasury::new(amt(10), 100, amt(5), 1);
    let i = t.propose([1; 32], amt(8), "both limits", 1);
    assert_eq!(t.approve(i, 0, 2).unwrap_err(), SpendError::OverEpochLimit);
    assert_eq!(t.balance, amt(10));
    assert_eq!(t.spend(i).unwrap().status, Status::Pending { next: 0 });
}
