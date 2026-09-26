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
        let (round, author) = (cert.vertex.round, cert.vertex.author);
        self.index.insert(cert.digest(), (round, author));
        self.rounds.entry(round).or_default().insert(author, cert);
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
        let round = cert.vertex.round;
        if round < self.gc_round || !self.missing_parents(&cert).is_empty() {
            return false;
        }
        if self.get(round, cert.vertex.author).is_some() {
            return false;
        }
        self.put(cert);
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
        for certs in self.rounds.values() {
            for c in certs.values() {
                self.index.remove(&c.digest());
            }
        }
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
