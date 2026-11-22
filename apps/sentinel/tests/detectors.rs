//! Each detector against hand-built observations. No RPC, no clock: `now` is
//! passed in, so every threshold is tested at the edge.

// Nodes are named a, b, x, y to match their sample names; `supply` returns the
// Option the `NodeObs` field takes.
#![allow(clippy::many_single_char_names, clippy::unnecessary_wraps)]

use maya_sentinel::detect::{Detector, Finding, Severity, Thresholds};
use maya_sentinel::obs::{NodeObs, Sample, Supply};
use primitive_types::U256;

fn obs(height: u64, id: &str, txs: usize) -> NodeObs {
    NodeObs {
        height,
        tip_id: id.to_owned(),
        tip_txs: txs,
        peers: Some(5),
        supply: None,
        pending: None,
        finalized: None,
        bft: None,
    }
}

fn up(name: &str, o: NodeObs) -> Sample {
    Sample {
        name: name.to_owned(),
        state: Ok(o),
    }
}

fn down(name: &str) -> Sample {
    Sample {
        name: name.to_owned(),
        state: Err("connection refused".to_owned()),
    }
}

fn supply(total: u64, circulating: u64, burned: u64) -> Option<Supply> {
    Some(Supply {
        total: U256::from(total),
        circulating: U256::from(circulating),
        burned: U256::from(burned),
    })
}

fn keys(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(|f| f.key.as_str()).collect()
}

#[test]
fn healthy_chain_raises_nothing() {
    let mut d = Detector::new(Thresholds::default());
    for i in 0..60 {
        let samples = [up("a", obs(10 + i, "aa", 0)), up("b", obs(10 + i, "aa", 0))];
        assert!(d.evaluate(i * 5_000, &samples).is_empty(), "poll {i}");
    }
}

#[test]
fn halt_fires_at_the_threshold_not_before() {
    let mut d = Detector::new(Thresholds::default());
    let samples = [up("a", obs(10, "aa", 0))];
    assert!(d.evaluate(0, &samples).is_empty());
    assert!(
        d.evaluate(29_999, &samples).is_empty(),
        "29.999 s is not a halt"
    );
    let f = d.evaluate(30_000, &samples);
    assert_eq!(keys(&f), ["halt"]);
    assert_eq!(f[0].severity, Severity::Critical);
}

#[test]
fn halt_clears_when_the_tip_advances() {
    let mut d = Detector::new(Thresholds::default());
    d.evaluate(0, &[up("a", obs(10, "aa", 0))]);
    assert_eq!(
        keys(&d.evaluate(40_000, &[up("a", obs(10, "aa", 0))])),
        ["halt"]
    );
    assert!(d.evaluate(45_000, &[up("a", obs(11, "bb", 0))]).is_empty());
}

#[test]
fn halt_ignores_a_lagging_node_when_another_advances() {
    let mut d = Detector::new(Thresholds::default());
    let lag = [up("a", obs(10, "aa", 0)), up("b", obs(9, "zz", 0))];
    d.evaluate(0, &lag);
    let ahead = [up("a", obs(11, "bb", 0)), up("b", obs(9, "zz", 0))];
    assert!(d.evaluate(20_000, &ahead).is_empty());
    assert!(
        d.evaluate(49_000, &ahead).is_empty(),
        "tip moved at 20 s, 29 s is short"
    );
    assert_eq!(keys(&d.evaluate(50_000, &ahead)), ["halt"]);
}

#[test]
fn fork_is_two_ids_at_one_height() {
    let mut d = Detector::new(Thresholds::default());
    let f = d.evaluate(
        0,
        &[
            up("a", obs(7, "aaaaaaaaaaaaaaaa", 0)),
            up("b", obs(7, "bbbbbbbbbbbbbbbb", 0)),
        ],
    );
    assert!(keys(&f).contains(&"fork:7"), "{f:?}");
    let f = d.evaluate(5_000, &[up("a", obs(7, "aa", 0)), up("b", obs(8, "bb", 0))]);
    assert!(
        !keys(&f).iter().any(|k| k.starts_with("fork")),
        "different heights are not a fork"
    );
}

#[test]
fn supply_must_add_up_and_agree_across_nodes() {
    let mut d = Detector::new(Thresholds::default());
    let mut a = obs(7, "aa", 0);
    a.supply = supply(100, 60, 30);
    let f = d.evaluate(0, &[up("a", a)]);
    assert_eq!(keys(&f), ["supply-mismatch:a"]);

    let mut x = obs(9, "cc", 0);
    x.supply = supply(100, 100, 0);
    let mut y = obs(9, "cc", 0);
    y.supply = supply(101, 101, 0);
    let f = d.evaluate(5_000, &[up("x", x), up("y", y)]);
    assert!(keys(&f).contains(&"supply-split:9"), "{f:?}");
}

#[test]
fn supply_falling_is_a_warning() {
    let mut d = Detector::new(Thresholds::default());
    let mut a = obs(5, "aa", 0);
    a.supply = supply(100, 100, 0);
    d.evaluate(0, &[up("a", a)]);
    let mut b = obs(6, "bb", 0);
    b.supply = supply(99, 99, 0);
    let f = d.evaluate(5_000, &[up("a", b)]);
    assert_eq!(keys(&f), ["supply-decrease:a"]);
    assert_eq!(f[0].severity, Severity::Warning);
}

#[test]
fn peer_collapse_below_the_floor() {
    let mut d = Detector::new(Thresholds::default());
    let mut a = obs(3, "aa", 0);
    a.peers = Some(1);
    let f = d.evaluate(0, &[up("a", a)]);
    assert_eq!(keys(&f), ["peers:a"]);
    let mut b = obs(4, "bb", 0);
    b.peers = Some(2);
    assert!(
        d.evaluate(5_000, &[up("a", b)]).is_empty(),
        "two is the floor, not below it"
    );
}

#[test]
fn unreachable_after_its_window() {
    // Node b keeps advancing, so the only finding can be the unreachable one.
    let live = |t: u64| up("b", obs(1 + t / 5_000, "x", 0));
    let mut d = Detector::new(Thresholds::default());
    assert!(d.evaluate(0, &[down("a"), live(0)]).is_empty());
    assert!(d.evaluate(44_999, &[down("a"), live(44_999)]).is_empty());
    let f = d.evaluate(45_000, &[down("a"), live(45_000)]);
    assert_eq!(keys(&f), ["rpc:a"]);
    assert_eq!(f[0].severity, Severity::Warning);
}

/// The benchmark pattern: transactions waiting, blocks keep finalizing, and
/// none of those blocks carries a transaction.
#[test]
fn stranding_by_block_count_resets_when_a_transaction_lands() {
    let mut d = Detector::new(Thresholds::default());
    let mut first = None;
    for i in 0..25u64 {
        let mut o = obs(100 + i, "aa", 0);
        o.pending = Some(5);
        let f = d.evaluate(i * 5_000, &[up("seed", o)]);
        if first.is_none() && keys(&f).contains(&"stranded") {
            first = Some(i);
        }
    }
    assert_eq!(first, Some(19), "20 empty blocks with transactions waiting");

    let mut landed = obs(125, "aa", 3);
    landed.pending = Some(2);
    let f = d.evaluate(125_000, &[up("seed", landed)]);
    assert!(
        !keys(&f).contains(&"stranded"),
        "a block that carries transactions resets the streak"
    );
}

#[test]
fn stranding_by_age_when_blocks_are_too_few_to_count() {
    let thresholds = Thresholds {
        strand_blocks: 10_000,
        ..Thresholds::default()
    };
    let mut d = Detector::new(thresholds);
    let mut fired_at = None;
    for i in 0..30u64 {
        let mut o = obs(100 + i, "aa", 0);
        o.pending = Some(1);
        if keys(&d.evaluate(i * 5_000, &[up("seed", o)])).contains(&"stranded")
            && fired_at.is_none()
        {
            fired_at = Some(i * 5_000);
        }
    }
    assert_eq!(
        fired_at,
        Some(120_000),
        "waiting 120 s with no block carrying one"
    );
}

#[test]
fn no_stranding_without_waiting_transactions() {
    let mut d = Detector::new(Thresholds::default());
    for i in 0..40u64 {
        let mut o = obs(100 + i, "aa", 0);
        o.pending = Some(0);
        assert!(d.evaluate(i * 5_000, &[up("seed", o)]).is_empty());
    }
}
