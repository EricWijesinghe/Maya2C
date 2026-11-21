//! What a validator refuses before it spends a hash or a signature check on
//! it: proposals and certificates outside the round window it can use,
//! vertices listing more parents than the committee has members, and fetch
//! replies past one budget shared by every requester.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use maya_dag_bft::{
    Authenticator, BUFFER_ROUNDS, Certificate, Committee, Dest, Digest, Message, Output, Params,
    SignContext, Validator, ValidatorId, Vertex,
};

const N: u16 = 4;
const INTERVAL_MS: u64 = 1_000;

/// Accepts every signature and counts how many it was asked to check.
#[derive(Clone, Default)]
struct Counting(Arc<AtomicU32>);

impl Authenticator for Counting {
    fn sign(&self, _ctx: SignContext, _digest: &Digest) -> Vec<u8> {
        vec![1]
    }

    fn verify(&self, _signer: ValidatorId, _digest: &Digest, _signature: &[u8]) -> bool {
        self.0.fetch_add(1, Ordering::Relaxed);
        true
    }
}

fn validator() -> (Validator<Counting>, Arc<AtomicU32>) {
    let auth = Counting::default();
    let calls = Arc::clone(&auth.0);
    let params = Params {
        resend_interval_ms: INTERVAL_MS,
        ..Params::default()
    };
    (
        Validator::with_auth(0, Committee::new(N), params, auth),
        calls,
    )
}

/// `count` distinct digests nobody holds, sorted as a vertex lists them.
fn unknown_parents(count: usize) -> Vec<Digest> {
    (0..count)
        .map(|i| *blake3::hash(&i.to_le_bytes()).as_bytes())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn vertex(round: u64, parents: Vec<Digest>) -> Vertex {
    Vertex {
        epoch: 0,
        round,
        author: 1,
        timestamp_ms: 0,
        parents,
        batch: Vec::new(),
    }
}

fn fetches(out: &Output) -> usize {
    out.sends
        .iter()
        .filter(|(_, m)| matches!(m, Message::Fetch(_)))
        .count()
}

fn quorum() -> usize {
    usize::from(Committee::new(N).min_quorum_size())
}

#[test]
fn a_certificate_beyond_the_buffer_window_is_dropped_before_any_signature_check() {
    let (mut node, calls) = validator();
    let quorum = quorum();
    let cert = Certificate {
        vertex: vertex(BUFFER_ROUNDS + 10, unknown_parents(quorum)),
        votes: (0..N).collect(),
        signatures: vec![vec![0]; usize::from(N)],
    };
    let out = node.handle(0, 1, Message::Cert(cert));
    assert_eq!(
        calls.load(Ordering::Relaxed),
        0,
        "a certificate the engine would never buffer must not cost a quorum of verifications"
    );
    assert_eq!(fetches(&out), 0);
}

#[test]
fn a_proposal_beyond_the_round_window_costs_no_check_and_no_fetch() {
    let (mut node, calls) = validator();
    let quorum = quorum();
    let out = node.handle(
        0,
        1,
        Message::Propose {
            vertex: vertex(BUFFER_ROUNDS + 10, unknown_parents(quorum)),
            signature: vec![1],
        },
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        fetches(&out),
        0,
        "a far-future proposal must not start a fetch"
    );
}

#[test]
fn a_proposal_listing_more_parents_than_members_is_refused() {
    let (mut node, calls) = validator();
    let out = node.handle(
        0,
        1,
        Message::Propose {
            vertex: vertex(1, unknown_parents(usize::from(N) + 1)),
            signature: vec![1],
        },
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        fetches(&out),
        0,
        "one round holds at most one certificate per member"
    );
}

#[test]
fn a_proposal_inside_the_window_is_still_checked_and_its_parents_fetched() {
    let (mut node, calls) = validator();
    let out = node.handle(
        0,
        1,
        Message::Propose {
            vertex: vertex(2, unknown_parents(quorum())),
            signature: vec![1],
        },
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(fetches(&out), quorum());
}

#[test]
fn fetch_replies_share_one_budget_whatever_sender_the_frame_claims() {
    let (mut node, _) = validator();
    let held = Certificate::genesis(&Committee::new(N))[1].digest();
    let per_member = 4 * u32::from(N);
    let mut replies = 0u32;
    for from in 0..N {
        for _ in 0..=per_member {
            let out = node.handle(0, from, Message::Fetch(held));
            replies += u32::try_from(
                out.sends
                    .iter()
                    .filter(|(d, m)| *d == Dest::To(from) && matches!(m, Message::Cert(_)))
                    .count(),
            )
            .unwrap();
        }
    }
    assert!(
        replies <= 2 * per_member,
        "{replies} replies in one interval: claiming every member's id multiplied the budget"
    );
    assert!(replies >= per_member, "one member's own budget still holds");
    // The window resets: the next interval answers again.
    let out = node.handle(INTERVAL_MS, 0, Message::Fetch(held));
    let answered = out
        .sends
        .iter()
        .filter(|(_, m)| matches!(m, Message::Cert(_)))
        .count();
    assert_eq!(answered, 1);
}

#[test]
fn a_proposal_below_the_collection_horizon_gets_no_vote() {
    // Below the horizon the engine no longer remembers which digest it voted
    // for in a slot, so a vote there could be a second vote for that slot.
    let (mut node, calls) = validator();
    node.resume_after(60);
    let out = node.handle(
        0,
        1,
        Message::Propose {
            vertex: vertex(10, unknown_parents(quorum())),
            signature: vec![1],
        },
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(
        !out.sends
            .iter()
            .any(|(_, m)| matches!(m, Message::Vote { .. })),
        "a slot below the horizon was voted for"
    );
}
