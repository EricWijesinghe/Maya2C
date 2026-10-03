//! The engine under an authenticator that can actually refuse: forged votes
//! and certificates do nothing, a double proposal becomes evidence, and a
//! validator restored from its own safety records does not sign twice.
//!
//! The "signature" is a keyed BLAKE3 MAC per validator — not a signature
//! scheme, but enough for the engine to tell a vote made with a key from one
//! made without it, which is the only property these tests exercise. The
//! node's ML-DSA authenticator is tested in `custom-l1-node`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::needless_range_loop
)]

use std::collections::VecDeque;

use maya_dag_bft::{
    Authenticator, Certificate, Committee, Dest, Digest, Message, Output, Params, SignContext,
    Validator, ValidatorId, Vertex,
};

#[derive(Clone, Copy)]
struct Mac {
    me: ValidatorId,
}

fn mac(who: ValidatorId, digest: &Digest) -> Vec<u8> {
    let key = blake3::hash(&who.to_le_bytes());
    blake3::keyed_hash(key.as_bytes(), digest)
        .as_bytes()
        .to_vec()
}

impl Authenticator for Mac {
    fn sign(&self, _ctx: SignContext, digest: &Digest) -> Vec<u8> {
        mac(self.me, digest)
    }

    fn verify(&self, signer: ValidatorId, digest: &Digest, signature: &[u8]) -> bool {
        signer < 4 && signature == mac(signer, digest)
    }
}

type Node = Validator<Mac>;

fn nodes(n: u16) -> Vec<Node> {
    let committee = Committee::new(n);
    (0..n)
        .map(|i| Validator::with_auth(i, committee.clone(), Params::default(), Mac { me: i }))
        .collect()
}

/// Delivers everything until quiet, ticking at `now`; returns what each node
/// committed.
fn run(nodes: &mut [Node], seed: Vec<(ValidatorId, Output)>, now: u64) -> Vec<Vec<Digest>> {
    let mut queue: VecDeque<(ValidatorId, Dest, Message)> = VecDeque::new();
    let mut committed = vec![Vec::new(); nodes.len()];
    let push = |from: ValidatorId, out: Output, q: &mut VecDeque<_>, c: &mut Vec<Vec<Digest>>| {
        c[usize::from(from)].extend(out.committed.iter().map(Certificate::digest));
        for (d, m) in out.sends {
            q.push_back((from, d, m));
        }
    };
    for (from, out) in seed {
        push(from, out, &mut queue, &mut committed);
    }
    let mut steps = 0;
    while let Some((from, dest, msg)) = queue.pop_front() {
        steps += 1;
        if steps > 20_000 {
            break;
        }
        let targets: Vec<ValidatorId> = match dest {
            Dest::All => (0..nodes.len() as u16).filter(|i| *i != from).collect(),
            Dest::To(i) => vec![i],
        };
        for t in targets {
            let out = nodes[usize::from(t)].handle(now, from, msg.clone());
            push(t, out, &mut queue, &mut committed);
        }
    }
    committed
}

fn start_all(nodes: &mut [Node], now: u64) -> Vec<(ValidatorId, Output)> {
    (0..nodes.len())
        .map(|i| (i as u16, nodes[i].start(now)))
        .collect()
}

#[test]
fn signed_validators_commit_the_same_anchors() {
    let mut ns = nodes(4);
    for (i, v) in ns.iter_mut().enumerate() {
        for k in 0..50u64 {
            v.submit((k * 4 + i as u64).to_le_bytes().to_vec());
        }
    }
    // Rounds advance as fast as messages flow, so one bounded pump is
    // dozens of rounds.
    let seed = start_all(&mut ns, 0);
    run(&mut ns, seed, 0);
    let anchors: Vec<_> = ns.iter().map(|v| v.anchors().to_vec()).collect();
    let shortest = anchors.iter().map(Vec::len).min().unwrap();
    assert!(shortest >= 3, "only {shortest} anchors committed");
    for a in &anchors {
        assert_eq!(
            &a[..shortest],
            &anchors[0][..shortest],
            "anchor logs diverge"
        );
    }
}

#[test]
fn a_forged_vote_does_not_count_toward_a_certificate() {
    let mut ns = nodes(4);
    let out = ns[0].start(0);
    let Some((_, Message::Propose { vertex, .. })) = out.sends.first().cloned() else {
        panic!("validator 0 proposes round 1 at start");
    };
    let digest = vertex.digest();
    // Two forged votes (validator 1 and 2 never signed): no certificate.
    for voter in [1u16, 2] {
        let forged = Message::Vote {
            digest,
            round: 1,
            voter,
            signature: mac(3, &digest), // someone else's key
        };
        let out = ns[0].handle(0, voter, forged);
        assert!(
            !out.sends.iter().any(|(_, m)| matches!(m, Message::Cert(_))),
            "a forged vote produced a certificate"
        );
    }
    // Two genuine votes do certify (with the author's own, that is 3 = n − f).
    let mut certified = false;
    for voter in [1u16, 2] {
        let genuine = Message::Vote {
            digest,
            round: 1,
            voter,
            signature: mac(voter, &digest),
        };
        let out = ns[0].handle(0, voter, genuine);
        certified |= out.sends.iter().any(|(_, m)| matches!(m, Message::Cert(_)));
    }
    assert!(certified);
}

#[test]
fn a_certificate_with_a_forged_signature_is_refused() {
    let mut ns = nodes(4);
    let genesis: Vec<Digest> = {
        let mut g: Vec<_> = Certificate::genesis(&Committee::new(4))
            .iter()
            .map(Certificate::digest)
            .collect();
        g.sort_unstable();
        g
    };
    let vertex = Vertex {
        epoch: 0,
        round: 1,
        author: 3,
        timestamp_ms: 0,
        parents: genesis,
        batch: vec![],
    };
    let d = vertex.digest();
    let forged = Certificate {
        vertex: vertex.clone(),
        votes: vec![0, 1, 3],
        signatures: vec![mac(0, &d), mac(2, &d), mac(3, &d)], // 1's is 2's
    };
    ns[1].handle(0, 3, Message::Cert(forged));
    assert!(!ns[1].dag().contains(&d), "forged certificate accepted");
    let genuine = Certificate {
        vertex,
        votes: vec![0, 1, 3],
        signatures: vec![mac(0, &d), mac(1, &d), mac(3, &d)],
    };
    ns[1].handle(0, 3, Message::Cert(genuine));
    assert!(ns[1].dag().contains(&d));
}

#[test]
fn a_second_signed_proposal_for_one_slot_is_reported_and_not_voted_for() {
    let mut ns = nodes(4);
    let mut propose = |stamp: u64| {
        let mut parents: Vec<Digest> = Certificate::genesis(&Committee::new(4))
            .iter()
            .map(Certificate::digest)
            .collect();
        parents.sort_unstable();
        let vertex = Vertex {
            epoch: 0,
            round: 1,
            author: 2,
            timestamp_ms: stamp,
            parents,
            batch: vec![],
        };
        let signature = mac(2, &vertex.digest());
        ns[0].handle(0, 2, Message::Propose { vertex, signature })
    };
    let first = propose(1);
    assert!(
        first
            .sends
            .iter()
            .any(|(_, m)| matches!(m, Message::Vote { .. }))
    );
    assert!(first.equivocations.is_empty());
    let second = propose(2);
    assert!(
        !second
            .sends
            .iter()
            .any(|(_, m)| matches!(m, Message::Vote { .. })),
        "voted for both"
    );
    assert_eq!(second.equivocations.len(), 1);
    assert!(second.equivocations[0].is_valid(&Mac { me: 0 }));
}

#[test]
fn a_restored_validator_re_sends_its_proposal_instead_of_signing_a_new_one() {
    let committee = Committee::new(4);
    let mut before = Validator::with_auth(1, committee.clone(), Params::default(), Mac { me: 1 });
    let out = before.start(5);
    let Some((_, Message::Propose { vertex, signature })) = out.sends.first().cloned() else {
        panic!("proposes at start");
    };
    // Restart: a fresh engine, restored from the safety record.
    let mut after = Validator::with_auth(1, committee.clone(), Params::default(), Mac { me: 1 });
    after.restore_proposal(vertex.clone(), signature.clone());
    assert!(
        after.start(9_000).sends.is_empty(),
        "signed a new round-1 vertex"
    );
    let resent = after.tick(9_000);
    assert!(resent.sends.iter().any(|(_, m)| matches!(
        m,
        Message::Propose { vertex: v, .. } if v == &vertex
    )));
    // Without the record the same restart signs a conflicting vertex, which is
    // exactly the equivocation the record exists to prevent.
    let mut amnesiac = Validator::with_auth(1, committee.clone(), Params::default(), Mac { me: 1 });
    let fresh = amnesiac.start(9_000);
    let Some((_, Message::Propose { vertex: v2, .. })) = fresh.sends.first() else {
        panic!("proposes at start");
    };
    assert_ne!(v2.digest(), vertex.digest());
}

#[test]
fn an_observer_commits_what_validators_commit_without_signing() {
    let committee = Committee::new(4);
    let mut ns = nodes(4);
    let mut observer = Validator::observer(committee, Params::default(), Mac { me: 0 });
    assert!(observer.start(0).sends.is_empty());
    let seed = start_all(&mut ns, 0);
    let mut queue: VecDeque<(ValidatorId, Dest, Message)> = VecDeque::new();
    for (from, out) in seed {
        queue.extend(out.sends.into_iter().map(|(d, m)| (from, d, m)));
    }
    let mut steps = 0;
    for t in 0..3u64 {
        let now = t * 1_000;
        if t > 0 {
            for i in 0..4 {
                let out = ns[i].tick(now);
                queue.extend(out.sends.into_iter().map(|(d, m)| (i as u16, d, m)));
            }
        }
        while let Some((from, dest, msg)) = queue.pop_front() {
            steps += 1;
            if steps > 20_000 {
                break;
            }
            if let Message::Cert(_) = &msg {
                let out = observer.handle(now, from, msg.clone());
                assert!(
                    out.sends
                        .iter()
                        .all(|(_, m)| matches!(m, Message::Cert(_) | Message::Fetch(_))),
                    "an observer only fetches"
                );
            }
            let targets: Vec<u16> = match dest {
                Dest::All => (0..4).filter(|i| *i != from).collect(),
                Dest::To(i) => vec![i],
            };
            for t in targets {
                let out = ns[usize::from(t)].handle(now, from, msg.clone());
                queue.extend(out.sends.into_iter().map(|(d, m)| (t, d, m)));
            }
        }
    }
    let validator = ns[0].anchors();
    let seen = observer.anchors();
    let common = validator.len().min(seen.len());
    assert!(common >= 3, "observer committed {} anchors", seen.len());
    assert_eq!(&validator[..common], &seen[..common]);
}

#[test]
fn a_certificate_at_the_collection_horizon_is_accepted_without_its_parents() {
    // ADR-038 catch-up resumes the engine with its horizon at the resume
    // round, and joins by accepting certificates there whose parents it
    // will never hold. ADR-040's parent check weighs parents through the
    // DAG, and must not refuse these: both reviews of it caught that it did.
    let mut ns = nodes(4);
    let horizon = 40;
    ns[1].resume_after(horizon);
    let parents: Vec<Digest> = {
        // Digests of certificates this node has never seen and never will.
        let mut p: Vec<Digest> = (0..3u8).map(|i| [i + 1; 32]).collect();
        p.sort_unstable();
        p
    };
    let vertex = Vertex {
        epoch: 0,
        round: horizon,
        author: 3,
        timestamp_ms: 0,
        parents,
        batch: vec![],
    };
    let d = vertex.digest();
    let cert = Certificate {
        vertex: vertex.clone(),
        votes: vec![0, 2, 3],
        signatures: vec![mac(0, &d), mac(2, &d), mac(3, &d)],
    };
    ns[1].handle(0, 3, Message::Cert(cert));
    assert!(
        ns[1].dag().contains(&d),
        "a horizon certificate was refused"
    );

    // One round above the horizon, the same unknown parents are missing,
    // not exempt: the certificate waits for them instead of entering.
    let above = Vertex {
        round: horizon + 1,
        ..vertex
    };
    let d = above.digest();
    let cert = Certificate {
        vertex: above,
        votes: vec![0, 2, 3],
        signatures: vec![mac(0, &d), mac(2, &d), mac(3, &d)],
    };
    ns[1].handle(0, 3, Message::Cert(cert));
    assert!(!ns[1].dag().contains(&d), "entered without its parents");
}
