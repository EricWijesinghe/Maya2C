//! Replays a scripted node through the whole path: RPC answers, `observe`,
//! the detectors, and the incident layer. The scenario is the stranding bug
//! from the benchmark: consensus keeps finalizing, but the blocks are empty
//! while transactions wait in the mempool.

#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use std::future::Future;
use std::sync::Mutex;

use maya_sentinel::detect::{Detector, Thresholds};
use maya_sentinel::incident::{Event, Incidents};
use maya_sentinel::obs::{Sample, Transport, gather};
use serde_json::{Value, json};

/// The node under test: its tip, the transactions in that tip, and the mempool.
struct Node {
    height: Mutex<u64>,
    tip_txs: Mutex<usize>,
    pending: Mutex<Option<u64>>,
}

impl Node {
    fn new(height: u64, tip_txs: usize, pending: Option<u64>) -> Self {
        Self {
            height: Mutex::new(height),
            tip_txs: Mutex::new(tip_txs),
            pending: Mutex::new(pending),
        }
    }

    fn set(&self, height: u64, tip_txs: usize, pending: Option<u64>) {
        *self.height.lock().unwrap() = height;
        *self.tip_txs.lock().unwrap() = tip_txs;
        *self.pending.lock().unwrap() = pending;
    }

    fn answer(&self, method: &str, args: &[u64]) -> Result<Value, String> {
        let height = *self.height.lock().unwrap();
        match method {
            "get_chain_info" => Ok(json!({ "height": height, "chain_id": "replay" })),
            "get_block_by_height" if args.first() == Some(&height) => {
                let txs = vec![json!({}); *self.tip_txs.lock().unwrap()];
                Ok(json!({
                    "height": height,
                    "header": { "id": format!("{height:064x}") },
                    "transactions": txs,
                }))
            }
            "get_peers" => Ok(json!({ "peer_id": "replay", "peers": [{}, {}, {}] })),
            "get_mempool_info" => match *self.pending.lock().unwrap() {
                Some(p) => Ok(json!({ "pending": p })),
                None => Err("method not found".to_owned()),
            },
            _ => Err(format!("{method}: not scripted")),
        }
    }
}

/// Borrowed view of a node as a transport.
struct Wire<'a>(&'a Node);

impl Transport for Wire<'_> {
    fn call(
        &self,
        method: &'static str,
        args: Vec<u64>,
    ) -> impl Future<Output = Result<Value, String>> + Send {
        let answer = self.0.answer(method, &args);
        async move { answer }
    }
}

const POLL_MS: u64 = 5_000;

/// Runs `polls` rounds. `script` sets the node state before each round.
async fn run(node: &Node, polls: u64, mut script: impl FnMut(u64, &Node)) -> Vec<(u64, Event)> {
    let mut detector = Detector::new(Thresholds::default());
    let mut incidents = Incidents::new(30_000);
    let mut events = Vec::new();
    for poll in 0..polls {
        script(poll, node);
        let now = poll * POLL_MS;
        let nodes = [("seed".to_owned(), Wire(node))];
        let samples: Vec<Sample> = gather(&nodes, false).await;
        for e in incidents.update(now, detector.evaluate(now, &samples)) {
            events.push((poll, e));
        }
    }
    events
}

fn key_of(e: &Event) -> &str {
    match e {
        Event::Raised(f) => f.key.as_str(),
        Event::Resolved { key, .. } => key.as_str(),
    }
}

/// Each block is one higher and empty, while five transactions wait.
#[tokio::test]
async fn stranding_pattern_raises_once_and_resolves_when_transactions_land() {
    let node = Node::new(100, 0, Some(5));
    let events = run(&node, 40, |poll, n| {
        let height = 100 + poll;
        // Transactions get into a block from poll 30: the chain recovers.
        if poll < 30 {
            n.set(height, 0, Some(5));
        } else {
            n.set(height, 2, Some(0));
        }
    })
    .await;

    let stranded: Vec<(u64, &Event)> = events
        .iter()
        .filter(|(_, e)| key_of(e) == "stranded")
        .map(|(p, e)| (*p, e))
        .collect();
    assert_eq!(
        stranded.len(),
        2,
        "one raise and one resolve, got {stranded:?}"
    );
    assert!(matches!(stranded[0].1, Event::Raised(_)));
    assert_eq!(
        stranded[0].0, 19,
        "raised on the 20th empty block with transactions waiting"
    );
    assert!(matches!(stranded[1].1, Event::Resolved { .. }));
    assert_eq!(
        stranded[1].0, 35,
        "resolves after 30 s clear, 6 polls after the last stranded poll"
    );
    assert!(
        !events.iter().any(|(_, e)| key_of(e) == "halt"),
        "the chain kept finalizing, so this is not a halt"
    );
}

/// Frozen tip with the node answering: one halt incident, no repeats, and
/// one resolve after the tip moves again.
#[tokio::test]
async fn frozen_tip_is_one_halt_incident() {
    let node = Node::new(50, 0, Some(0));
    let events = run(&node, 40, |poll, n| {
        if poll < 10 {
            n.set(50 + poll, 0, Some(0));
        } else if poll == 10 {
            n.set(60, 0, Some(0));
        } else if poll >= 30 {
            n.set(61 + poll - 30, 0, Some(0));
        }
    })
    .await;

    let halts: Vec<&(u64, Event)> = events.iter().filter(|(_, e)| key_of(e) == "halt").collect();
    assert_eq!(halts.len(), 2, "one raise and one resolve: {halts:?}");
    assert!(matches!(halts[0].1, Event::Raised(_)));
    assert_eq!(
        halts[0].0, 16,
        "tip frozen at poll 10 (t=50 s), raised at t=80 s"
    );
    assert!(matches!(halts[1].1, Event::Resolved { .. }));
}

/// A node without the mempool method never reports stranding: the detector
/// stays silent instead of guessing.
#[tokio::test]
async fn no_mempool_method_means_no_stranding_alert() {
    let node = Node::new(10, 0, None);
    let events = run(&node, 40, |poll, n| n.set(10 + poll, 0, None)).await;
    assert!(events.iter().all(|(_, e)| key_of(e) != "stranded"));
}
