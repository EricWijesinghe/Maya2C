//! Adversarial-schedule checking of the real DAG-BFT engine.
//!
//! The thresholds are proved in `property_tests.rs`; this file attacks the
//! engine that uses them. Four [`Validator`]s run unchanged code. The adversary
//! owns the network and validator 3 (f = 1):
//!
//! - **Network:** delivers in-flight messages in any order, drops them, and
//!   duplicates them; advances any node's clock.
//! - **Byzantine author:** equivocates (a second vertex for a round it already
//!   proposed, sent to a chosen subset), harvests every vote addressed to it,
//!   and broadcasts a certificate for *any* vertex that reaches a quorum.
//!
//! What it may not do is sign as an honest validator: in the node every vote
//! and proposal is ML-DSA-65 signed, so the injected messages are limited to
//! author/voter 3, exactly the power a real Byzantine key-holder has.
//!
//! Checked on every schedule:
//! - **Agreement:** any two honest nodes' committed sequences are prefixes of
//!   one another (deterministic finality: nothing committed is ever undone).
//! - **No duplicate commit** on a node.
//! - **Certificate uniqueness:** across honest DAGs, one certified vertex per
//!   (round, author), even for the equivocating author.
//! - **Liveness after GST:** once the network turns reliable and validator 3
//!   crashes, every honest node commits again within a bounded number of steps.
//!
//! `proptest` shrinks a failing schedule to a short one, and the
//! `MAYA_SCHEDULE_CASES` environment variable raises the case count for the
//! nightly run without changing the default CI cost.

use std::collections::{BTreeMap, BTreeSet};

use maya_dag_bft::{
    Certificate, Committee, Committer, Dag, Dest, Digest, Message, Output, Params, Validator,
    ValidatorId, Vertex,
};
use proptest::prelude::*;

const N: u16 = 4;
const BYZ: ValidatorId = 3;
/// Clock step per tick action: four of them pass the default anchor timeout.
const TICK_MS: u64 = 300;
/// Steps the reliable phase may take before liveness is declared failed.
const GST_STEPS: usize = 4_000;
/// New commits each honest node must make after GST.
const GST_PROGRESS: usize = 2;
const DEFAULT_CASES: u32 = 192;
/// Long enough for a schedule to cover several anchor rounds.
const MAX_ACTIONS: usize = 3_000;

#[derive(Clone, Copy, Debug)]
enum Action {
    Deliver(u16),
    Drop(u16),
    Duplicate(u16),
    Tick(u16),
    /// Re-send validator 3's latest proposal with a different batch to the
    /// honest nodes selected by the low three bits.
    Equivocate(u8),
    /// Hold back every message to one node (`id % (N + 1) == N` releases it),
    /// so nodes reach a round with different views of its anchor: the case
    /// the indirect commit rule exists for.
    Starve(u8),
}

fn action() -> impl Strategy<Value = Action> {
    prop_oneof![
        6 => any::<u16>().prop_map(Action::Deliver),
        1 => any::<u16>().prop_map(Action::Drop),
        1 => any::<u16>().prop_map(Action::Duplicate),
        2 => any::<u16>().prop_map(Action::Tick),
        1 => any::<u8>().prop_map(Action::Equivocate),
        1 => any::<u8>().prop_map(Action::Starve),
    ]
}

struct InFlight {
    to: ValidatorId,
    from: ValidatorId,
    msg: Message,
}

struct World {
    committee: Committee,
    nodes: Vec<Validator>,
    clocks: Vec<u64>,
    net: Vec<InFlight>,
    committed: Vec<Vec<Digest>>,
    /// Votes addressed to the Byzantine author, per vertex digest.
    harvested: BTreeMap<Digest, (Vertex, BTreeSet<ValidatorId>)>,
    /// Vertices validator 3 has proposed, by digest, so a harvested vote can be
    /// matched to the vertex it certifies.
    byz_vertices: BTreeMap<Digest, Vertex>,
    forged: BTreeSet<Digest>,
    starved: Option<ValidatorId>,
}

impl World {
    fn new(committee: Committee) -> Self {
        // The node's own pacing (bins/maya2c-node/src/bft.rs): re-broadcast at
        // half the anchor timeout. The simulator's zero re-sends on every
        // tick, which on top of an adversarial backlog floods the reliable
        // phase and measures the flood, not the protocol.
        let defaults = Params::default();
        let params = Params {
            resend_interval_ms: defaults.anchor_timeout_ms / 2,
            ..defaults
        };
        let nodes: Vec<_> = (0..N)
            .map(|i| Validator::new(i, committee.clone(), params))
            .collect();
        let mut w = Self {
            committee,
            nodes,
            clocks: vec![0; N as usize],
            net: Vec::new(),
            committed: vec![Vec::new(); N as usize],
            harvested: BTreeMap::new(),
            byz_vertices: BTreeMap::new(),
            forged: BTreeSet::new(),
            starved: None,
        };
        for i in 0..N {
            let out = w.nodes[i as usize].start(0);
            w.absorb(i, out);
        }
        w
    }

    fn absorb(&mut self, from: ValidatorId, out: Output) {
        for c in &out.committed {
            self.committed[from as usize].push(c.digest());
        }
        for (dest, msg) in out.sends {
            if from == BYZ
                && let Message::Propose { vertex, .. } = &msg
            {
                self.byz_vertices.insert(vertex.digest(), vertex.clone());
            }
            match dest {
                Dest::All => {
                    for to in (0..N).filter(|&t| t != from) {
                        self.send(to, from, msg.clone());
                    }
                }
                Dest::To(to) => self.send(to, from, msg),
            }
        }
    }

    fn send(&mut self, to: ValidatorId, from: ValidatorId, msg: Message) {
        if to == BYZ
            && let Message::Vote { digest, voter, .. } = &msg
        {
            self.harvest(*digest, *voter);
        }
        self.net.push(InFlight { to, from, msg });
    }

    /// The Byzantine author certifies anything a quorum voted for, including
    /// an equivocation its own engine would never have certified.
    fn harvest(&mut self, digest: Digest, voter: ValidatorId) {
        let Some(vertex) = self.byz_vertices.get(&digest).cloned() else {
            return;
        };
        let entry = self
            .harvested
            .entry(digest)
            .or_insert_with(|| (vertex, BTreeSet::from([BYZ])));
        entry.1.insert(voter);
        if self.committee.is_quorum(entry.1.iter().copied()) && self.forged.insert(digest) {
            let votes: Vec<_> = entry.1.iter().copied().collect();
            let cert = Certificate {
                vertex: entry.0.clone(),
                signatures: vec![Vec::new(); votes.len()],
                votes,
            };
            for to in 0..BYZ {
                self.net.push(InFlight {
                    to,
                    from: BYZ,
                    msg: Message::Cert(cert.clone()),
                });
            }
        }
    }

    fn deliver(&mut self, i: usize) {
        let m = self.net.remove(i);
        let now = self.clocks[m.to as usize];
        let out = self.nodes[m.to as usize].handle(now, m.from, m.msg);
        self.absorb(m.to, out);
    }

    fn tick(&mut self, id: ValidatorId) {
        self.clocks[id as usize] += TICK_MS;
        let out = self.nodes[id as usize].tick(self.clocks[id as usize]);
        self.absorb(id, out);
    }

    fn equivocate(&mut self, subset: u8) {
        let Some(original) = self.byz_vertices.values().max_by_key(|v| v.round).cloned() else {
            return;
        };
        let mut twin = original;
        twin.batch.push(b"equivocation".to_vec());
        self.byz_vertices.insert(twin.digest(), twin.clone());
        for to in (0..BYZ).filter(|t| subset & (1 << t) != 0) {
            self.net.push(InFlight {
                to,
                from: BYZ,
                msg: Message::Propose {
                    vertex: twin.clone(),
                    signature: Vec::new(),
                },
            });
        }
    }

    fn apply(&mut self, a: Action) {
        let len = self.net.len();
        match a {
            Action::Deliver(i) => {
                let open: Vec<usize> = (0..len)
                    .filter(|&k| Some(self.net[k].to) != self.starved)
                    .collect();
                if !open.is_empty() {
                    self.deliver(open[i as usize % open.len()]);
                }
            }
            Action::Drop(i) if len > 0 => {
                self.net.remove(i as usize % len);
            }
            Action::Duplicate(i) if len > 0 => {
                let m = &self.net[i as usize % len];
                let copy = InFlight {
                    to: m.to,
                    from: m.from,
                    msg: m.msg.clone(),
                };
                self.net.push(copy);
            }
            Action::Tick(id) => self.tick(id % N),
            Action::Equivocate(s) => self.equivocate(s),
            Action::Starve(id) => {
                let id = u16::from(id) % (N + 1);
                self.starved = (id < N).then_some(id);
            }
            // Nothing in flight: nothing to deliver, drop or copy.
            Action::Drop(_) | Action::Duplicate(_) => {}
        }
    }

    fn honest() -> impl Iterator<Item = usize> {
        0..BYZ as usize
    }

    fn check_safety(&self) -> Result<(), TestCaseError> {
        for a in Self::honest() {
            let seq = &self.committed[a];
            let unique: BTreeSet<_> = seq.iter().collect();
            prop_assert_eq!(
                unique.len(),
                seq.len(),
                "node {} committed a certificate twice",
                a
            );
            for b in Self::honest().filter(|&b| b > a) {
                let other = &self.committed[b];
                let k = seq.len().min(other.len());
                prop_assert_eq!(&seq[..k], &other[..k], "nodes {} and {} disagree", a, b);
            }
        }
        let mut certified: BTreeMap<(u64, ValidatorId), Digest> = BTreeMap::new();
        for a in Self::honest() {
            let dag = self.nodes[a].dag();
            for round in 1..=dag.highest_round() {
                for author in 0..N {
                    if let Some(d) = dag.digest_at(round, author) {
                        let first = *certified.entry((round, author)).or_insert(d);
                        prop_assert_eq!(
                            first,
                            d,
                            "two certified vertices for ({}, {})",
                            round,
                            author
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// GST: every message reaches its honest destination in order, nothing is
    /// lost, and the Byzantine node has crashed.
    fn check_liveness(&mut self) -> Result<(), TestCaseError> {
        let before: Vec<usize> = self.committed.iter().map(Vec::len).collect();
        // Crashed: nothing more to or from validator 3. What it already got
        // certified is held by honest nodes and served on fetch.
        self.net.retain(|m| m.to != BYZ && m.from != BYZ);
        for step in 0..GST_STEPS {
            if Self::honest().all(|i| self.committed[i].len() >= before[i] + GST_PROGRESS) {
                return Ok(());
            }
            if self.net.is_empty() || step % 16 == 0 {
                for id in 0..BYZ {
                    self.tick(id);
                }
            } else {
                self.deliver(0);
            }
            self.net.retain(|m| m.to != BYZ);
        }
        let after: Vec<usize> = self.committed.iter().map(Vec::len).collect();
        let views: Vec<String> = Self::honest()
            .map(|i| {
                let n = &self.nodes[i];
                let top = n.dag().highest_round();
                let lo = top.saturating_sub(6);
                let counts: Vec<String> = (lo..=top)
                    .map(|r| {
                        let who: String = (0..N)
                            .map(|a| {
                                if n.dag().digest_at(r, a).is_some() {
                                    char::from_digit(u32::from(a), 10).unwrap_or('?')
                                } else {
                                    '.'
                                }
                            })
                            .collect();
                        format!("r{r}:{who}")
                    })
                    .collect();
                format!(
                    "node {i}: round {} committed-round {} [{}]",
                    n.round(),
                    n.last_committed_round(),
                    counts.join(" ")
                )
            })
            .collect();
        Err(TestCaseError::fail(format!(
            "no progress after GST: commits before {before:?}, after {after:?}, {} in flight
{}",
            self.net.len(),
            views.join(
                "
"
            )
        )))
    }
}

fn run(committee: Committee, actions: &[Action]) -> Result<(), TestCaseError> {
    let mut w = World::new(committee);
    for &a in actions {
        w.apply(a);
        w.check_safety()?;
    }
    w.check_liveness()?;
    w.check_safety()
}

fn cases() -> u32 {
    std::env::var("MAYA_SCHEDULE_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_CASES)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases()))]

    #[test]
    fn equal_weights_survive_any_schedule(actions in prop::collection::vec(action(), 0..MAX_ACTIONS)) {
        run(Committee::new(N), &actions)?;
    }

    /// The Byzantine seat is the heaviest one short of a blocking third.
    #[test]
    fn stake_weights_survive_any_schedule(actions in prop::collection::vec(action(), 0..MAX_ACTIONS)) {
        let committee = Committee::weighted(vec![300, 300, 300, 299]).expect("4 seats");
        run(committee, &actions)?;
    }
}

/// The adversary is not a straw man: on a schedule built for it, it gets an
/// equivocation in front of every honest node and harvests votes for it.
#[test]
fn the_adversary_reaches_honest_nodes_with_an_equivocation() {
    let mut w = World::new(Committee::new(N));
    for _ in 0..200 {
        if w.net.is_empty() {
            break;
        }
        w.deliver(0);
    }
    w.equivocate(0b111);
    for _ in 0..200 {
        if w.net.is_empty() {
            break;
        }
        w.deliver(0);
    }
    let twins = w.byz_vertices.len();
    assert!(
        twins >= 2,
        "validator 3 proposed {twins} vertices; the equivocation was never made"
    );
    w.check_safety()
        .expect("safety holds after a broadcast equivocation");
}

/// One certificate, voted by the first three seats: a quorum of four. The
/// signatures are empty because the default `Unauthenticated` engine accepts
/// any; the node's ML-DSA authenticator is tested in `auth_and_restart.rs`.
fn cert(round: u64, author: ValidatorId, parents: &[Digest]) -> Certificate {
    let mut parents = parents.to_vec();
    parents.sort_unstable();
    let vertex = Vertex {
        epoch: 0,
        round,
        author,
        timestamp_ms: 0,
        parents,
        batch: Vec::new(),
    };
    Certificate {
        vertex,
        votes: vec![0, 1, 2],
        signatures: vec![Vec::new(); 3],
    }
}

fn digests<'a>(certs: impl IntoIterator<Item = &'a Certificate>) -> Vec<Digest> {
    certs.into_iter().map(Certificate::digest).collect()
}

/// The schedule random search needs thousands of cases to find, built by hand:
/// two nodes see different children of the round-2 anchor A (leader 1).
/// Node Y has two links to A, the validity threshold, and commits it
/// directly. Node X has one and must commit it indirectly, through the
/// round-4 anchor B (leader 2). Disabling the indirect rule fails this test.
#[test]
fn an_anchor_committed_directly_by_one_node_is_committed_indirectly_by_another() {
    let committee = Committee::new(N);
    let genesis = Certificate::genesis(&committee);
    let r0: Vec<_> = genesis.iter().map(Certificate::digest).collect();
    let r1: Vec<_> = (0..N).map(|a| cert(1, a, &r0)).collect();
    let r1d = digests(&r1);
    let r2: Vec<_> = (0..N).map(|a| cert(2, a, &r1d)).collect();
    let [a0, anchor_a, a2, a3] = [0, 1, 2, 3].map(|i| r2[i].digest());
    let r3 = [
        cert(3, 0, &[a0, anchor_a, a2]),
        cert(3, 1, &[a0, anchor_a, a2]),
        cert(3, 2, &[a0, a2, a3]),
        cert(3, 3, &[a0, a2, a3]),
    ];
    let x_r3 = digests([&r3[0], &r3[2], &r3[3]]);
    let r4 = [
        cert(4, 0, &x_r3),
        cert(4, 1, &digests([&r3[0], &r3[1], &r3[2]])),
        cert(4, 2, &x_r3),
        cert(4, 3, &x_r3),
    ];
    let x_r4 = digests([&r4[0], &r4[2], &r4[3]]);
    let r5: Vec<_> = [0, 2, 3].map(|a| cert(5, a, &x_r4)).into();

    let insert_all = |dag: &mut Dag, certs: &[&Certificate]| {
        for c in certs {
            assert!(
                dag.insert((*c).clone()),
                "certificate ({}, {}) refused",
                c.vertex.round,
                c.vertex.author
            );
        }
    };
    let base: Vec<&Certificate> = r1.iter().chain(&r2).collect();

    let mut x = Dag::new(genesis.clone());
    insert_all(&mut x, &base);
    insert_all(
        &mut x,
        &[
            &r3[0], &r3[2], &r3[3], &r4[0], &r4[2], &r4[3], &r5[0], &r5[1], &r5[2],
        ],
    );
    let x_subs = Committer::new().try_commit_sub_dags(&x, &committee);
    let x_anchors: Vec<_> = x_subs.iter().map(|s| s.anchor.digest()).collect();
    assert_eq!(
        x_anchors,
        vec![anchor_a, r4[2].digest()],
        "X must reach A through B"
    );

    let mut y = Dag::new(genesis);
    let mut y_commit = Committer::new();
    insert_all(&mut y, &base);
    insert_all(&mut y, &[&r3[0], &r3[1], &r3[2]]);
    let mut y_seq = y_commit.try_commit(&y, &committee);
    assert_eq!(
        y_seq.last().map(Certificate::digest),
        Some(anchor_a),
        "Y commits A directly"
    );
    insert_all(
        &mut y,
        &[
            &r3[3], &r4[0], &r4[1], &r4[2], &r4[3], &r5[0], &r5[1], &r5[2],
        ],
    );
    y_seq.extend(y_commit.try_commit(&y, &committee));

    let x_seq: Vec<Digest> = x_subs
        .iter()
        .flat_map(|s| s.certificates.iter().map(Certificate::digest))
        .collect();
    let y_seq: Vec<Digest> = y_seq.iter().map(Certificate::digest).collect();
    assert_eq!(
        x_seq, y_seq,
        "the two views ordered the same history differently"
    );
}

/// Why the direct rule needs f + 1 links and not one: a single link to the
/// round-2 anchor A can be missing from every parent set of the round-4
/// anchor B. A node committing A on that one link would order it before B;
/// a node that never saw the link orders B with no path to A. The engine
/// must commit A on neither view, so both orders agree.
#[test]
fn one_link_to_an_anchor_is_not_enough_to_commit_it() {
    let committee = Committee::new(N);
    let genesis = Certificate::genesis(&committee);
    let r0: Vec<_> = genesis.iter().map(Certificate::digest).collect();
    let r1: Vec<_> = (0..N).map(|a| cert(1, a, &r0)).collect();
    let r1d = digests(&r1);
    let r2: Vec<_> = (0..N).map(|a| cert(2, a, &r1d)).collect();
    let [a0, anchor_a, a2, a3] = [0, 1, 2, 3].map(|i| r2[i].digest());
    let r3 = [
        cert(3, 0, &[a0, anchor_a, a2]),
        cert(3, 1, &[a0, a2, a3]),
        cert(3, 2, &[a0, a2, a3]),
        cert(3, 3, &[a0, a2, a3]),
    ];
    let y_r3 = digests([&r3[1], &r3[2], &r3[3]]);
    let r4: Vec<_> = (0..N).map(|a| cert(4, a, &y_r3)).collect();
    let y_r4 = digests([&r4[0], &r4[2], &r4[3]]);
    let r5: Vec<_> = [0, 2, 3].map(|a| cert(5, a, &y_r4)).into();
    let base: Vec<&Certificate> = r1.iter().chain(&r2).collect();
    let tail: Vec<&Certificate> = r4.iter().chain(&r5).collect();

    let mut x = Dag::new(genesis.clone());
    let mut x_commit = Committer::new();
    for c in base.iter().chain([&&r3[0], &&r3[1], &&r3[2]]) {
        assert!(x.insert((*c).clone()));
    }
    let mut x_seq = x_commit.try_commit(&x, &committee);
    assert!(x_seq.is_empty(), "A was committed on one link");
    for c in [&r3[3]].into_iter().chain(tail.iter().copied()) {
        assert!(x.insert(c.clone()));
    }
    x_seq.extend(x_commit.try_commit(&x, &committee));

    let mut y = Dag::new(genesis);
    for c in base.iter().copied().chain(&r3).chain(tail.iter().copied()) {
        assert!(y.insert(c.clone()));
    }
    let y_seq = Committer::new().try_commit(&y, &committee);
    assert!(!y_seq.is_empty(), "B was not committed");
    let order = |s: &[Certificate]| s.iter().map(Certificate::digest).collect::<Vec<_>>();
    assert_eq!(
        order(&x_seq),
        order(&y_seq),
        "the two views ordered history differently"
    );
}

/// Found by the deep search (2026-10-28): a certificate buffered for a missing
/// parent asked for that parent once. If the fetch or its reply was lost, a
/// re-delivery of the buffered certificate returned early and asked again
/// for nothing, so the node never voted again and, with one seat down, the
/// network stalled. Every re-delivery must re-request what it waits on.
#[test]
fn a_lost_fetch_is_asked_for_again() {
    let committee = Committee::new(N);
    let genesis = Certificate::genesis(&committee);
    let r0: Vec<_> = genesis.iter().map(Certificate::digest).collect();
    let r1: Vec<_> = (0..N).map(|a| cert(1, a, &r0)).collect();
    let r1d = digests(&r1);
    let r2: Vec<_> = (0..N).map(|a| cert(2, a, &r1d)).collect();
    let r2d = digests(&r2);
    let r3: Vec<_> = (0..N).map(|a| cert(3, a, &r2d)).collect();
    let missing = r3[3].digest();
    let late = cert(4, 3, &digests([&r3[0], &r3[1], &r3[3]]));

    let mut node = Validator::new(2, committee, Params::default());
    for c in r1.iter().chain(&r2).chain(&r3[..3]) {
        node.handle(0, 0, Message::Cert(c.clone()));
    }
    let asks = |out: &Output| out.sends.iter().any(|(_, m)| *m == Message::Fetch(missing));
    assert!(
        asks(&node.handle(0, 0, Message::Cert(late.clone()))),
        "first delivery fetches the parent"
    );
    // That fetch is lost. The certificate arrives again from another peer.
    assert!(
        asks(&node.handle(0, 1, Message::Cert(late))),
        "a re-delivery must fetch it again"
    );
}

/// The transitive case: a certificate waits on a buffered parent, which
/// waits on one nobody has sent. Re-delivering the child must reach through
/// the buffered parent and fetch the grandparent.
#[test]
fn a_lost_fetch_behind_a_buffered_parent_is_asked_for_again() {
    let committee = Committee::new(N);
    let genesis = Certificate::genesis(&committee);
    let r0 = digests(&genesis);
    let r1: Vec<_> = (0..N).map(|a| cert(1, a, &r0)).collect();
    let r2: Vec<_> = (0..N).map(|a| cert(2, a, &digests(&r1))).collect();
    let r3: Vec<_> = (0..N).map(|a| cert(3, a, &digests(&r2))).collect();
    let grandparent = r2[3].digest();
    let parent = cert(3, 3, &digests([&r2[0], &r2[1], &r2[3]]));
    let child = cert(4, 3, &digests([&r3[0], &r3[1], &parent]));

    let mut node = Validator::new(2, committee, Params::default());
    for c in r1.iter().chain(&r2[..3]).chain(&r3[..3]) {
        node.handle(0, 0, Message::Cert(c.clone()));
    }
    node.handle(0, 0, Message::Cert(parent));
    node.handle(0, 0, Message::Cert(child.clone()));
    // Every fetch so far is lost. The child arrives again.
    let out = node.handle(0, 1, Message::Cert(child));
    let fetches: Vec<_> = out
        .sends
        .iter()
        .filter(|(_, m)| matches!(m, Message::Fetch(_)))
        .collect();
    assert_eq!(
        fetches.len(),
        1,
        "one fetch, for the grandparent alone: {fetches:?}"
    );
    assert_eq!(fetches[0].1, Message::Fetch(grandparent));
}

/// The retry is not an amplifier: a peer replaying one buffered certificate
/// gets one round of fetches per `resend_interval_ms`, not one per replay.
#[test]
fn replaying_a_buffered_certificate_is_throttled() {
    let committee = Committee::new(N);
    let genesis = Certificate::genesis(&committee);
    let r1: Vec<_> = (0..N).map(|a| cert(1, a, &digests(&genesis))).collect();
    let r2: Vec<_> = (0..N).map(|a| cert(2, a, &digests(&r1))).collect();
    let r3: Vec<_> = (0..N).map(|a| cert(3, a, &digests(&r2))).collect();
    let late = cert(4, 3, &digests([&r3[0], &r3[1], &r3[3]]));
    let params = Params {
        resend_interval_ms: 500,
        ..Params::default()
    };

    let mut node = Validator::new(2, committee, params);
    for c in r1.iter().chain(&r2).chain(&r3[..3]) {
        node.handle(1_000, 0, Message::Cert(c.clone()));
    }
    let fetches = |out: Output| {
        out.sends
            .iter()
            .filter(|(_, m)| matches!(m, Message::Fetch(_)))
            .count()
    };
    assert_eq!(
        fetches(node.handle(1_000, 0, Message::Cert(late.clone()))),
        1,
        "first delivery"
    );
    assert_eq!(
        fetches(node.handle(1_000, 1, Message::Cert(late.clone()))),
        1,
        "first retry"
    );
    for _ in 0..100 {
        assert_eq!(
            fetches(node.handle(1_200, 1, Message::Cert(late.clone()))),
            0,
            "replay inside the interval"
        );
    }
    assert_eq!(
        fetches(node.handle(1_500, 1, Message::Cert(late))),
        1,
        "retry once the interval passes"
    );
}

/// What a node may persist (`Output::inserted`): a certificate waiting for a
/// parent is not inserted, and is reported in the step that finally inserts
/// it, alongside the parent that unblocked it. The node's certificate log
/// writes exactly these, so an unverified frame never reaches the disk.
#[test]
fn only_inserted_certificates_are_reported_for_persistence() {
    let committee = Committee::new(N);
    let genesis = Certificate::genesis(&committee);
    let r1: Vec<_> = (0..N).map(|a| cert(1, a, &digests(&genesis))).collect();
    let r2: Vec<_> = (0..N).map(|a| cert(2, a, &digests(&r1))).collect();
    let child = cert(3, 0, &digests([&r2[0], &r2[1], &r2[3]]));

    let mut node = Validator::new(2, committee, Params::default());
    for c in r1.iter().chain(&r2[..3]) {
        let out = node.handle(0, 0, Message::Cert(c.clone()));
        assert_eq!(
            out.inserted,
            vec![c.digest()],
            "a held parent inserts at once"
        );
    }
    let waiting = node.handle(0, 0, Message::Cert(child.clone()));
    assert!(
        waiting.inserted.is_empty(),
        "buffered, not inserted: {:?}",
        waiting.inserted
    );
    assert!(
        node.handle(0, 1, Message::Cert(child.clone()))
            .inserted
            .is_empty(),
        "re-delivery while waiting"
    );
    let unblocked = node.handle(0, 0, Message::Cert(r2[3].clone()));
    assert_eq!(
        unblocked.inserted,
        vec![r2[3].digest(), child.digest()],
        "parent, then the child it unblocked"
    );
    assert!(
        node.handle(0, 0, Message::Cert(child)).inserted.is_empty(),
        "a held certificate is not reported twice"
    );
}
