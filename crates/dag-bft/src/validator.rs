//! One validator as a sans-IO state machine: messages in, messages out.
//!
//! The protocol is Narwhal's certified DAG: propose a vertex, collect 2f + 1
//! votes, broadcast the certificate, advance once 2f + 1 certificates of the
//! round are held. Lost messages are recovered by re-broadcast on
//! [`Validator::tick`] and by fetching missing parents from whoever sent the
//! child, so the engine is live over a lossy, reordering network.
//!
//! Time is whatever the caller passes in milliseconds; the engine never
//! reads a clock, so a simulator can drive it deterministically.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::commit::Committer;
use crate::dag::Dag;
use crate::vertex::{Certificate, Committee, Digest, ValidatorId, Vertex};

/// Rounds kept behind the last committed anchor before garbage collection.
pub const GC_DEPTH: u64 = 50;

/// What validators say to each other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// An uncertified vertex, asking for votes.
    Propose(Vertex),
    /// A vote for `digest`, sent to the vertex's author.
    Vote {
        /// Vertex voted for.
        digest: Digest,
        /// Its round.
        round: u64,
        /// The voter.
        voter: ValidatorId,
    },
    /// A certified vertex.
    Cert(Certificate),
    /// A request for a certificate the sender is missing.
    Fetch(Digest),
}

/// Where a message goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    /// Every other validator.
    All,
    /// One validator.
    To(ValidatorId),
}

/// What one step produced.
#[derive(Clone, Debug, Default)]
pub struct Output {
    /// Messages to deliver.
    pub sends: Vec<(Dest, Message)>,
    /// Certificates newly ordered, in commit order.
    pub committed: Vec<Certificate>,
}

/// Engine parameters.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Most transactions per vertex.
    pub batch_size: usize,
    /// How long to wait for an anchor before advancing without it.
    pub anchor_timeout_ms: u64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            batch_size: 500,
            anchor_timeout_ms: 1_000,
        }
    }
}

/// A validator.
#[derive(Debug)]
pub struct Validator {
    id: ValidatorId,
    committee: Committee,
    params: Params,
    dag: Dag,
    committer: Committer,
    /// Round of the vertex most recently proposed.
    round: u64,
    /// Own vertex awaiting votes, and the votes so far.
    pending: Option<(Vertex, BTreeSet<ValidatorId>)>,
    /// One vote per (round, author): the rule that makes equivocation
    /// uncertifiable.
    voted: BTreeMap<(u64, ValidatorId), Digest>,
    /// Certificates waiting for parents.
    buffer: BTreeMap<Digest, Certificate>,
    mempool: VecDeque<u64>,
    /// When the node first had a quorum for `round` without the anchor.
    waiting_since: Option<(u64, u64)>,
}

impl Validator {
    /// Validator `id` with the shared genesis.
    pub fn new(id: ValidatorId, committee: Committee, params: Params) -> Self {
        Self {
            id,
            committee,
            params,
            dag: Dag::new(Certificate::genesis(committee)),
            committer: Committer::new(),
            round: 0,
            pending: None,
            voted: BTreeMap::new(),
            buffer: BTreeMap::new(),
            mempool: VecDeque::new(),
            waiting_since: None,
        }
    }

    /// Queues a transaction for a future vertex.
    pub fn submit(&mut self, tx: u64) {
        self.mempool.push_back(tx);
    }

    /// Transactions not yet proposed.
    pub fn mempool_len(&self) -> usize {
        self.mempool.len()
    }

    /// The local DAG.
    pub fn dag(&self) -> &Dag {
        &self.dag
    }

    /// Committed anchors, in order.
    pub fn anchors(&self) -> &[Digest] {
        self.committer.anchors()
    }

    /// Round most recently proposed.
    pub fn round(&self) -> u64 {
        self.round
    }

    /// Proposes round 1.
    pub fn start(&mut self, now_ms: u64) -> Output {
        let mut out = Output::default();
        self.try_advance(now_ms, &mut out);
        out
    }

    /// Handles one message from `from`.
    pub fn handle(&mut self, now_ms: u64, from: ValidatorId, msg: Message) -> Output {
        let mut out = Output::default();
        match msg {
            Message::Propose(v) => self.on_propose(from, &v, &mut out),
            Message::Vote { digest, round, voter } => self.on_vote(digest, round, voter, &mut out),
            Message::Cert(c) => self.on_cert(from, c, &mut out),
            Message::Fetch(d) => {
                if let Some(c) = self.dag.by_digest(&d).or_else(|| self.buffer.get(&d)) {
                    out.sends.push((Dest::To(from), Message::Cert(c.clone())));
                }
            }
        }
        self.progress(now_ms, &mut out);
        out
    }

    /// Periodic work: re-broadcast what may have been lost, and advance past
    /// an anchor that timed out.
    pub fn tick(&mut self, now_ms: u64) -> Output {
        let mut out = Output::default();
        if let Some((v, _)) = &self.pending {
            out.sends.push((Dest::All, Message::Propose(v.clone())));
        } else if let Some(c) = self.dag.get(self.round, self.id) {
            out.sends.push((Dest::All, Message::Cert(c.clone())));
        }
        self.progress(now_ms, &mut out);
        out
    }

    fn on_propose(&mut self, from: ValidatorId, v: &Vertex, out: &mut Output) {
        if v.author != from || v.round == 0 || v.author >= self.committee.size() {
            return;
        }
        let digest = v.digest();
        let probe = Certificate { vertex: v.clone(), votes: Vec::new() };
        let missing = self.dag.missing_parents(&probe);
        if !missing.is_empty() {
            for d in missing {
                out.sends.push((Dest::To(from), Message::Fetch(d)));
            }
            return; // the author re-proposes on its next tick
        }
        let quorum_parents = v.parents.len() >= usize::from(self.committee.quorum())
            && v.parents.windows(2).all(|w| w[0] < w[1])
            && v.parents.iter().all(|p| {
                self.dag
                    .by_digest(p)
                    .is_none_or(|c| c.vertex.round + 1 == v.round)
            });
        if !quorum_parents {
            return;
        }
        let slot = self.voted.entry((v.round, v.author)).or_insert(digest);
        if *slot == digest {
            out.sends.push((
                Dest::To(from),
                Message::Vote { digest, round: v.round, voter: self.id },
            ));
        }
    }

    fn on_vote(&mut self, digest: Digest, round: u64, voter: ValidatorId, out: &mut Output) {
        let Some((vertex, votes)) = &mut self.pending else {
            return;
        };
        if vertex.round != round || vertex.digest() != digest || voter >= self.committee.size() {
            return;
        }
        votes.insert(voter);
        if votes.len() >= usize::from(self.committee.quorum()) {
            let cert = Certificate {
                vertex: vertex.clone(),
                votes: votes.iter().copied().collect(),
            };
            self.pending = None;
            out.sends.push((Dest::All, Message::Cert(cert.clone())));
            self.accept(cert, self.id, out);
        }
    }

    fn on_cert(&mut self, from: ValidatorId, c: Certificate, out: &mut Output) {
        if c.is_well_formed(self.committee) {
            self.accept(c, from, out);
        }
    }

    fn accept(&mut self, c: Certificate, from: ValidatorId, out: &mut Output) {
        let digest = c.digest();
        if self.dag.contains(&digest) || c.vertex.round < self.dag.gc_round() {
            return;
        }
        let missing = self.dag.missing_parents(&c);
        if !missing.is_empty() {
            for d in missing.into_iter().filter(|d| !self.buffer.contains_key(d)) {
                out.sends.push((Dest::To(from), Message::Fetch(d)));
            }
            self.buffer.insert(digest, c);
            return;
        }
        self.dag.insert(c);
        self.drain_buffer();
    }

    /// Inserts buffered certificates whose parents have arrived.
    fn drain_buffer(&mut self) {
        loop {
            let ready: Vec<Digest> = self
                .buffer
                .iter()
                .filter(|(_, c)| self.dag.missing_parents(c).is_empty())
                .map(|(d, _)| *d)
                .collect();
            if ready.is_empty() {
                return;
            }
            for d in ready {
                if let Some(c) = self.buffer.remove(&d) {
                    self.dag.insert(c);
                }
            }
        }
    }

    fn progress(&mut self, now_ms: u64, out: &mut Output) {
        let committed = self.committer.try_commit(&self.dag, self.committee);
        if !committed.is_empty() {
            let horizon = self.committer.last_committed_round().saturating_sub(GC_DEPTH);
            self.dag.collect_below(horizon);
            self.committer.collect_below(horizon);
            self.voted = self.voted.split_off(&(horizon, 0));
            self.buffer.retain(|_, c| c.vertex.round >= horizon);
        }
        out.committed.extend(committed);
        self.try_advance(now_ms, out);
    }

    /// Proposes the next round once this round has a quorum (and, in an
    /// anchor round, the anchor or a timeout).
    fn try_advance(&mut self, now_ms: u64, out: &mut Output) {
        loop {
            let round = self.round;
            // Our own vertex need not be among the quorum: a slow certificate
            // must not stall the node, which is what lets it tolerate loss.
            if self.dag.round_len(round) < usize::from(self.committee.quorum()) {
                return;
            }
            if let Some(leader) = self.committee.leader(round)
                && round > 0
                && self.dag.get(round, leader).is_none()
            {
                match self.waiting_since {
                    Some((r, since)) if r == round => {
                        if now_ms.saturating_sub(since) < self.params.anchor_timeout_ms {
                            return;
                        }
                    }
                    _ => {
                        self.waiting_since = Some((round, now_ms));
                        return;
                    }
                }
            }
            self.propose(round + 1, out);
        }
    }

    fn propose(&mut self, round: u64, out: &mut Output) {
        let mut parents: Vec<Digest> = self.dag.round(round - 1).map(Certificate::digest).collect();
        parents.sort_unstable();
        // A vertex that never gathered a quorum is abandoned; its
        // transactions go back to the front of the queue, not into the void.
        if let Some((abandoned, _)) = self.pending.take() {
            for tx in abandoned.batch.into_iter().rev() {
                self.mempool.push_front(tx);
            }
        }
        let take = self.params.batch_size.min(self.mempool.len());
        let batch: Vec<u64> = self.mempool.drain(..take).collect();
        let vertex = Vertex { round, author: self.id, parents, batch };
        let digest = vertex.digest();
        self.voted.insert((round, self.id), digest);
        self.round = round;
        self.waiting_since = None;
        // Our own vote counts; with a committee of one that is a quorum.
        let mut votes = BTreeSet::new();
        votes.insert(self.id);
        self.pending = Some((vertex.clone(), votes));
        out.sends.push((Dest::All, Message::Propose(vertex)));
        if self.committee.quorum() <= 1 {
            self.on_vote(digest, round, self.id, out);
        }
    }
}
