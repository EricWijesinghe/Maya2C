//! The local DAG of certificates, indexed by round and author.
//!
//! `BTreeMap` throughout: the commit rule iterates this structure, and
//! iteration order that depended on a hasher's seed would let two honest
//! nodes order the same DAG differently (Standing Order 4).

use std::collections::{BTreeMap, BTreeSet};

use crate::vertex::{Certificate, Digest, ValidatorId};

/// Certificates a node holds, all of whose parents it also holds (or has
/// garbage-collected).
#[derive(Clone, Debug, Default)]
pub struct Dag {
    rounds: BTreeMap<u64, BTreeMap<ValidatorId, Certificate>>,
    index: BTreeMap<Digest, (u64, ValidatorId)>,
    /// The reverse of `index`: each held certificate's digest, computed once
    /// on insert. A digest hashes the whole payload — megabytes for a full
    /// vertex — and the commit rule asks for anchors' digests on every
    /// message, so recomputing it was the engine's hottest path.
    digests: BTreeMap<(u64, ValidatorId), Digest>,
    /// Rounds below this were garbage-collected.
    gc_round: u64,
}

impl Dag {
    /// A DAG holding `genesis`.
    pub fn new(genesis: Vec<Certificate>) -> Self {
        let mut dag = Self::default();
        for c in genesis {
            dag.put(c);
        }
        dag
    }

    fn put(&mut self, cert: Certificate) {
        let digest = cert.digest();
        self.put_digested(cert, digest);
    }

    fn put_digested(&mut self, cert: Certificate, digest: Digest) {
        let (round, author) = (cert.vertex.round, cert.vertex.author);
        self.index.insert(digest, (round, author));
        self.digests.insert((round, author), digest);
        self.rounds.entry(round).or_default().insert(author, cert);
    }

    /// The digest of the certificate in slot `(round, author)`, without
    /// rehashing it.
    pub fn digest_at(&self, round: u64, author: ValidatorId) -> Option<Digest> {
        self.digests.get(&(round, author)).copied()
    }

    /// Whether `digest` is held, or is old enough that it was collected.
    pub fn contains(&self, digest: &Digest) -> bool {
        self.index.contains_key(digest)
    }

    /// Parents of `cert` not yet held. A round at or below the GC horizon
    /// counts as held: those parents were ordered or abandoned already.
    pub fn missing_parents(&self, cert: &Certificate) -> Vec<Digest> {
        if cert.vertex.round <= self.gc_round {
            return Vec::new();
        }
        cert.vertex
            .parents
            .iter()
            .filter(|p| !self.contains(p))
            .copied()
            .collect()
    }

    /// Inserts `cert` if every parent is held and the slot is free. Returns
    /// whether it was inserted. A second, different certificate for an
    /// occupied `(round, author)` slot is refused: quorum intersection makes
    /// it impossible with ≤ f faults, so seeing one is evidence, not data.
    pub fn insert(&mut self, cert: Certificate) -> bool {
        let digest = cert.digest();
        self.insert_digested(cert, digest)
    }

    /// [`Dag::insert`] for a caller that already holds `cert.digest()`.
    pub fn insert_digested(&mut self, cert: Certificate, digest: Digest) -> bool {
        let round = cert.vertex.round;
        if round < self.gc_round || !self.missing_parents(&cert).is_empty() {
            return false;
        }
        if self.get(round, cert.vertex.author).is_some() {
            return false;
        }
        self.put_digested(cert, digest);
        true
    }

    /// The certificate in slot `(round, author)`.
    pub fn get(&self, round: u64, author: ValidatorId) -> Option<&Certificate> {
        self.rounds.get(&round)?.get(&author)
    }

    /// The certificate with `digest`.
    pub fn by_digest(&self, digest: &Digest) -> Option<&Certificate> {
        let (round, author) = self.index.get(digest)?;
        self.get(*round, *author)
    }

    /// Certificates of `round`, by author.
    pub fn round(&self, round: u64) -> impl Iterator<Item = &Certificate> {
        self.rounds
            .get(&round)
            .into_iter()
            .flat_map(BTreeMap::values)
    }

    /// Digests of `round`'s certificates, by author, from the index.
    pub fn round_digests(&self, round: u64) -> Vec<Digest> {
        self.digests
            .range((round, 0)..=(round, ValidatorId::MAX))
            .map(|(_, d)| *d)
            .collect()
    }

    /// Number of certificates in `round`.
    pub fn round_len(&self, round: u64) -> usize {
        self.rounds.get(&round).map_or(0, BTreeMap::len)
    }

    /// Highest round held.
    pub fn highest_round(&self) -> u64 {
        self.rounds.keys().next_back().copied().unwrap_or(0)
    }

    /// Whether `to` is reachable from `from` by parent links.
    pub fn has_path(&self, from: &Digest, to: &Digest) -> bool {
        let Some(&(target_round, _)) = self.index.get(to) else {
            return false;
        };
        let mut frontier = BTreeSet::from([*from]);
        while !frontier.is_empty() {
            if frontier.contains(to) {
                return true;
            }
            let mut next = BTreeSet::new();
            for d in &frontier {
                if let Some(c) = self.by_digest(d)
                    && c.vertex.round > target_round
                {
                    next.extend(c.vertex.parents.iter().copied());
                }
            }
            frontier = next;
        }
        false
    }

    /// Drops every round below `round`.
    pub fn collect_below(&mut self, round: u64) {
        if round <= self.gc_round {
            return;
        }
        let kept = self.rounds.split_off(&round);
        let kept_digests = self.digests.split_off(&(round, 0));
        for digest in self.digests.values() {
            self.index.remove(digest);
        }
        self.digests = kept_digests;
        self.rounds = kept;
        self.gc_round = round;
    }

    /// Rounds below this are gone.
    pub fn gc_round(&self) -> u64 {
        self.gc_round
    }

    /// Certificates held; bounded by GC.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether nothing is held.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}
