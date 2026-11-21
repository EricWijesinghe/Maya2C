//! Red team: the sentinel's two promises, read-only and one page per incident,
//! against a recording transport, hostile answers and a flapping condition.

use std::future::Future;
use std::sync::Mutex;

use maya_sentinel::detect::{Detector, Finding, Severity, Thresholds};
use maya_sentinel::incident::{Event, Incidents};
use maya_sentinel::obs::{NodeObs, Sample, Transport, gather};
use serde_json::{Value, json};

/// Every getter the sentinel is allowed to call. A new method has to be added
/// here on purpose, which is the review point for "still read-only".
const GETTERS: [&str; 7] = [
    "get_chain_info",
    "get_block_by_height",
    "get_peers",
    "get_mempool_info",
    "get_checkpoint",
    "get_bft_status",
    "get_supply",
];

/// Answers like a healthy node and writes down every method it was asked.
struct Recording {
    calls: Mutex<Vec<&'static str>>,
    supply: Value,
}

impl Recording {
    fn new(supply: Value) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            supply,
        }
    }
}

impl Transport for Recording {
    fn call(
        &self,
        method: &'static str,
        _args: Vec<u64>,
    ) -> impl Future<Output = Result<Value, String>> + Send {
        self.calls.lock().expect("lock").push(method);
        let answer = match method {
            "get_chain_info" => Ok(json!({ "height": 9 })),
            "get_block_by_height" => Ok(json!({
                "height": 9, "header": { "id": "ab".repeat(32) }, "transactions": [],
            })),
            "get_peers" => Ok(json!({ "peers": [{}, {}, {}] })),
            "get_mempool_info" => Ok(json!({ "size": 0 })),
            "get_checkpoint" => Ok(json!({ "height": 8 })),
            "get_bft_status" => Ok(json!({
                "round": 3, "committed_round": 2, "validator": 0, "follower": false,
            })),
            "get_supply" => Ok(self.supply.clone()),
            other => Err(format!("{other}: method not found")),
        };
        async move { answer }
    }
}

fn healthy_supply() -> Value {
    json!({ "total": "100", "circulating": "90", "burned": "10" })
}

#[tokio::test]
async fn a_full_poll_calls_getters_only() {
    let nodes = vec![("a".to_owned(), Recording::new(healthy_supply()))];
    let samples = gather(&nodes, true).await;
    assert!(samples[0].state.is_ok(), "{:?}", samples[0].state);
    let calls = nodes[0].1.calls.lock().expect("lock").clone();
    assert!(
        calls.contains(&"get_supply"),
        "the widest poll was exercised"
    );
    for method in calls {
        assert!(GETTERS.contains(&method), "non-getter RPC call: {method}");
    }
}

#[tokio::test]
async fn a_hostile_supply_answer_degrades_to_no_supply_not_a_panic() {
    for bad in [
        json!({ "total": "-1", "circulating": "0", "burned": "0" }),
        json!({ "total": "9".repeat(200), "circulating": "0", "burned": "0" }),
        json!({ "total": 100, "circulating": 90, "burned": 10 }),
        json!("not an object"),
    ] {
        let nodes = vec![("a".to_owned(), Recording::new(bad.clone()))];
        let samples = gather(&nodes, true).await;
        let obs = samples[0]
            .state
            .as_ref()
            .expect("the node is still observed");
        assert_eq!(obs.supply, None, "{bad}");
    }
}

fn finding(key: &str) -> Finding {
    Finding {
        key: key.to_owned(),
        severity: Severity::Critical,
        title: format!("{key} title"),
        body: String::new(),
    }
}

#[test]
fn a_flapping_condition_pages_once_and_resolves_once() {
    let mut incidents = Incidents::new(60_000);
    let mut events = Vec::new();
    // Present, absent for less than the clear window, present, then gone.
    for (now, on) in [(0, true), (10_000, false), (20_000, true), (50_000, false)] {
        let findings = if on { vec![finding("halt")] } else { vec![] };
        events.extend(incidents.update(now, findings));
    }
    events.extend(incidents.update(80_000, vec![]));
    events.extend(incidents.update(200_000, vec![]));
    assert_eq!(
        events,
        vec![
            Event::Raised(finding("halt")),
            Event::Resolved {
                key: "halt".into(),
                title: "halt title".into()
            },
        ]
    );
}

#[test]
fn a_finding_repeated_within_one_poll_is_one_page() {
    let mut incidents = Incidents::new(60_000);
    let events = incidents.update(0, vec![finding("fork:7"), finding("fork:7")]);
    assert_eq!(events, vec![Event::Raised(finding("fork:7"))]);
}

fn up(name: &str, height: u64) -> Sample {
    Sample {
        name: name.to_owned(),
        state: Ok(NodeObs {
            height,
            tip_id: format!("{height:064x}"),
            tip_txs: 1,
            peers: Some(5),
            supply: None,
            pending: None,
            finalized: None,
            bft: None,
        }),
    }
}

fn down(name: &str) -> Sample {
    Sample {
        name: name.to_owned(),
        state: Err("connection refused".to_owned()),
    }
}

/// Every node goes dark, then comes back still at the old tip: the chain did
/// not move in between, and the halt clock must not have restarted.
#[test]
fn nodes_that_return_at_the_old_tip_after_an_outage_report_a_halt() {
    let mut d = Detector::new(Thresholds::default());
    assert!(d.evaluate(0, &[up("a", 10), up("b", 10)]).is_empty());
    for now in [10_000, 20_000] {
        let keys: Vec<String> = d
            .evaluate(now, &[down("a"), down("b")])
            .into_iter()
            .map(|f| f.key)
            .collect();
        assert!(!keys.contains(&"halt".to_owned()), "no tip to judge yet");
    }
    let keys: Vec<String> = d
        .evaluate(31_000, &[up("a", 10), up("b", 10)])
        .into_iter()
        .map(|f| f.key)
        .collect();
    assert!(keys.contains(&"halt".to_owned()), "{keys:?}");
}

/// A node that restarts and resyncs from below reports a lower height and a
/// lower total on the way up; that is not supply falling.
#[test]
fn a_resyncing_node_is_not_a_supply_decrease() {
    let with = |h: u64, total: u64| {
        let mut s = up("a", h);
        if let Ok(o) = s.state.as_mut() {
            o.supply = Some(maya_sentinel::obs::Supply {
                total: total.into(),
                circulating: total.into(),
                burned: 0u64.into(),
            });
        }
        s
    };
    let mut d = Detector::new(Thresholds::default());
    d.evaluate(0, &[with(100, 1_000)]);
    let keys: Vec<String> = d
        .evaluate(1_000, &[with(5, 50)])
        .into_iter()
        .map(|f| f.key)
        .collect();
    assert!(
        !keys.iter().any(|k| k.starts_with("supply-decrease")),
        "{keys:?}"
    );
}
