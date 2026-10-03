//! One validator as a sans-IO state machine: messages in, messages out.
//!
//! The protocol is Narwhal's certified DAG: propose a vertex, collect a quorum of
//! votes, broadcast the certificate, advance once a quorum of certificates of the
//! round are held. Lost messages are recovered by re-broadcast on
//! [`Validator::tick`] and by fetching missing parents from whoever sent the
//! child, so the engine is live over a lossy, reordering network.
//!
//! Every proposal, vote and certificate is checked through the
//! [`Authenticator`], so the transport's idea of who sent a message is never
//! trusted for anything but where to send a `Fetch` reply.
//!
//! Time is whatever the caller passes in milliseconds; the engine never
//! reads a clock, so a simulator can drive it deterministically.

use std::collections::{BTreeMap, VecDeque};

use crate::auth::{Authenticator, Equivocation, SignContext, SignKind, Unauthenticated};
use crate::commit::{Committer, SubDag};
use crate::dag::Dag;
use crate::vertex::{Certificate, Committee, Digest, Payload, ValidatorId, Vertex};

/// Rounds kept behind the last committed anchor before garbage collection.
pub const GC_DEPTH: u64 = 50;

/// What validators say to each other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// An uncertified vertex, asking for votes, signed by its author.
    Propose {
        /// The vertex.
        vertex: Vertex,
        /// Its author's signature over the vertex digest.
        signature: Vec<u8>,
    },
    /// A vote for `digest`, sent to the vertex's author.
    Vote {
        /// Vertex voted for.
        digest: Digest,
        /// Its round.
        round: u64,
        /// The voter.
        voter: ValidatorId,
        /// The voter's signature over `digest`.
        signature: Vec<u8>,
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
    /// Certificates newly ordered, in commit order: `sub_dags` flattened.
    pub committed: Vec<Certificate>,
    /// The same certificates grouped by the anchor that ordered them.
    pub sub_dags: Vec<SubDag>,
    /// Conflicting signed proposals seen, for the staking module.
    pub equivocations: Vec<Equivocation>,
}

/// Engine parameters.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Most transactions per vertex.
    pub batch_size: usize,
    /// Most payload bytes per vertex. A vertex over it is not voted for, so
    /// one author cannot make every validator hold an unbounded proposal.
    pub max_batch_bytes: usize,
    /// How long to wait for an anchor before advancing without it.
    pub anchor_timeout_ms: u64,
    /// The committee epoch this engine instance runs; bound into every digest.
    pub epoch: u64,
    /// Least time between two of this validator's proposals, unless a full
    /// batch is waiting. Zero (the simulator's value) advances as fast as
    /// certificates arrive, which on an idle network is a stream of empty
    /// vertices; a node paces itself so an idle chain costs little.
    pub min_round_interval_ms: u64,
    /// Least time between two re-broadcasts of this validator's pending
    /// proposal or latest certificate. A re-broadcast only recovers a lost
    /// message; sending a full vertex — megabytes when its batch is full — on
    /// every tick made each peer decode and discard it every tick. Zero (the
    /// simulator's value) re-broadcasts on every tick, as before.
    pub resend_interval_ms: u64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            batch_size: 500,
            max_batch_bytes: 4 * 1024 * 1024,
            anchor_timeout_ms: 1_000,
            epoch: 0,
            min_round_interval_ms: 0,
            resend_interval_ms: 0,
        }
    }
}

/// A validator. Without a type argument it is the simulator's
/// [`Unauthenticated`] validator.
#[derive(Debug)]
pub struct Validator<A = Unauthenticated> {
    id: ValidatorId,
    committee: Committee,
    params: Params,
    auth: A,
    /// Whether this instance proposes and votes. An observer (a full node
    /// without a validator key) runs the same DAG and commit rule on
    /// certificates alone and derives the same blocks.
    voting: bool,
    dag: Dag,
    committer: Committer,
    /// Round of the vertex most recently proposed.
    round: u64,
    /// Own vertex awaiting votes, and the signed votes so far.
    pending: Option<(Vertex, BTreeMap<ValidatorId, Vec<u8>>)>,
    /// `pending`'s digest, kept beside it: a vertex digest hashes the whole
    /// payload, and every vote and certificate would otherwise rehash it.
    pending_digest: Option<Digest>,
    /// One vote per (round, author): the rule that makes equivocation
    /// uncertifiable.
    voted: BTreeMap<(u64, ValidatorId), Digest>,
    /// The first signed proposal seen per (round, author), kept to prove an
    /// equivocation when a second one arrives.
    seen: BTreeMap<(u64, ValidatorId), (Vertex, Vec<u8>, Digest)>,
    /// Certificates waiting for parents.
    buffer: BTreeMap<Digest, Certificate>,
    mempool: VecDeque<Payload>,
    /// When the node first had a quorum for `round` without the anchor.
    waiting_since: Option<(u64, u64)>,
    /// When this validator last proposed, for `min_round_interval_ms`.
    last_proposed_ms: u64,
    /// When `tick` last re-broadcast, for `resend_interval_ms`.
    last_resend_ms: u64,
}

impl Validator<Unauthenticated> {
    /// SIM: validator `id` with the shared genesis and no signatures.
    pub fn new(id: ValidatorId, committee: Committee, params: Params) -> Self {
        Self::with_auth(id, committee, params, Unauthenticated)
    }
}

impl<A: Authenticator> Validator<A> {
    /// Validator `id`, signing and verifying through `auth`.
    pub fn with_auth(id: ValidatorId, committee: Committee, params: Params, auth: A) -> Self {
        let genesis = Certificate::genesis_in(params.epoch, &committee);
        Self {
            id,
            committee,
            params,
            auth,
            voting: true,
            dag: Dag::new(genesis),
            committer: Committer::new(),
            round: 0,
            pending: None,
            pending_digest: None,
            voted: BTreeMap::new(),
            seen: BTreeMap::new(),
            buffer: BTreeMap::new(),
            mempool: VecDeque::new(),
            waiting_since: None,
            last_proposed_ms: 0,
            last_resend_ms: 0,
        }
    }

    /// A non-voting observer: follows certificates, commits, never signs.
    pub fn observer(committee: Committee, params: Params, auth: A) -> Self {
        let mut v = Self::with_auth(ValidatorId::MAX, committee, params, auth);
        v.voting = false;
        v
    }

    /// Queues a transaction for a future vertex.
    pub fn submit(&mut self, tx: impl Into<Payload>) {
        self.mempool.push_back(tx.into());
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

    /// Round of the last committed anchor.
    pub fn last_committed_round(&self) -> u64 {
        self.committer.last_committed_round()
    }

    /// The committee epoch.
    pub fn epoch(&self) -> u64 {
        self.params.epoch
    }

    /// Restores a vote this validator cast before a restart, so it cannot
    /// vote for a different vertex in the same slot afterwards.
    pub fn restore_vote(&mut self, round: u64, author: ValidatorId, digest: Digest) {
        self.voted.entry((round, author)).or_insert(digest);
    }

    /// Restores one of this validator's own signed proposals from before a
    /// restart. The newest becomes pending again and sets the round, so the
    /// validator re-broadcasts it rather than signing a second vertex for a
    /// round it already proposed in — which would be an equivocation.
    ///
    /// Call for every recorded proposal, oldest first, *before* replaying
    /// certificates, and before [`Validator::start`].
    pub fn restore_proposal(&mut self, vertex: Vertex, signature: Vec<u8>) {
        if vertex.author != self.id || vertex.epoch != self.params.epoch {
            return;
        }
        let digest = vertex.digest();
        self.voted.insert((vertex.round, self.id), digest);
        self.seen.insert(
            (vertex.round, self.id),
            (vertex.clone(), signature.clone(), digest),
        );
        if vertex.round >= self.round {
            self.round = vertex.round;
            self.last_proposed_ms = vertex.timestamp_ms;
            let mut votes = BTreeMap::new();
            votes.insert(self.id, signature);
            self.pending = Some((vertex, votes));
            self.pending_digest = Some(digest);
        }
    }

    /// Proposes round 1.
    pub fn start(&mut self, now_ms: u64) -> Output {
        let mut out = Output::default();
        if self.voting {
            self.try_advance(now_ms, &mut out);
        }
        out
    }

    /// Handles one message from `from`.
    pub fn handle(&mut self, now_ms: u64, from: ValidatorId, msg: Message) -> Output {
        let mut out = Output::default();
        match msg {
            Message::Propose { vertex, signature } => {
                self.on_propose(from, vertex, &signature, &mut out);
            }
            Message::Vote {
                digest,
                round,
                voter,
                signature,
            } => self.on_vote(digest, round, voter, signature, &mut out),
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
        let due = now_ms.saturating_sub(self.last_resend_ms) >= self.params.resend_interval_ms;
        if due {
            self.last_resend_ms = now_ms;
            if let Some((v, votes)) = &self.pending {
                let signature = votes.get(&self.id).cloned().unwrap_or_default();
                out.sends.push((
                    Dest::All,
                    Message::Propose {
                        vertex: v.clone(),
                        signature,
                    },
                ));
            } else if let Some(c) = self.dag.get(self.round, self.id) {
                out.sends.push((Dest::All, Message::Cert(c.clone())));
            }
        }
        self.progress(now_ms, &mut out);
        out
    }

    fn on_propose(&mut self, from: ValidatorId, v: Vertex, signature: &[u8], out: &mut Output) {
        if !self.voting
            || v.epoch != self.params.epoch
            || v.round == 0
            || v.author >= self.committee.size()
            || v.batch.len() > self.params.batch_size
            || v.payload_bytes() > self.params.max_batch_bytes
        {
            return;
        }
        let digest = v.digest();
        if !self.auth.verify(v.author, &digest, signature) {
            return;
        }
        if !self.record_proposal(&v, signature, digest, out) {
            return;
        }
        let probe = Certificate {
            vertex: v,
            votes: Vec::new(),
            signatures: Vec::new(),
        };
        let missing = self.dag.missing_parents(&probe);
        if !missing.is_empty() {
            for d in missing {
                out.sends.push((Dest::To(from), Message::Fetch(d)));
            }
            return; // the author re-proposes on its next tick
        }
        let v = probe.vertex;
        let quorum_parents = self.has_parent_quorum(&v)
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
                Dest::To(v.author),
                Message::Vote {
                    digest,
                    round: v.round,
                    voter: self.id,
                    signature: self.auth.sign(
                        SignContext {
                            kind: SignKind::Vote,
                            round: v.round,
                            author: v.author,
                        },
                        &digest,
                    ),
                },
            ));
        }
    }

    /// Remembers the first signed proposal per slot; reports a second,
    /// different one as an equivocation. Returns whether `v` may be voted for.
    fn record_proposal(
        &mut self,
        v: &Vertex,
        signature: &[u8],
        digest: Digest,
        out: &mut Output,
    ) -> bool {
        let slot = (v.round, v.author);
        match self.seen.get(&slot) {
            None => {
                self.seen
                    .insert(slot, (v.clone(), signature.to_vec(), digest));
                true
            }
            Some((_, _, first_digest)) if *first_digest == digest => true,
            Some((first, first_signature, _)) => {
                out.equivocations.push(Equivocation {
                    first: first.clone(),
                    first_signature: first_signature.clone(),
                    second: v.clone(),
                    second_signature: signature.to_vec(),
                });
                false
            }
        }
    }

    fn on_vote(
        &mut self,
        digest: Digest,
        round: u64,
        voter: ValidatorId,
        signature: Vec<u8>,
        out: &mut Output,
    ) {
        let Some((vertex, votes)) = &mut self.pending else {
            return;
        };
        if vertex.round != round
            || self.pending_digest != Some(digest)
            || voter >= self.committee.size()
            || votes.contains_key(&voter)
            || !self.auth.verify(voter, &digest, &signature)
        {
            return;
        }
        votes.insert(voter, signature);
        if self.committee.is_quorum(votes.keys().copied()) {
            let (voters, signatures) = votes.iter().map(|(v, s)| (*v, s.clone())).unzip();
            let cert = Certificate {
                vertex: vertex.clone(),
                votes: voters,
                signatures,
            };
            self.pending = None;
            out.sends.push((Dest::All, Message::Cert(cert.clone())));
            self.accept(cert, digest, self.id, out);
        }
    }

    fn on_cert(&mut self, from: ValidatorId, c: Certificate, out: &mut Output) {
        if c.vertex.epoch != self.params.epoch || !c.is_well_formed(&self.committee) {
            return;
        }
        // A held slot answers before the digest does: certification leaves one
        // certificate per slot, and the DAG refuses a second anyway, so a
        // re-broadcast need not be hashed — which for a full vertex is
        // megabytes — to be recognised.
        if self.dag.get(c.vertex.round, c.vertex.author).is_some() {
            return;
        }
        let digest = c.digest();
        // Before the signatures: a re-broadcast of a held certificate costs a
        // lookup, not a quorum of verifications.
        if self.dag.contains(&digest) || self.buffer.contains_key(&digest) {
            return;
        }
        let authentic = c
            .votes
            .iter()
            .zip(&c.signatures)
            .all(|(voter, sig)| self.auth.verify(*voter, &digest, sig));
        if authentic {
            self.accept(c, digest, from, out);
        }
    }

    fn accept(&mut self, c: Certificate, digest: Digest, from: ValidatorId, out: &mut Output) {
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
        if !self.has_parent_quorum(&c.vertex) {
            return;
        }
        // Our own vertex certified by someone else's broadcast (or replayed
        // from disk after a restart): nothing is pending any more.
        if self.pending.is_some() && self.pending_digest == Some(digest) {
            self.pending = None;
        }
        self.dag.insert_digested(c, digest);
        self.drain_buffer();
    }

    /// Whether `v`'s parents, all held, were authored by a quorum of weight
    /// in the round just before `v`'s. Genesis has none and needs none. At
    /// or below the collection horizon the parents are gone and cannot be
    /// weighed — the exemption `Dag::missing_parents` makes — so only
    /// `is_well_formed`'s head-count floor applies there. Elsewhere a parent
    /// not held, or from another round, counts for nothing.
    fn has_parent_quorum(&self, v: &Vertex) -> bool {
        v.round == 0
            || v.round <= self.dag.gc_round()
            || self.committee.is_quorum(v.parents.iter().filter_map(|p| {
                self.dag
                    .by_digest(p)
                    .filter(|c| c.vertex.round + 1 == v.round)
                    .map(|c| c.vertex.author)
            }))
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
                if let Some(c) = self.buffer.remove(&d)
                    && self.has_parent_quorum(&c.vertex)
                {
                    self.dag.insert_digested(c, d);
                }
            }
        }
    }

    fn progress(&mut self, now_ms: u64, out: &mut Output) {
        let sub_dags = self
            .committer
            .try_commit_sub_dags(&self.dag, &self.committee);
        if !sub_dags.is_empty() {
            let horizon = self
                .committer
                .last_committed_round()
                .saturating_sub(GC_DEPTH);
            self.dag.collect_below(horizon);
            self.committer.collect_below(horizon);
            self.voted = self.voted.split_off(&(horizon, 0));
            self.seen = self.seen.split_off(&(horizon, 0));
            self.buffer.retain(|_, c| c.vertex.round >= horizon);
        }
        out.committed
            .extend(sub_dags.iter().flat_map(|s| s.certificates.iter().cloned()));
        out.sub_dags.extend(sub_dags);
        if self.voting {
            self.try_advance(now_ms, out);
        }
    }

    /// Proposes the next round once this round has a quorum (and, in an
    /// anchor round, the anchor or a timeout).
    fn try_advance(&mut self, now_ms: u64, out: &mut Output) {
        loop {
            let round = self.round;
            // Our own vertex need not be among the quorum: a slow certificate
            // must not stall the node, which is what lets it tolerate loss.
            if !self
                .committee
                .is_quorum(self.dag.round(round).map(|c| c.vertex.author))
            {
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
            let full = self.mempool.len() >= self.params.batch_size;
            let early =
                now_ms.saturating_sub(self.last_proposed_ms) < self.params.min_round_interval_ms;
            if round > 0 && early && !full {
                return; // the next tick retries
            }
            self.propose(round + 1, now_ms, out);
        }
    }

    /// Takes the next batch off the queue, within both the count and the byte
    /// bound.
    fn next_batch(&mut self) -> Vec<Payload> {
        let mut batch = Vec::new();
        let mut bytes = 0usize;
        while batch.len() < self.params.batch_size {
            let Some(next) = self.mempool.front() else {
                break;
            };
            if bytes + next.len() > self.params.max_batch_bytes {
                if batch.is_empty() {
                    // One transaction over the bound on its own can never be
                    // proposed; dropping it is the only way past it.
                    self.mempool.pop_front();
                    continue;
                }
                break;
            }
            bytes += next.len();
            batch.extend(self.mempool.pop_front());
        }
        batch
    }

    fn propose(&mut self, round: u64, now_ms: u64, out: &mut Output) {
        let mut parents: Vec<Digest> = self.dag.round_digests(round - 1);
        parents.sort_unstable();
        // A vertex that never gathered a quorum is abandoned; its
        // transactions go back to the front of the queue, not into the void.
        if let Some((abandoned, _)) = self.pending.take() {
            for tx in abandoned.batch.into_iter().rev() {
                self.mempool.push_front(tx);
            }
        }
        let vertex = Vertex {
            epoch: self.params.epoch,
            round,
            author: self.id,
            timestamp_ms: now_ms,
            parents,
            batch: self.next_batch(),
        };
        let digest = vertex.digest();
        let signature = self.auth.sign(
            SignContext {
                kind: SignKind::Proposal,
                round,
                author: self.id,
            },
            &digest,
        );
        self.voted.insert((round, self.id), digest);
        self.seen.insert(
            (round, self.id),
            (vertex.clone(), signature.clone(), digest),
        );
        self.round = round;
        self.waiting_since = None;
        self.last_proposed_ms = now_ms;
        // Our own vote counts; with a committee of one that is a quorum.
        let mut votes = BTreeMap::new();
        votes.insert(self.id, signature.clone());
        self.pending = Some((vertex.clone(), votes));
        self.pending_digest = Some(digest);
        out.sends
            .push((Dest::All, Message::Propose { vertex, signature }));
        if self.committee.is_quorum([self.id]) {
            self.certify_alone(out);
        }
    }

    /// A committee of one certifies its own vertex on its own vote.
    fn certify_alone(&mut self, out: &mut Output) {
        let Some((vertex, votes)) = self.pending.take() else {
            return;
        };
        let (voters, signatures) = votes.into_iter().unzip();
        let cert = Certificate {
            vertex,
            votes: voters,
            signatures,
        };
        let digest = self.pending_digest.unwrap_or_else(|| cert.digest());
        out.sends.push((Dest::All, Message::Cert(cert.clone())));
        self.accept(cert, digest, self.id, out);
    }
}
