//! The live network view: who is connected, which validators are voting, and
//! what changed, rebuilt from a poll of each watched node once a second.
//!
//! Everything here is a pure function of what the nodes answered, so the
//! join/leave and up/down logic is tested without a network. `main` does the
//! polling and serves [`LiveView`] at `/api/live`.
//!
//! What it shows is measured, never estimated: if a node did not answer, its
//! row says so instead of carrying last poll's numbers.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

/// Events kept for the feed.
pub const EVENT_LOG: usize = 60;
/// Seconds of history kept for the sparklines.
pub const HISTORY: usize = 600;

/// One connected peer, as a node's `get_peers` reports it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PeerInfo {
    /// libp2p peer id.
    pub peer_id: String,
    /// `inbound` or `outbound`.
    pub direction: String,
    /// `tcp`, `websocket`, ...
    pub transport: String,
    /// Seconds connected.
    pub connected_secs: u64,
    /// Open connections.
    pub connections: u32,
    /// The node's health score for it.
    pub score: Option<f64>,
    /// Mean block propagation delay from it, ms.
    pub block_latency_ms: Option<u64>,
    /// Ignored for misbehaviour right now.
    pub quarantined: bool,
}

/// A node's `get_peers` answer.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NetworkView {
    /// The answering node's own peer id.
    pub peer_id: String,
    /// Its connections.
    pub peers: Vec<PeerInfo>,
}

/// A node's `get_bft_status` answer.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct BftStatus {
    /// Current epoch.
    pub epoch: u64,
    /// Current DAG round.
    pub round: u64,
    /// Newest committed round.
    pub committed_round: u64,
    /// This node's committee seat, if it has one.
    pub validator: Option<u64>,
    /// Committee size.
    pub committee: u64,
    /// Following rather than voting.
    pub follower: bool,
}

/// What one watched node answered this poll (`None`: no answer).
#[derive(Clone, Debug, Default)]
pub struct NodePoll {
    /// The watched node's label (`seed`, `v2`, ...).
    pub name: String,
    /// `get_bft_status`.
    pub bft: Option<BftStatus>,
    /// `get_peers`.
    pub view: Option<NetworkView>,
    /// `get_chain_info` height.
    pub height: Option<u64>,
}

/// One watched node in the view.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct WatchedNode {
    /// Label.
    pub name: String,
    /// Peer id, once it has answered `get_peers`.
    pub peer_id: Option<String>,
    /// Whether it answered this poll.
    pub up: bool,
    /// Its height.
    pub height: Option<u64>,
    /// Its consensus status.
    pub bft: Option<BftStatus>,
    /// Rounds its commits trail the furthest watched node's.
    pub lag_rounds: Option<u64>,
    /// Peers it is connected to.
    pub peers: usize,
    /// Voting: a seat, not a follower, answering.
    pub voting: bool,
}

/// A peer of any watched node, merged across them.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct NetworkPeer {
    /// Peer id.
    pub peer_id: String,
    /// The watched node's label when this peer is one of them.
    pub watched_as: Option<String>,
    /// Watched nodes it is connected to.
    pub linked_to: Vec<String>,
    /// Longest connection among those links, seconds.
    pub connected_secs: u64,
    /// `websocket` if any link is (it came in through the public relay).
    pub transport: String,
    /// Best health score any watched node gives it.
    pub score: Option<f64>,
    /// Lowest mean block latency any watched node measures from it, ms.
    pub block_latency_ms: Option<u64>,
    /// Any watched node is ignoring it.
    pub quarantined: bool,
    /// Unix ms the view first saw it this session.
    pub first_seen_ms: u64,
}

/// One line of the feed.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Event {
    /// Unix ms.
    pub at_ms: u64,
    /// `joined`, `left`, `node-down`, `node-up`, `seat`, `epoch`.
    pub kind: String,
    /// Plain sentence.
    pub text: String,
}

/// One second of history.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct Sample {
    /// Unix ms.
    pub at_ms: u64,
    /// Highest height any watched node reported.
    pub height: u64,
    /// Distinct peers.
    pub peers: usize,
    /// Watched nodes voting.
    pub voting: usize,
}

/// Everything `/api/live` serves.
#[derive(Clone, Debug, Serialize, Default)]
pub struct LiveView {
    /// When this view was built, unix ms.
    pub at_ms: u64,
    /// The network's name.
    pub network: String,
    /// Watched nodes, in the order they were given.
    pub nodes: Vec<WatchedNode>,
    /// Every peer, longest-connected first.
    pub peers: Vec<NetworkPeer>,
    /// Newest first.
    pub events: Vec<Event>,
    /// Oldest first, up to [`HISTORY`] samples.
    pub history: Vec<Sample>,
    /// Distinct peers seen since this service started.
    pub seen_total: usize,
}

/// State carried between polls.
#[derive(Debug, Default)]
pub struct Live {
    network: String,
    /// Peer ids named on the command line (`--known NAME=PEERID`), for
    /// watched nodes that do not serve `get_peers` yet and so cannot say
    /// who they are.
    known: BTreeMap<String, String>,
    first_seen: BTreeMap<String, u64>,
    present: BTreeSet<String>,
    up: BTreeMap<String, bool>,
    epoch: Option<u64>,
    events: VecDeque<Event>,
    history: VecDeque<Sample>,
    view: LiveView,
}

fn short(id: &str) -> String {
    let tail = id.len().saturating_sub(6);
    format!("…{}", id.get(tail..).unwrap_or(id))
}

impl Live {
    /// A view of `network` with nothing observed yet.
    #[must_use]
    pub fn new(network: impl Into<String>) -> Self {
        Self {
            network: network.into(),
            ..Self::default()
        }
    }

    /// The same view, naming `known` peer ids (peer id → node name).
    #[must_use]
    pub fn with_known(self, known: BTreeMap<String, String>) -> Self {
        Self { known, ..self }
    }

    /// The current view.
    #[must_use]
    pub fn view(&self) -> &LiveView {
        &self.view
    }

    fn push(&mut self, at_ms: u64, kind: &str, text: String) {
        self.events.push_front(Event {
            at_ms,
            kind: kind.to_owned(),
            text,
        });
        self.events.truncate(EVENT_LOG);
    }

    /// Folds one poll of every watched node into the view.
    pub fn observe(&mut self, at_ms: u64, polls: &[NodePoll]) {
        // What a node says about itself wins over the command line.
        let mut watched = self.known.clone();
        watched.extend(
            polls
                .iter()
                .filter_map(|p| Some((p.view.as_ref()?.peer_id.clone(), p.name.clone()))),
        );
        let peers = self.merge_peers(at_ms, polls, &watched);
        let nodes = self.watched_nodes(at_ms, polls, &watched);
        self.note_epoch(at_ms, polls);

        let sample = Sample {
            at_ms,
            height: polls.iter().filter_map(|p| p.height).max().unwrap_or(0),
            peers: peers.len(),
            voting: nodes.iter().filter(|n| n.voting).count(),
        };
        self.history.push_back(sample);
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        self.view = LiveView {
            at_ms,
            network: self.network.clone(),
            nodes,
            peers,
            events: self.events.iter().cloned().collect(),
            history: self.history.iter().copied().collect(),
            seen_total: self.first_seen.len(),
        };
    }

    fn merge_peers(
        &mut self,
        at_ms: u64,
        polls: &[NodePoll],
        watched: &BTreeMap<String, String>,
    ) -> Vec<NetworkPeer> {
        let mut merged: BTreeMap<String, NetworkPeer> = BTreeMap::new();
        for poll in polls {
            let Some(view) = &poll.view else { continue };
            for p in &view.peers {
                let first_seen_ms = *self.first_seen.entry(p.peer_id.clone()).or_insert(at_ms);
                let e = merged
                    .entry(p.peer_id.clone())
                    .or_insert_with(|| NetworkPeer {
                        peer_id: p.peer_id.clone(),
                        watched_as: watched.get(&p.peer_id).cloned(),
                        linked_to: Vec::new(),
                        connected_secs: 0,
                        transport: p.transport.clone(),
                        score: None,
                        block_latency_ms: None,
                        quarantined: false,
                        first_seen_ms,
                    });
                e.linked_to.push(poll.name.clone());
                e.connected_secs = e.connected_secs.max(p.connected_secs);
                if p.transport == "websocket" {
                    e.transport.clone_from(&p.transport);
                }
                e.score = match (e.score, p.score) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                e.block_latency_ms = match (e.block_latency_ms, p.block_latency_ms) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                e.quarantined |= p.quarantined;
            }
        }
        // Joins and leaves, judged only when at least one node answered: a
        // poll where every node was silent is an outage, not an exodus.
        if polls.iter().any(|p| p.view.is_some()) {
            for id in merged.keys() {
                if self.present.insert(id.clone()) {
                    let who = watched
                        .get(id)
                        .map_or_else(|| format!("peer {}", short(id)), Clone::clone);
                    self.push(at_ms, "joined", format!("{who} connected"));
                }
            }
            let gone: Vec<String> = self
                .present
                .iter()
                .filter(|id| !merged.contains_key(*id))
                .cloned()
                .collect();
            for id in gone {
                self.present.remove(&id);
                let who = watched
                    .get(&id)
                    .map_or_else(|| format!("peer {}", short(&id)), Clone::clone);
                self.push(at_ms, "left", format!("{who} disconnected"));
            }
        }
        let mut out: Vec<NetworkPeer> = merged.into_values().collect();
        out.sort_by(|a, b| {
            b.connected_secs
                .cmp(&a.connected_secs)
                .then_with(|| a.peer_id.cmp(&b.peer_id))
        });
        out
    }

    fn watched_nodes(
        &mut self,
        at_ms: u64,
        polls: &[NodePoll],
        watched: &BTreeMap<String, String>,
    ) -> Vec<WatchedNode> {
        let best = polls
            .iter()
            .filter_map(|p| p.bft.map(|b| b.committed_round))
            .max();
        let mut out = Vec::with_capacity(polls.len());
        for p in polls {
            let up = p.bft.is_some() || p.height.is_some();
            let was = self.up.insert(p.name.clone(), up);
            match (was, up) {
                (Some(true), false) => {
                    self.push(at_ms, "node-down", format!("{} stopped answering", p.name));
                }
                (Some(false), true) => self.push(at_ms, "node-up", format!("{} is back", p.name)),
                _ => {}
            }
            out.push(WatchedNode {
                name: p.name.clone(),
                peer_id: p.view.as_ref().map(|v| v.peer_id.clone()).or_else(|| {
                    watched
                        .iter()
                        .find(|(_, name)| **name == p.name)
                        .map(|(id, _)| id.clone())
                }),
                up,
                height: p.height,
                bft: p.bft,
                lag_rounds: match (best, p.bft) {
                    (Some(best), Some(b)) => Some(best.saturating_sub(b.committed_round)),
                    _ => None,
                },
                peers: p.view.as_ref().map_or(0, |v| v.peers.len()),
                voting: p.bft.is_some_and(|b| b.validator.is_some() && !b.follower),
            });
        }
        out
    }

    fn note_epoch(&mut self, at_ms: u64, polls: &[NodePoll]) {
        let Some(epoch) = polls.iter().filter_map(|p| p.bft.map(|b| b.epoch)).max() else {
            return;
        };
        if self.epoch.is_some_and(|e| e < epoch) {
            let committee = polls
                .iter()
                .filter_map(|p| p.bft.map(|b| b.committee))
                .max()
                .unwrap_or(0);
            self.push(
                at_ms,
                "epoch",
                format!("epoch {epoch} began, committee of {committee}"),
            );
        }
        self.epoch = Some(epoch);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn peer(id: &str, secs: u64, transport: &str) -> PeerInfo {
        PeerInfo {
            peer_id: id.into(),
            direction: "inbound".into(),
            transport: transport.into(),
            connected_secs: secs,
            connections: 1,
            score: Some(1.0),
            block_latency_ms: Some(40),
            quarantined: false,
        }
    }

    fn poll(
        name: &str,
        own: &str,
        peers: Vec<PeerInfo>,
        seat: Option<u64>,
        round: u64,
    ) -> NodePoll {
        NodePoll {
            name: name.into(),
            bft: Some(BftStatus {
                epoch: 1,
                round,
                committed_round: round,
                validator: seat,
                committee: 4,
                follower: false,
            }),
            view: Some(NetworkView {
                peer_id: own.into(),
                peers,
            }),
            height: Some(100),
        }
    }

    #[test]
    fn a_new_peer_appears_with_a_joined_event_and_leaves_with_a_left_one() {
        let mut live = Live::new("maya-testnet-1");
        live.observe(1_000, &[poll("seed", "S", vec![], Some(0), 10)]);
        assert!(live.view().peers.is_empty());

        live.observe(
            2_000,
            &[poll(
                "seed",
                "S",
                vec![peer("FRIEND1", 1, "websocket")],
                Some(0),
                11,
            )],
        );
        let v = live.view();
        assert_eq!(v.peers.len(), 1);
        assert_eq!(v.peers[0].first_seen_ms, 2_000);
        assert_eq!(v.events[0].kind, "joined");
        assert!(v.events[0].text.contains("RIEND1"));

        live.observe(3_000, &[poll("seed", "S", vec![], Some(0), 12)]);
        assert_eq!(live.view().events[0].kind, "left");
        assert_eq!(live.view().seen_total, 1);
    }

    #[test]
    fn watched_nodes_are_named_and_peers_merge_across_them() {
        let mut live = Live::new("t");
        live.observe(
            1_000,
            &[
                poll(
                    "seed",
                    "S",
                    vec![peer("V2", 50, "tcp"), peer("X", 5, "websocket")],
                    Some(0),
                    20,
                ),
                poll(
                    "v2",
                    "V2",
                    vec![peer("S", 50, "tcp"), peer("X", 9, "tcp")],
                    Some(1),
                    18,
                ),
            ],
        );
        let v = live.view();
        let x = v.peers.iter().find(|p| p.peer_id == "X").unwrap();
        assert_eq!(x.linked_to, vec!["seed".to_string(), "v2".to_string()]);
        assert_eq!(x.connected_secs, 9);
        assert_eq!(x.transport, "websocket");
        let v2 = v.peers.iter().find(|p| p.peer_id == "V2").unwrap();
        assert_eq!(v2.watched_as.as_deref(), Some("v2"));
        assert_eq!(v.nodes[1].lag_rounds, Some(2));
        assert!(v.nodes.iter().all(|n| n.voting));
    }

    #[test]
    fn a_known_peer_id_names_a_node_that_cannot_say_who_it_is() {
        let known = BTreeMap::from([("V3ID".to_string(), "v3".to_string())]);
        let mut live = Live::new("t").with_known(known);
        let old_v3 = NodePoll {
            name: "v3".into(),
            bft: Some(BftStatus {
                validator: Some(1),
                committee: 4,
                ..BftStatus::default()
            }),
            view: None, // an old binary: no get_peers
            height: Some(5),
        };
        live.observe(
            1_000,
            &[
                poll("seed", "S", vec![peer("V3ID", 30, "tcp")], Some(0), 3),
                old_v3,
            ],
        );
        let v = live.view();
        assert_eq!(v.peers[0].watched_as.as_deref(), Some("v3"));
        assert_eq!(v.nodes[1].peer_id.as_deref(), Some("V3ID"));
        assert!(v.events[0].text.starts_with("v3 "), "{:?}", v.events);
    }

    #[test]
    fn a_silent_poll_is_an_outage_not_an_exodus() {
        let mut live = Live::new("t");
        live.observe(
            1_000,
            &[poll("seed", "S", vec![peer("A", 1, "tcp")], Some(0), 1)],
        );
        live.observe(
            2_000,
            &[NodePoll {
                name: "seed".into(),
                ..NodePoll::default()
            }],
        );
        let v = live.view();
        assert!(!v.nodes[0].up);
        assert_eq!(v.events[0].kind, "node-down");
        assert!(v.events.iter().all(|e| e.kind != "left"), "{:?}", v.events);
    }

    #[test]
    fn history_is_bounded_and_epochs_are_announced() {
        let mut live = Live::new("t");
        for i in 0..(HISTORY as u64 + 10) {
            let mut p = poll("seed", "S", vec![], Some(0), i);
            if let Some(b) = p.bft.as_mut() {
                b.epoch = 1 + i / 500;
            }
            live.observe(i * 1_000, &[p]);
        }
        assert_eq!(live.view().history.len(), HISTORY);
        assert!(live.view().events.iter().any(|e| e.kind == "epoch"));
    }
}
