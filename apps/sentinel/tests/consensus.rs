//! The consensus-side detectors (ADR-101) at their thresholds: finality lag,
//! per-validator liveness, and the sentinel's own blindness.

use maya_sentinel::detect::{Detector, Finding, Severity, Thresholds};
use maya_sentinel::obs::{Bft, NodeObs, Sample};

/// A committee member at `height`, proposing in `round`, with the network
/// committed to `committed`, and the newest checkpoint at `finalized`.
fn val(name: &str, idx: u16, height: u64, round: u64, committed: u64, finalized: u64) -> Sample {
    Sample {
        name: name.to_owned(),
        state: Ok(NodeObs {
            height,
            tip_id: format!("{height:016x}"),
            tip_txs: 0,
            peers: Some(3),
            supply: None,
            pending: None,
            finalized: Some(finalized),
            bft: Some(Bft {
                round,
                committed_round: committed,
                validator: Some(idx),
                follower: false,
            }),
        }),
    }
}

fn keys(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(|f| f.key.as_str()).collect()
}

#[test]
fn finality_lag_fires_at_the_limit_not_before() {
    let mut d = Detector::new(Thresholds::default());
    let f = d.evaluate(0, &[val("a", 0, 130, 10, 9, 101)]);
    assert!(!keys(&f).contains(&"finality-lag"), "29 blocks: {f:?}");
    let f = d.evaluate(5_000, &[val("a", 0, 131, 11, 10, 101)]);
    let lag: Vec<&Finding> = f.iter().filter(|f| f.key == "finality-lag").collect();
    assert_eq!(lag.len(), 1, "30 blocks: {f:?}");
    assert_eq!(lag[0].severity, Severity::Warning);
}

#[test]
fn finality_uses_the_best_checkpoint_any_node_holds() {
    let mut d = Detector::new(Thresholds::default());
    // b is behind on checkpoints, a is not: the network is attesting.
    let f = d.evaluate(
        0,
        &[val("a", 0, 200, 9, 8, 198), val("b", 1, 200, 9, 8, 100)],
    );
    assert!(!keys(&f).contains(&"finality-lag"), "{f:?}");
}

#[test]
fn no_finality_alert_without_checkpoints() {
    let mut d = Detector::new(Thresholds::default());
    let mut s = val("a", 0, 10_000, 1, 1, 0);
    if let Ok(o) = &mut s.state {
        o.finalized = None;
    }
    assert!(!keys(&d.evaluate(0, &[s])).contains(&"finality-lag"));
}

/// Four validators; `d` stops proposing at round 10 while the rest commit.
#[test]
fn a_validator_that_stops_proposing_is_reported_after_its_window() {
    let mut d = Detector::new(Thresholds::default());
    let mut fired = None;
    for poll in 0..20u64 {
        let r = 10 + poll * 2;
        let h = 100 + poll;
        let samples = [
            val("a", 0, h, r, r - 1, h - 1),
            val("b", 1, h, r, r - 1, h - 1),
            val("c", 2, h, r, r - 1, h - 1),
            val("d", 3, h, 10, r - 1, h - 1),
        ];
        let f = d.evaluate(poll * 5_000, &samples);
        if fired.is_none() && keys(&f).contains(&"liveness:d") {
            fired = Some(poll * 5_000);
            assert_eq!(keys(&f), ["liveness:d"], "only d: {f:?}");
        }
    }
    assert_eq!(fired, Some(60_000), "round stuck since t=0, 60 s window");
}

#[test]
fn a_chain_wide_stall_is_a_halt_not_a_liveness_alert_per_validator() {
    let mut d = Detector::new(Thresholds::default());
    for poll in 0..30u64 {
        let samples = [val("a", 0, 50, 10, 9, 49), val("b", 1, 50, 10, 9, 49)];
        let f = d.evaluate(poll * 5_000, &samples);
        assert!(
            !keys(&f).iter().any(|k| k.starts_with("liveness")),
            "poll {poll}: {f:?}"
        );
    }
}

#[test]
fn observers_and_followers() {
    let mut d = Detector::new(Thresholds::default());
    let mut last = Vec::new();
    for poll in 0..14u64 {
        let r = 10 + poll;
        let mut obsv = val("o", 0, 100 + poll, 0, r, 99 + poll);
        let mut fol = val("f", 1, 100 + poll, 7, r, 99 + poll);
        if let (Ok(a), Ok(b)) = (&mut obsv.state, &mut fol.state) {
            a.bft = a.bft.map(|x| Bft {
                validator: None,
                ..x
            });
            b.bft = b.bft.map(|x| Bft {
                follower: true,
                ..x
            });
        }
        last = d.evaluate(
            poll * 5_000,
            &[obsv, fol, val("a", 2, 100 + poll, r, r, 99 + poll)],
        );
    }
    assert_eq!(
        keys(&last),
        ["liveness:f"],
        "observer never pages: {last:?}"
    );
    assert!(
        last[0].body.contains("following attested blocks"),
        "{}",
        last[0].body
    );
}

#[test]
fn blind_when_no_node_answers_for_the_window() {
    let down = |n: &str| Sample {
        name: n.to_owned(),
        state: Err("connection refused".to_owned()),
    };
    let mut d = Detector::new(Thresholds::default());
    assert!(!keys(&d.evaluate(0, &[val("a", 0, 1, 1, 1, 1)])).contains(&"blind"));
    assert!(!keys(&d.evaluate(44_999, &[down("a"), down("b")])).contains(&"blind"));
    let f = d.evaluate(45_000, &[down("a"), down("b")]);
    let blind: Vec<&Finding> = f.iter().filter(|f| f.key == "blind").collect();
    assert_eq!(blind.len(), 1, "{f:?}");
    assert_eq!(blind[0].severity, Severity::Critical);
    let f = d.evaluate(50_000, &[down("a"), val("b", 1, 2, 2, 2, 2)]);
    assert!(!keys(&f).contains(&"blind"), "one answer is sight: {f:?}");
}
