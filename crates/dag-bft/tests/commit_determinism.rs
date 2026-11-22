//! Committed sub-DAGs must not depend on how many anchors a node happens to
//! commit in one step.
//!
//! Each sub-DAG becomes one block, so two honest nodes that commit the same
//! anchor sequence must also order the same certificates under each anchor.
//! The engine garbage-collects after every step that commits, so the rounds
//! it still holds depend on its step batching. The ordering cutoff therefore
//! has to be a function of the committed anchor sequence, not of what the
//! local DAG still holds.
//!
//! Shape, with four members (f = 1): members 0-2 reference each other every
//! round; member 3 keeps a chain of its own certificates that no honest
//! vertex references until round `K`, where every vertex references its
//! round `K - 1` certificate. Node A receives everything in round order and
//! commits one anchor per step. Node B receives the round `K` and `K + 1`
//! certificates before the one round `K - 1` certificate that completes the
//! vote for anchor `K - 2`, so a single step commits anchors `K - 2` and `K`.

use maya_dag_bft::{
    Certificate, Committee, Digest, GC_DEPTH, Message, Params, SubDag, Unauthenticated, Validator,
    ValidatorId, Vertex,
};

const N: u16 = 4;
const WEAK: ValidatorId = 3;
/// Anchor whose history reaches member 3's chain: leader 1. Anchor `K - 2`
/// is led by member 0 and anchor `K - 4` by member 3 (never committed: no
/// honest vertex references it).
const K: u64 = 114;

fn cert(round: u64, author: ValidatorId, parents: &[Digest]) -> Certificate {
    let mut parents = parents.to_vec();
    parents.sort_unstable();
    Certificate {
        vertex: Vertex {
            epoch: 0,
            round,
            author,
            timestamp_ms: 0,
            parents,
            batch: Vec::new(),
        },
        votes: vec![0, 1, 2],
        signatures: vec![Vec::new(); 3],
    }
}

/// Every certificate of the schedule, in round order.
fn schedule() -> Vec<Certificate> {
    let committee = Committee::new(N);
    let genesis = Certificate::genesis(&committee);
    let mut prev: Vec<Digest> = genesis.iter().map(Certificate::digest).collect();
    let mut all = Vec::new();
    for round in 1..K {
        let honest = [prev[0], prev[1], prev[2]];
        // The weak chain: its own previous vertex plus two honest parents
        // that are never the anchor of `K - 2` (member 0), so it adds no
        // vote for that anchor.
        let weak_parents = [prev[1], prev[2], prev[3]];
        let mut row: Vec<Certificate> = (0..3).map(|a| cert(round, a, &honest)).collect();
        row.push(cert(round, WEAK, &weak_parents));
        prev = row.iter().map(Certificate::digest).collect();
        if round == K - 1 {
            // Member 0's round `K - 1` vertex is never certified.
            row.remove(0);
        }
        all.extend(row);
    }
    // Round K - 1 holds members 1, 2 and 3; every round-K vertex references
    // all three, so member 3's chain enters anchor K's history.
    let last: Vec<Digest> = all[all.len() - 3..]
        .iter()
        .map(Certificate::digest)
        .collect();
    let round_k: Vec<Certificate> = (0..3).map(|a| cert(K, a, &last)).collect();
    let kd: Vec<Digest> = round_k.iter().map(Certificate::digest).collect();
    let round_k1: Vec<Certificate> = (0..3).map(|a| cert(K + 1, a, &kd)).collect();
    all.extend(round_k);
    all.extend(round_k1);
    all
}

fn run(order: &[Certificate]) -> Vec<SubDag> {
    let mut node = Validator::observer(Committee::new(N), Params::default(), Unauthenticated);
    let mut subs = Vec::new();
    for c in order {
        subs.extend(node.handle(0, 0, Message::Cert(c.clone())).sub_dags);
    }
    subs
}

fn ids(subs: &[SubDag]) -> Vec<(Digest, Vec<Digest>)> {
    subs.iter()
        .map(|s| {
            (
                s.anchor.digest(),
                s.certificates.iter().map(Certificate::digest).collect(),
            )
        })
        .collect()
}

#[test]
fn step_batching_does_not_change_committed_sub_dags() {
    let all = schedule();
    // The round K - 1 certificate of member 1 completes both the vote for
    // anchor K - 2 and the parents of every round-K vertex.
    let hinge = all
        .iter()
        .position(|c| c.vertex.round == K - 1 && c.vertex.author == 1)
        .expect("hinge certificate");
    let mut batched: Vec<Certificate> = all.clone();
    let late = batched.remove(hinge);
    batched.push(late);

    let a = run(&all);
    let b = run(&batched);

    let anchor_rounds =
        |s: &[SubDag]| -> Vec<u64> { s.iter().map(|d| d.anchor.vertex.round).collect() };
    assert_eq!(
        anchor_rounds(&a),
        anchor_rounds(&b),
        "anchor sequences differ"
    );
    assert!(
        anchor_rounds(&a).ends_with(&[K - 2, K]),
        "anchor K was not committed"
    );
    let weak_in_last = a
        .last()
        .unwrap()
        .certificates
        .iter()
        .filter(|c| c.vertex.author == WEAK)
        .count();
    assert!(
        weak_in_last > 0,
        "the schedule never reached member 3's chain"
    );
    assert!(
        u64::try_from(weak_in_last).unwrap() <= GC_DEPTH + 2,
        "anchor K ordered history from below the collection depth"
    );
    let shape = |s: &[SubDag]| -> Vec<(u64, usize)> {
        s.iter()
            .map(|d| (d.anchor.vertex.round, d.certificates.len()))
            .filter(|(_, n)| *n > 0)
            .collect()
    };
    assert_eq!(
        shape(&a).last(),
        shape(&b).last(),
        "anchor K ordered a different number of certificates"
    );
    assert!(ids(&a) == ids(&b), "the two nodes derived different blocks");
}
