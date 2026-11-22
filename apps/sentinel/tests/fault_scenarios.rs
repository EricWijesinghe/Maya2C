//! The network faults of the netfault relay (#453, ADR-097) replayed as what
//! a 4-validator network's RPC would report under each: partition (2|2 and
//! 3|1, with heal), latency with jitter, and loss as retransmission stalls.
//! Each run goes through the detectors and the incident layer, and checks the
//! pages a person would get: which, when, and that each raises and resolves
//! once.
//!
//! These are models of the RPC answers, not a run of the relay: the relay's
//! end-to-end scenarios have not been run yet (see #453).

use std::collections::BTreeSet;

use maya_sentinel::detect::{Detector, Thresholds};
use maya_sentinel::incident::{Event, Incidents};
use maya_sentinel::obs::{Bft, NodeObs, Sample};

const POLL_MS: u64 = 5_000;
const CLEAR_MS: u64 = 30_000;
const NAMES: [&str; 4] = ["a", "b", "c", "d"];

/// What one validator's RPC answers in one poll.
#[derive(Clone, Copy)]
struct View {
    height: u64,
    finalized: u64,
    round: u64,
    committed: u64,
    peers: usize,
}

/// A healthy network at `poll`: one block per poll, two rounds per block,
/// checkpoints two blocks behind, everyone connected.
fn healthy(poll: u64) -> View {
    let height = 100 + poll;
    View {
        height,
        finalized: height - 2,
        round: height * 2,
        committed: height * 2 - 2,
        peers: 3,
    }
}

fn sample(idx: usize, v: View) -> Sample {
    Sample {
        name: NAMES[idx].to_owned(),
        state: Ok(NodeObs {
            height: v.height,
            // One id per height: DAG-BFT does not fork, so neither does the model.
            tip_id: format!("{:016x}", v.height),
            tip_txs: 1,
            peers: Some(v.peers),
            supply: None,
            pending: Some(0),
            finalized: Some(v.finalized),
            bft: Some(Bft {
                round: v.round,
                committed_round: v.committed,
                validator: u16::try_from(idx).ok(),
                follower: false,
            }),
        }),
    }
}

/// Runs `polls` polls; `views(poll)` gives each validator's view.
fn replay(polls: u64, views: impl Fn(u64) -> [View; 4]) -> Vec<(u64, Event)> {
    let mut detector = Detector::new(Thresholds::default());
    let mut incidents = Incidents::new(CLEAR_MS);
    let mut events = Vec::new();
    for poll in 0..polls {
        let samples: Vec<Sample> = views(poll)
            .into_iter()
            .enumerate()
            .map(|(i, v)| sample(i, v))
            .collect();
        let now = poll * POLL_MS;
        for e in incidents.update(now, detector.evaluate(now, &samples)) {
            events.push((poll, e));
        }
    }
    events
}

fn raised(events: &[(u64, Event)]) -> BTreeSet<String> {
    events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::Raised(f) => Some(f.key.clone()),
            Event::Resolved { .. } => None,
        })
        .collect()
}

/// `(raised at poll, resolved at poll)` for one key; panics on anything but
/// exactly one raise followed by one resolve.
fn once(events: &[(u64, Event)], key: &str) -> (u64, u64) {
    let mine: Vec<&(u64, Event)> = events
        .iter()
        .filter(|(_, e)| match e {
            Event::Raised(f) => f.key == key,
            Event::Resolved { key: k, .. } => k == key,
        })
        .collect();
    match mine.as_slice() {
        [(r, Event::Raised(_)), (c, Event::Resolved { .. })] => (*r, *c),
        other => panic!("{key}: expected one raise then one resolve, got {other:?}"),
    }
}

/// 2|2 split: neither side has `n - f = 3`, so nothing commits anywhere.
/// One halt page, four peer pages, no fork and no per-validator noise.
#[test]
fn even_partition_halts_and_heals() {
    let frozen = healthy(9);
    let events = replay(45, |poll| {
        let v = match poll {
            0..=9 => healthy(poll),
            10..=25 => View { peers: 1, ..frozen },
            _ => healthy(poll - 16),
        };
        [v; 4]
    });
    let expected: BTreeSet<String> = ["halt", "peers:a", "peers:b", "peers:c", "peers:d"]
        .map(String::from)
        .into();
    assert_eq!(raised(&events), expected);
    // Tip last moved at poll 9 (t=45 s): halt at t=75 s; the tip moves again
    // at poll 26 (t=130 s), so the last halt finding is t=125 s and it clears
    // 30 s later.
    assert_eq!(once(&events, "halt"), (15, 31));
    for n in NAMES {
        assert_eq!(once(&events, &format!("peers:{n}")), (10, 31));
    }
}

/// 3|1 split: the majority keeps committing; `d` is cut off. `d` is paged as a
/// validator that stopped proposing, never as a halt or a fork.
#[test]
fn minority_partition_pages_the_isolated_validator_only() {
    let stuck = healthy(9);
    let events = replay(45, |poll| {
        let h = healthy(poll);
        let d = match poll {
            10..=30 => View { peers: 0, ..stuck },
            _ => h,
        };
        [h, h, h, d]
    });
    let expected: BTreeSet<String> = ["liveness:d", "peers:d"].map(String::from).into();
    assert_eq!(raised(&events), expected);
    // d's round last moved at poll 9 (t=45 s): 60 s window -> t=105 s.
    // It rejoins at poll 31; last finding at t=150 s clears at t=180 s.
    assert_eq!(once(&events, "liveness:d"), (21, 36));
    assert_eq!(once(&events, "peers:d"), (10, 36));
}

/// Latency with jitter: blocks come one to three per poll and checkpoints
/// trail by up to eleven blocks. Below every threshold: no page at all.
#[test]
fn latency_with_jitter_raises_nothing() {
    // Fixed-seed LCG, so the run replays the same jitter every time.
    let jitter = |poll: u64| poll.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1) >> 33;
    let events = replay(120, |poll| {
        let height = 100 + (0..=poll).map(|p| 1 + jitter(p) % 3).sum::<u64>();
        let v = View {
            height,
            finalized: height - 2 - jitter(poll) % 10,
            round: height * 2,
            committed: height * 2 - 2,
            peers: 3,
        };
        [v; 4]
    });
    assert!(events.is_empty(), "{events:?}");
}

/// Loss as retransmission stalls: progress arrives in bursts every 30 s, so
/// the longest silence a poll sees is 25 s, below the 30 s halt window.
#[test]
fn bursty_loss_below_the_halt_window_raises_nothing() {
    let events = replay(80, |poll| [healthy(poll - poll % 6); 4]);
    assert!(events.is_empty(), "{events:?}");
}

/// Loss that eats attestation frames: blocks keep coming, checkpoints stop.
/// One finality page, resolved once attestations get through again.
#[test]
fn attestation_loss_pages_finality_lag_once() {
    let events = replay(60, |poll| {
        let mut v = healthy(poll);
        if (10..=45).contains(&poll) {
            v.finalized = healthy(9).finalized;
        }
        [v; 4]
    });
    let expected: BTreeSet<String> = ["finality-lag"].map(String::from).into();
    assert_eq!(raised(&events), expected);
    // Checkpoint frozen at 107; tip 100+poll reaches 137 at poll 37.
    assert_eq!(once(&events, "finality-lag"), (37, 51));
}
