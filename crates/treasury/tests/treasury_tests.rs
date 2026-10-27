#![allow(clippy::unwrap_used)]

use maya_amount::Amount;
use maya_treasury::spending::{SpendError, Status, Treasury};
use maya_treasury::vesting::{Grant, VestingError};

const YEAR: u64 = 2_102_400; // blocks at 15 s

#[test]
fn nothing_vests_before_the_cliff_then_release_is_linear_and_capped() {
    let g = Grant::new(Amount::from_u64(1_200), 100, YEAR, 4 * YEAR, false).unwrap();
    assert_eq!(g.vested(100 + YEAR - 1), Amount::from_u64(0));
    assert_eq!(
        g.vested(100 + YEAR),
        Amount::from_u64(300),
        "a quarter at the one-year cliff of four"
    );
    assert_eq!(g.vested(100 + 2 * YEAR), Amount::from_u64(600));
    assert_eq!(g.vested(100 + 4 * YEAR), Amount::from_u64(1_200));
    assert_eq!(g.vested(u64::MAX), Amount::from_u64(1_200), "never above the total");
    assert_eq!(
        Grant::new(Amount::from_u64(1), 0, 10, 5, false),
        Err(VestingError::BadSchedule)
    );
}

#[test]
fn claims_never_exceed_what_has_vested() {
    let mut g = Grant::new(Amount::from_u64(1_000), 0, 0, 1_000, false).unwrap();
    g.claim(Amount::from_u64(250), 250).unwrap();
    assert_eq!(g.claim(Amount::from_u64(1), 250), Err(VestingError::NotVested));
    g.claim(Amount::from_u64(250), 500).unwrap();
    assert_eq!(g.claimable(500), Amount::from_u64(0));
}

#[test]
fn early_termination_freezes_vesting_and_returns_the_rest() {
    let mut g = Grant::new(Amount::from_u64(1_000), 0, 100, 1_000, false).unwrap();
    assert_eq!(
        g.terminate(400).unwrap(),
        Amount::from_u64(600),
        "unvested share returns to the treasury"
    );
    assert_eq!(g.vested(900), Amount::from_u64(400), "vesting stopped at termination");
    g.claim(Amount::from_u64(400), 900).unwrap();
    assert_eq!(g.terminate(950), Err(VestingError::AlreadyClosed));
    // Terminated before the cliff: nothing vests, everything returns.
    let mut early = Grant::new(Amount::from_u64(1_000), 0, 100, 1_000, false).unwrap();
    assert_eq!(early.terminate(50).unwrap(), Amount::from_u64(1_000));
}

#[test]
fn clawback_only_where_the_grant_allowed_it() {
    let mut no = Grant::new(Amount::from_u64(1_000), 0, 0, 1_000, false).unwrap();
    assert_eq!(no.claw_back(), Err(VestingError::ClawbackNotAllowed));
    let mut yes = Grant::new(Amount::from_u64(1_000), 0, 0, 1_000, true).unwrap();
    yes.claim(Amount::from_u64(300), 300).unwrap();
    assert_eq!(
        yes.claw_back().unwrap(),
        Amount::from_u64(700),
        "everything unclaimed, vested or not"
    );
    assert_eq!(yes.claimable(1_000), Amount::from_u64(0));
    assert_eq!(yes.claw_back(), Err(VestingError::AlreadyClosed));
}

#[test]
fn spends_need_every_stage_in_order_and_respect_the_epoch_limit() {
    let mut t = Treasury::new(Amount::from_u64(10_000), 1_000, Amount::from_u64(3_000), 3);
    let a = t.propose([1; 32], Amount::from_u64(2_000), "second-client grant, milestone 1", 10);
    assert_eq!(
        t.approve(a, 1, 11),
        Err(SpendError::OutOfOrder),
        "stage 1 before stage 0"
    );
    t.approve(a, 0, 12).unwrap();
    t.approve(a, 1, 13).unwrap();
    assert_eq!(
        t.approve(a, 2, 14).unwrap(),
        &Status::Executed { height: 14 }
    );
    assert_eq!(t.balance, Amount::from_u64(8_000));
    let b = t.propose([2; 32], Amount::from_u64(1_500), "audit", 20);
    t.approve(b, 0, 21).unwrap();
    t.approve(b, 1, 22).unwrap();
    assert_eq!(
        t.approve(b, 2, 23),
        Err(SpendError::OverEpochLimit),
        "2,000 + 1,500 > 3,000 in one epoch"
    );
    assert_eq!(
        t.approve(b, 2, 1_001).unwrap(),
        &Status::Executed { height: 1_001 },
        "next epoch"
    );
    let c = t.propose([3; 32], Amount::from_u64(100), "rejected", 30);
    t.reject(c, 0, 31).unwrap();
    assert_eq!(t.approve(c, 0, 32), Err(SpendError::OutOfOrder));
    let events: Vec<&str> = t.ledger().iter().map(|e| e.event).collect();
    assert_eq!(
        events,
        [
            "proposed", "approved", "approved", "executed", "proposed", "approved", "approved",
            "executed", "proposed", "rejected"
        ]
    );
}

#[test]
fn a_grant_beyond_u128_vests_exactly() {
    // 2^200 - 1 (far past u128) over 1,000 blocks vests with no clamp.
    let total = Amount([0, 0, 0, 0]).saturating_add(Amount::MAX.shr(56));
    let mut g = Grant::new(total, 0, 0, 1_000, false).unwrap();
    assert_eq!(g.vested(1_000), total);
    assert_eq!(g.vested(500), total.mul_div(Amount::from_u64(1), Amount::from_u64(2)).unwrap());
    g.claim(g.claimable(500), 500).unwrap();
    assert_eq!(g.claimable(1_000), total.saturating_sub(g.claimed));
}
