//! Exact boundaries: puzzle difficulty, budget pruning, verdict thresholds.

#![allow(clippy::unwrap_used)]

use maya_dos_guard::Addr;
use maya_dos_guard::budget::{Budgets, Rate};
use maya_dos_guard::ledger::{HALF_LIFE_MS, Ledger, Thresholds, Verdict};
use maya_dos_guard::puzzle;

/// The exact number of leading zero bits of the digest for `nonce`, found
/// through the public `check` only (it is monotone in `bits`).
fn zero_bits(challenge: &[u8; 32], nonce: u64) -> u32 {
    (0..=256)
        .take_while(|b| puzzle::check(challenge, nonce, *b))
        .last()
        .unwrap()
}

#[test]
fn a_solution_one_bit_short_of_the_difficulty_fails() {
    let challenge = [7u8; 32];
    for bits in 1..=12u32 {
        let (nonce, _) = puzzle::solve(&challenge, bits);
        let z = zero_bits(&challenge, nonce);
        assert!(z >= bits, "solve returned a non-solution at {bits}");
        assert!(puzzle::check(&challenge, nonce, z), "exactly z bits passes");
        assert!(!puzzle::check(&challenge, nonce, z + 1), "z+1 bits fails");
    }
}

#[test]
fn solve_returns_the_first_solution_and_counts_attempts() {
    let challenge = [9u8; 32];
    let (nonce, attempts) = puzzle::solve(&challenge, 8);
    assert_eq!(attempts, nonce + 1);
    for earlier in 0..nonce {
        assert!(!puzzle::check(&challenge, earlier, 8), "nonce {earlier}");
    }
}

#[test]
fn difficulty_zero_always_passes_and_impossible_difficulty_never() {
    for nonce in 0..64 {
        assert!(puzzle::check(&[1; 32], nonce, 0));
        assert!(
            !puzzle::check(&[1; 32], nonce, 257),
            "more bits than a hash has"
        );
    }
}

fn rate(per_sec: u32, burst: u32) -> Rate {
    Rate { per_sec, burst }
}

#[test]
fn prune_drops_only_buckets_idle_for_the_full_window() {
    // per_sec 0 so a drained bucket stays drained: a wrongly pruned bucket
    // would come back full and be admitted again.
    let mut b = Budgets::new(rate(0, 1), rate(0, 100));
    let old = Addr::V4([1, 1, 1, 1]);
    let fresh = Addr::V4([2, 2, 2, 2]);
    assert!(b.admit(old, 0));
    assert!(b.admit(fresh, 5_000));
    assert_eq!(b.len(), 4, "two addresses plus two subnets");

    b.prune(9_999, 10_000);
    assert_eq!(b.len(), 4, "idle 9,999 ms < 10,000 ms: nothing expired");

    b.prune(10_000, 10_000);
    assert_eq!(
        b.len(),
        2,
        "old idle exactly the window: its two buckets go"
    );
    assert!(
        !b.admit(fresh, 10_000),
        "unexpired bucket keeps its drained state"
    );
    assert!(b.admit(old, 10_000), "pruned bucket starts full again");
}

#[test]
fn prune_everything_empties_and_a_refused_admit_still_counts_as_activity() {
    let mut b = Budgets::new(rate(0, 1), rate(0, 100));
    let a = Addr::V4([3, 3, 3, 3]);
    assert!(b.admit(a, 0));
    assert!(
        !b.admit(a, 9_000),
        "refused, but touches the bucket at 9,000"
    );
    b.prune(10_000, 10_000);
    // The refused IP bucket was touched at 9,000; admit short-circuits before
    // it reads the subnet bucket, so only that one survives.
    assert_eq!(b.len(), 1, "touched at 9,000, so not idle for 10,000 yet");
    b.prune(19_000, 10_000);
    assert!(b.is_empty());
}

fn ledger() -> Ledger<u32> {
    Ledger::new(Thresholds {
        throttle: 100,
        disconnect: 200,
    })
}

#[test]
fn verdict_escalates_strictly_above_each_threshold() {
    let steps = [
        (100, Verdict::Ok),
        (101, Verdict::Throttle),
        (200, Verdict::Throttle),
        (201, Verdict::Disconnect),
    ];
    for (cost, want) in steps {
        let mut l = ledger();
        l.charge(1, cost, 0);
        assert_eq!(l.verdict(1, 0), want, "net cost {cost}");
    }
}

#[test]
fn credit_offsets_cost_and_cannot_go_negative() {
    let mut l = ledger();
    l.charge(1, 201, 0);
    assert_eq!(l.verdict(1, 0), Verdict::Disconnect);
    l.credit(1, 1, 0);
    assert_eq!(l.verdict(1, 0), Verdict::Throttle, "net 200");
    l.credit(1, 100, 0);
    assert_eq!(l.verdict(1, 0), Verdict::Ok, "net 100");
    l.credit(1, 1_000_000, 0);
    assert_eq!(l.verdict(1, 0), Verdict::Ok, "credit beyond cost saturates");
}

#[test]
fn peers_are_independent() {
    let mut l = ledger();
    l.charge(1, 500, 0);
    assert_eq!(l.verdict(1, 0), Verdict::Disconnect);
    assert_eq!(l.verdict(2, 0), Verdict::Ok);
}

#[test]
fn cost_halves_exactly_once_per_half_life() {
    let mut l = ledger();
    l.charge(1, 400, 0);
    assert_eq!(
        l.verdict(1, HALF_LIFE_MS - 1),
        Verdict::Disconnect,
        "no decay yet"
    );
    assert_eq!(l.verdict(1, HALF_LIFE_MS), Verdict::Throttle, "400 -> 200");
    assert_eq!(l.verdict(1, 2 * HALF_LIFE_MS - 1), Verdict::Throttle);
    assert_eq!(l.verdict(1, 2 * HALF_LIFE_MS), Verdict::Ok, "200 -> 100");
}

#[test]
fn decay_keeps_the_phase_when_polled_off_boundary() {
    // Polled at 1.5 half-lives, the tab must keep the 0.5 leftover, so the
    // second halving still lands at 2 half-lives rather than 2.5.
    let mut l = ledger();
    l.charge(1, 400, 0);
    assert_eq!(
        l.verdict(1, HALF_LIFE_MS + HALF_LIFE_MS / 2),
        Verdict::Throttle
    );
    assert_eq!(l.verdict(1, 2 * HALF_LIFE_MS), Verdict::Ok);
}

#[test]
fn a_huge_gap_decays_to_zero_without_overflow() {
    let mut l = ledger();
    l.charge(1, u64::MAX, 0);
    assert_eq!(l.verdict(1, 0), Verdict::Disconnect);
    assert_eq!(l.verdict(1, u64::MAX), Verdict::Ok);
}

#[test]
fn charges_saturate_instead_of_wrapping() {
    let mut l = ledger();
    l.charge(1, u64::MAX, 0);
    l.charge(1, u64::MAX, 0);
    assert_eq!(
        l.verdict(1, 0),
        Verdict::Disconnect,
        "no wrap to a small value"
    );
}

#[test]
fn remove_forgets_the_peer() {
    let mut l = ledger();
    l.charge(1, 500, 0);
    l.remove(1);
    assert_eq!(l.verdict(1, 0), Verdict::Ok);
    l.remove(99); // unknown peer: no panic
}

// Red team (P2 verification): the defences must not become the lever.

use maya_dos_guard::admission::{Admitted, Pending, Policy, Pool, Refusal};
use maya_dos_guard::diversity::{Limits, Table};

#[test]
fn refused_inbound_attempts_leave_no_state_behind() {
    // A full table must cost an attacker's rotating subnets nothing to refuse:
    // an IPv6 /48 rotation is effectively free, so a record per refused subnet
    // would be unbounded memory handed out by the defence itself.
    let mut t = Table::new(Limits {
        inbound: 4,
        per_subnet: 1,
        outbound: 0,
    });
    for i in 0..4u8 {
        assert!(t.accept_inbound(Addr::V4([10, i, 0, 1])));
    }
    for i in 0..10_000u32 {
        let [_, _, a, b] = i.to_be_bytes();
        let mut v6 = [0u8; 16];
        v6[..4].copy_from_slice(&[0x20, 0x01, a, b]);
        assert!(!t.accept_inbound(Addr::V6(v6)), "table is full");
    }
    let (inbound, _) = t.connections();
    assert_eq!(inbound.len(), 4, "only the accepted subnets are recorded");
    assert!(inbound.iter().all(|(_, n)| *n == 1));
}

fn tx(sender: u8, fee: u64, size: u64) -> Pending {
    Pending {
        sender: [sender; 32],
        nonce: 0,
        fee,
        size,
    }
}

#[test]
fn a_replacement_cannot_grow_the_transaction_without_paying_per_byte() {
    // Pool capacity counts transactions, so pricing by byte is what stops a
    // small transaction being swapped for a huge one for a 10% bump.
    let mut p = Pool::new(Policy {
        capacity: 8,
        per_sender: 4,
        bump_percent: 10,
    });
    assert_eq!(p.admit(tx(1, 100, 10)), Ok(Admitted::Added));
    assert_eq!(
        p.admit(tx(1, 110, 1_000)),
        Err(Refusal::UnderpricedReplacement),
        "100x the bytes for 10% more fee"
    );
    // Paying the bump and keeping the per-byte price is a replacement.
    assert_eq!(p.admit(tx(1, 11_000, 1_000)), Ok(Admitted::Replaced));
}

#[test]
fn a_tie_on_fee_per_byte_never_evicts() {
    let mut p = Pool::new(Policy {
        capacity: 1,
        per_sender: 1,
        bump_percent: 10,
    });
    assert_eq!(p.admit(tx(1, 100, 10)), Ok(Admitted::Added));
    assert_eq!(p.admit(tx(2, 1_000, 100)), Err(Refusal::PoolFull));
    assert_eq!(p.len(), 1);
}
