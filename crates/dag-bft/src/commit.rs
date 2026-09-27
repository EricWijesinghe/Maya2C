//! The Bullshark commit rule (Spiegelman et al., CCS 2022), partially
//! synchronous variant.
//!
//! Even rounds have an anchor chosen round-robin. An anchor is committed
//! *directly* when at least f + 1 certificates of the next round reference
//! it. Committing an anchor first commits, oldest first, every earlier
//! uncommitted anchor it has a path to — quorum intersection guarantees that
//! any anchor some honest node committed directly is reachable from every
//! later committed anchor, so every honest node commits the same anchor
//! sequence. Each anchor's not-yet-ordered causal history is then emitted
//! sorted by `(round, author)`: a total order that is a pure function of the
//! DAG, with no clock and no hash-map order in it.

use std::collections::BTreeSet;

use crate::dag::Dag;
use crate::vertex::{Certificate, Committee, Digest};

/// One committed anchor and the causal history it ordered: the unit the node
/// turns into exactly one block, so block boundaries are a pure function of
/// the DAG and not of when a node happened to run the rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubDag {
    /// The anchor.
    pub anchor: Certificate,
    /// Newly ordered certificates, ending with the anchor, by (round, author).
    pub certificates: Vec<Certificate>,
}

/// Commit-rule state: what has been ordered so far.
#[derive(Clone, Debug, Default)]
pub struct Committer {
    last_committed_round: u64,
    /// Digests already emitted, pruned with the DAG.
    ordered: BTreeSet<(u64, Digest)>,
    /// Anchors committed, in order: the check two honest nodes must agree on.
    anchors: Vec<Digest>,
}

impl Committer {
    /// Nothing ordered yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Round of the last committed anchor (0 before any).
    pub fn last_committed_round(&self) -> u64 {
        self.last_committed_round
    }

    /// Committed anchors in commit order.
    pub fn anchors(&self) -> &[Digest] {
        &self.anchors
    }

    /// Runs the rule over `dag` and returns newly ordered certificates.
    pub fn try_commit(&mut self, dag: &Dag, committee: Committee) -> Vec<Certificate> {
        self.try_commit_sub_dags(dag, committee)
            .into_iter()
            .flat_map(|s| s.certificates)
            .collect()
    }

    /// Runs the rule over `dag` and returns one [`SubDag`] per newly
    /// committed anchor, oldest first.
    pub fn try_commit_sub_dags(&mut self, dag: &Dag, committee: Committee) -> Vec<SubDag> {
        let mut out = Vec::new();
        let mut round = self.last_committed_round + 2;
        while round < dag.highest_round() {
            if let Some(anchor) = Self::anchor(dag, committee, round) {
                let digest = anchor.digest();
                let votes = dag
                    .round(round + 1)
                    .filter(|c| c.vertex.parents.contains(&digest))
                    .count();
                if votes >= usize::from(committee.validity()) {
                    self.commit_chain(dag, committee, anchor, &mut out);
                }
            }
            round += 2;
        }
        out
    }

    fn anchor(dag: &Dag, committee: Committee, round: u64) -> Option<&Certificate> {
        dag.get(round, committee.leader(round)?)
    }

    /// Commits `anchor` and every earlier uncommitted anchor linked to it.
    fn commit_chain(
        &mut self,
        dag: &Dag,
        committee: Committee,
        anchor: &Certificate,
        out: &mut Vec<SubDag>,
    ) {
        let mut chain = vec![anchor.clone()];
        let mut current = anchor.digest();
        let mut round = anchor.vertex.round;
        while round >= self.last_committed_round + 4 {
            round -= 2;
            if let Some(earlier) = Self::anchor(dag, committee, round)
                && dag.has_path(&current, &earlier.digest())
            {
                current = earlier.digest();
                chain.push(earlier.clone());
            }
        }
        for a in chain.iter().rev() {
            let mut certificates = Vec::new();
            self.order_history(dag, a, &mut certificates);
            self.anchors.push(a.digest());
            out.push(SubDag {
                anchor: a.clone(),
                certificates,
            });
        }
        self.last_committed_round = anchor.vertex.round;
    }

    /// Emits `anchor`'s causal history not yet ordered, by (round, author).
    fn order_history(&mut self, dag: &Dag, anchor: &Certificate, out: &mut Vec<Certificate>) {
        let mut seen = BTreeSet::new();
        let mut stack = vec![anchor.digest()];
        let mut history = Vec::new();
        while let Some(d) = stack.pop() {
            let Some(c) = dag.by_digest(&d) else {
                continue; // below the GC horizon
            };
            if self.ordered.contains(&(c.vertex.round, d)) || !seen.insert(d) {
                continue;
            }
            stack.extend(c.vertex.parents.iter().copied());
            history.push(c);
        }
        history.sort_by_key(|c| (c.vertex.round, c.vertex.author));
        for c in history {
            self.ordered.insert((c.vertex.round, c.digest()));
            out.push(c.clone());
        }
    }

    /// Forgets ordered digests below `round`, matching the DAG's GC.
    pub fn collect_below(&mut self, round: u64) {
        self.ordered = self.ordered.split_off(&(round, [0u8; 32]));
    }
}
