//! Audit CT-05 (needs Eric): the signer decides on the round and author the
//! node *claims*, but signs only `domain ‖ digest`, and the digest commits the
//! vertex's real epoch, round and author (`crates/dag-bft/src/vertex.rs`). A
//! compromised node can label a second vertex for an already signed slot
//! with another round and get it signed: two valid signatures for one real
//! `(round, author)` slot, which is the equivocation the signer exists to
//! prevent.
//!
//! Eric chose option B1 on 2026-11-24: the signed bytes carry the round and
//! author (`maya_dag_bft::slot_message`), so a signature obtained under a
//! false label does not verify for the real slot.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_crypto_pq::suite::{MasterSeed, validator};
use maya_dag_bft::Vertex;
use maya_signer::backend::KeystoreBackend;
use maya_signer::protection::{Kind, SlashingDb};
use maya_signer::service::{Request, Response, Service, signed_message};
use tempfile::TempDir;

/// A round-5 vertex by author 0; `stamp` makes two of them conflict.
fn vertex(stamp: u64) -> Vertex {
    Vertex {
        round: 5,
        timestamp_ms: stamp,
        ..Vertex::genesis_in(1, 0)
    }
}

#[test]
fn a_second_vertex_for_a_signed_slot_cannot_be_signed_under_another_label() {
    let dir = TempDir::new().unwrap();
    let mut service = Service::new(
        KeystoreBackend::new(&MasterSeed::from_bytes([7; 32])),
        SlashingDb::open(&dir.path().join("protection.jsonl")).unwrap(),
    );
    let (first, second) = (vertex(1), vertex(2));
    assert_ne!(first.digest(), second.digest());
    let honest = |v: &Vertex| Request {
        kind: Kind::Vertex,
        round: v.round,
        author: v.author,
        digest: v.digest(),
    };
    assert!(matches!(
        service.handle(&honest(&first)),
        Response::Signed(_)
    ));
    // The protection rule holds when the label is true.
    assert!(matches!(
        service.handle(&honest(&second)),
        Response::Refused(_)
    ));

    // The same second vertex, labelled one round later.
    let relabelled = Request {
        round: second.round + 1,
        ..honest(&second)
    };
    let Response::Signed(sig) = service.handle(&relabelled) else {
        return; // refused: the false label was caught
    };
    // What a validator checks for `second` at its real slot.
    let message = signed_message(&honest(&second));
    let public: &[u8; validator::PUBLIC_KEY_LEN] = service.public_key().try_into().unwrap();
    let sig: &[u8; validator::SIGNATURE_LEN] = sig.as_slice().try_into().unwrap();
    assert!(
        !validator::verify(public, &message, sig),
        "a second vertex for slot (round 5, author 0) now carries a valid signature"
    );
}
