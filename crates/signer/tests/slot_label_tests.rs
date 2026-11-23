//! Audit CT-05 (needs Eric): the signer decides on the round and author the
//! node *claims*, but signs only `domain ‖ digest`, and the digest commits the
//! vertex's real epoch, round and author (`crates/dag-bft/src/vertex.rs`). A
//! compromised node can label a second vertex for an already signed slot
//! with another round and get it signed: two valid signatures for one real
//! `(round, author)` slot, which is the equivocation the signer exists to
//! prevent.
//!
//! Ignored until Eric picks a fix (the [needs-eric] PR lists the options).
//! Either fix turns it green: the signer recomputes the digest from the
//! vertex header and refuses the false label (option A), or the signed bytes
//! carry the round and author, so the signature does not verify for the real
//! slot (option B).

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
#[ignore = "audit CT-05: red until Eric chooses option A or B ([needs-eric] PR)"]
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
    assert!(matches!(service.handle(&honest(&first)), Response::Signed(_)));
    // The protection rule holds when the label is true.
    assert!(matches!(service.handle(&honest(&second)), Response::Refused(_)));

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
