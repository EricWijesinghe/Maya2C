//! Hostile and out-of-order input for the status page's two measurements:
//! uptime from block timestamps, and committee stake spread.

#![allow(clippy::unwrap_used, clippy::float_cmp)]

use maya_status::Timeline;
use maya_status::decentral::{Seat, StakeSet, coalition, host_of, measure, one_host, stake};
use primitive_types::U256;

const THRESHOLD: u64 = 60;

#[test]
fn no_report_before_the_first_produced_block() {
    let mut t = Timeline::new(THRESHOLD);
    assert!(t.report(1_000).is_none());
    t.observe(0, 0);
    assert!(t.report(1_000).is_none(), "genesis alone starts no clock");
}

#[test]
fn a_proposer_clock_running_backwards_is_not_a_halt_and_does_not_panic() {
    let mut t = Timeline::new(THRESHOLD);
    t.observe(0, 0);
    t.observe(1, 10_000);
    assert_eq!(t.observe(2, 5), None);
    assert_eq!(t.observe(3, 6), None);
    let r = t.report(7).unwrap();
    assert!(r.halts.is_empty());
    assert!((0.0..=1.0).contains(&r.uptime));
}

#[test]
fn hostile_timestamps_cannot_overflow_the_downtime_sum() {
    // Two maximal gaps separated by a backwards jump: their sum exceeds u64.
    let mut t = Timeline::new(THRESHOLD);
    t.observe(0, 0);
    t.observe(1, 0);
    assert!(t.observe(2, u64::MAX).is_some());
    t.observe(3, 0);
    assert!(t.observe(4, u64::MAX).is_some());
    let r = t.report(u64::MAX).unwrap();
    assert_eq!(r.halts.len(), 2);
    assert!((0.0..=1.0).contains(&r.uptime), "uptime {}", r.uptime);
}

#[test]
fn a_wall_clock_behind_the_chain_reports_up_not_garbage() {
    let mut t = Timeline::new(THRESHOLD);
    for h in 0..=10 {
        t.observe(h, 1_000 + h);
    }
    let r = t.report(0).unwrap();
    assert!(!r.halted_now);
    assert_eq!(r.since_last_block, 0);
    assert!((0.0..=1.0).contains(&r.uptime));
}

#[test]
fn a_replayed_or_skipped_height_changes_nothing() {
    let mut t = Timeline::new(THRESHOLD);
    t.observe(0, 0);
    t.observe(1, 1);
    t.observe(2, 2);
    assert_eq!(t.observe(2, 9_999), None, "same height again");
    assert_eq!(t.observe(1, 9_999), None, "older height");
    assert_eq!(t.observe(4, 9_999), None, "skipped height");
    assert_eq!(t.height(), Some(2));
    assert!(t.report(2).unwrap().halts.is_empty());
}

#[test]
fn an_ongoing_halt_counts_as_downtime_but_never_more_than_all_of_it() {
    let mut t = Timeline::new(THRESHOLD);
    t.observe(0, 0);
    t.observe(1, 100);
    t.observe(2, 101);
    let r = t.report(1_000_000).unwrap();
    assert!(r.halted_now);
    assert!(r.uptime >= 0.0 && r.uptime < 0.001, "uptime {}", r.uptime);
}

fn seat(in_committee: bool, bond: &str, delegated: &str) -> Seat {
    Seat {
        in_committee,
        self_bond: bond.into(),
        delegated: delegated.into(),
    }
}

#[test]
fn a_stake_that_is_not_a_decimal_number_is_missing() {
    for bad in ["-1", "0x10", "1e3", " 5", "5 ", "1.5", "", "abc"] {
        assert_eq!(stake(&seat(true, bad, "0")), None, "self_bond {bad:?}");
        assert_eq!(stake(&seat(true, "0", bad)), None, "delegated {bad:?}");
    }
}

#[test]
fn a_stake_sum_past_u256_is_missing_not_wrapped() {
    let max = U256::MAX.to_string();
    assert_eq!(stake(&seat(true, &max, "1")), None);
    assert_eq!(stake(&seat(true, &max, "0")), Some(U256::MAX));
}

#[test]
fn a_truncated_listing_is_missing() {
    let set = StakeSet {
        epoch: 3,
        committee: 4,
        validators: vec![seat(true, "10", "0"), seat(true, "10", "0"), seat(true, "10", "0")],
    };
    assert_eq!(measure(&set), None);
}

#[test]
fn one_whale_is_both_a_third_and_two_thirds() {
    let set = StakeSet {
        epoch: 7,
        committee: 4,
        validators: vec![
            seat(true, "1", "0"),
            seat(true, "97", "0"),
            seat(true, "1", "0"),
            seat(true, "1", "0"),
            seat(false, "1000000", "0"),
        ],
    };
    let d = measure(&set).unwrap();
    assert_eq!(d.committee, 4);
    assert_eq!(d.total_stake, "100");
    assert_eq!(d.nakamoto_third, 1);
    assert_eq!(d.nakamoto_two_thirds, 1);
    assert_eq!(d.largest_share_bps, 9_700);
}

#[test]
fn exactly_a_third_is_not_more_than_a_third() {
    let stakes = [U256::from(1u8), U256::from(1u8), U256::from(1u8)];
    let total = U256::from(3u8);
    assert_eq!(coalition(&stakes, total, 1, 3), Some(2));
    assert_eq!(coalition(&stakes, total, 2, 3), Some(3));
    assert_eq!(coalition(&stakes, U256::zero(), 1, 3), None);
}

#[test]
fn stakes_near_u256_max_are_missing_not_wrapped() {
    let big = (U256::MAX / 2).to_string();
    let set = StakeSet {
        epoch: 1,
        committee: 2,
        validators: vec![seat(true, &big, "0"), seat(true, &big, "0")],
    };
    assert_eq!(measure(&set), None, "share in bps overflows U256");
}

#[test]
fn an_all_zero_committee_is_missing() {
    let set = StakeSet {
        epoch: 1,
        committee: 2,
        validators: vec![seat(true, "0", "0"), seat(true, "0", "0")],
    };
    assert_eq!(measure(&set), None);
}

#[test]
fn host_of_strips_scheme_port_path_and_keeps_ipv6_brackets() {
    assert_eq!(host_of("http://127.0.0.1:8545/rpc"), "127.0.0.1");
    assert_eq!(host_of("https://node.example?x=1"), "node.example");
    assert_eq!(host_of("http://[::1]:8545"), "[::1]");
    assert_eq!(host_of("node.example:1"), "node.example");
    assert_eq!(host_of("http://[::1"), "[::1");
    assert_eq!(host_of(""), "");
}

#[test]
fn one_host_needs_two_urls_on_the_same_host() {
    assert_eq!(one_host(&[]), None);
    assert_eq!(one_host(&["a"]), None);
    assert_eq!(one_host(&["a", "a"]), Some("a"));
    assert_eq!(one_host(&["a", "a", "b"]), None);
}
