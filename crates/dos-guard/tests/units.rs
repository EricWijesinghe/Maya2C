#![allow(clippy::unwrap_used, clippy::panic)]

use maya_dos_guard::Addr;
use maya_dos_guard::admission::{Admitted, Pending, Policy, Pool, Refusal};
use maya_dos_guard::budget::{Budgets, Rate};
use maya_dos_guard::cookie::{CookieJar, EPOCH_MS};
use maya_dos_guard::puzzle;

const A: Addr = Addr::V4([1, 2, 3, 4]);

#[test]
fn cookies_bind_address_and_epoch() {
    let jar = CookieJar::new([1; 32]);
    let c = jar.issue(A, 5_000);
    assert!(jar.check(A, &c, 5_000));
    assert!(
        jar.check(A, &c, 5_000 + EPOCH_MS),
        "the previous epoch is still accepted"
    );
    assert!(
        !jar.check(A, &c, 5_000 + 2 * EPOCH_MS),
        "two epochs later it has expired"
    );
    assert!(
        !jar.check(Addr::V4([1, 2, 3, 5]), &c, 5_000),
        "another address cannot reuse it"
    );
    assert!(
        !CookieJar::new([2; 32]).check(A, &c, 5_000),
        "another node's secret"
    );
}

#[test]
fn puzzles_are_checkable_in_one_hash() {
    let (nonce, _) = puzzle::solve(&[3; 32], 10);
    assert!(puzzle::check(&[3; 32], nonce, 10));
    // A solution is bound to its challenge: across 64 other challenges the same
    // nonce passes about 64 / 1024 times, far from always.
    let reused = (0..64u8)
        .filter(|c| puzzle::check(&[*c ^ 0x80; 32], nonce, 10))
        .count();
    assert!(reused < 8, "{reused}");
}

#[test]
fn subnet_budget_stops_address_rotation() {
    let mut b = Budgets::new(
        Rate {
            per_sec: 1,
            burst: 1,
        },
        Rate {
            per_sec: 1,
            burst: 3,
        },
    );
    let admitted = (0..10u8)
        .filter(|h| b.admit(Addr::V4([9, 9, 9, *h]), 0))
        .count();
    assert_eq!(
        admitted, 3,
        "fresh addresses in one /24 share the subnet's burst"
    );
    assert!(
        b.admit(Addr::V4([9, 9, 9, 200]), 1_000),
        "one token refills per second"
    );
    // Host .0 names its own /24: the two budgets must not share a bucket.
    let mut b = Budgets::new(
        Rate {
            per_sec: 1,
            burst: 1,
        },
        Rate {
            per_sec: 1,
            burst: 5,
        },
    );
    assert!(b.admit(Addr::V4([7, 7, 7, 0]), 0));
    assert!(
        !b.admit(Addr::V4([7, 7, 7, 0]), 0),
        "its own address budget is spent"
    );
    assert!(
        b.admit(Addr::V4([7, 7, 7, 1]), 0),
        "the subnet still has tokens"
    );
}

fn tx(sender: u8, nonce: u64, fee: u64, size: u64) -> Pending {
    Pending {
        sender: [sender; 32],
        nonce,
        fee,
        size,
    }
}

#[test]
fn admission_caps_senders_prices_replacements_and_evicts_by_fee_per_byte() {
    let mut p = Pool::new(Policy {
        capacity: 3,
        per_sender: 2,
        bump_percent: 10,
    });
    assert_eq!(p.admit(tx(1, 0, 100, 10)), Ok(Admitted::Added));
    assert_eq!(p.admit(tx(1, 1, 100, 10)), Ok(Admitted::Added));
    assert_eq!(p.admit(tx(1, 2, 100, 10)), Err(Refusal::SenderFull));
    assert_eq!(
        p.admit(tx(1, 0, 109, 10)),
        Err(Refusal::UnderpricedReplacement)
    );
    assert_eq!(p.admit(tx(1, 0, 110, 10)), Ok(Admitted::Replaced));
    assert_eq!(p.admit(tx(2, 0, 50, 10)), Ok(Admitted::Added));
    // Full: a big transaction paying more in total but less per byte is refused.
    assert_eq!(p.admit(tx(3, 0, 200, 100)), Err(Refusal::PoolFull));
    let Ok(Admitted::Evicted(out)) = p.admit(tx(3, 0, 60, 10)) else {
        panic!("expected eviction")
    };
    assert_eq!(out.sender, [2; 32]);
}
