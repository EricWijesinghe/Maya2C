//! Kani proofs of the timelock rule. Compiled only under Kani.
//!
//! Unbounded: every `u64` expiry and height, every status, both verifier
//! answers. These are the two properties a swap's safety argument uses — no
//! lock pays twice, and no open lock is stuck forever.

use crate::timelock::{
    ClaimOutcome, LockStatus, RefundOutcome, claim_window_open, decide_claim, decide_refund,
};

fn any_status() -> LockStatus {
    match kani::any::<u8>() % 3 {
        0 => LockStatus::Locked,
        1 => LockStatus::Claimed,
        _ => LockStatus::Refunded,
    }
}

#[kani::proof]
fn claim_and_refund_are_never_both_admitted() {
    let status = any_status();
    let expiry: u64 = kani::any();
    let height: u64 = kani::any();
    let opens: bool = kani::any();

    let claimed = decide_claim(status, expiry, height, || opens) == ClaimOutcome::Claimed;
    let refunded = decide_refund(status, expiry, height) == RefundOutcome::Refunded;
    assert!(!(claimed && refunded));
}

#[kani::proof]
fn an_open_lock_always_admits_one_of_them() {
    let expiry: u64 = kani::any();
    let height: u64 = kani::any();

    // A valid opening inside the window claims; outside it, a refund repays.
    let claim = decide_claim(LockStatus::Locked, expiry, height, || true);
    let refund = decide_refund(LockStatus::Locked, expiry, height);
    assert!(claim == ClaimOutcome::Claimed || refund == RefundOutcome::Refunded);
    assert!(claim_window_open(expiry, height) == (claim == ClaimOutcome::Claimed));
}

#[kani::proof]
fn a_settled_lock_admits_neither() {
    let status = any_status();
    kani::assume(status != LockStatus::Locked);
    let expiry: u64 = kani::any();
    let height: u64 = kani::any();
    let opens: bool = kani::any();

    assert!(decide_claim(status, expiry, height, || opens) == ClaimOutcome::AlreadySettled);
    assert!(decide_refund(status, expiry, height) == RefundOutcome::AlreadySettled);
}
