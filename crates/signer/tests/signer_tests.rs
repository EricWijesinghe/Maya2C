//! Master Prompt 16 §1-2: the remote signer's guarantees, tested.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::net::{TcpListener, TcpStream};

use maya_crypto_pq::suite::{MasterSeed, SuiteId, verify};
use maya_signer::backend::{BackendError, KeystoreBackend, SignerBackend, kms, pkcs11};
use maya_signer::channel::{self, ChannelError, Identity};
use maya_signer::keystore::{self, KeystoreError};
use maya_signer::protection::{Kind, SlashingDb};
use maya_signer::service::{Request, Response, Service, signed_message};
use tempfile::TempDir;

fn seed(b: u8) -> MasterSeed {
    MasterSeed::from_bytes([b; 32])
}

fn service(dir: &TempDir) -> Service<KeystoreBackend> {
    Service::new(
        KeystoreBackend::new(&seed(7)),
        SlashingDb::open(&dir.path().join("protection.jsonl")).unwrap(),
    )
}

/// A request for `author`'s vertex in `round`; `tag` names the vertex, so two
/// different tags are two conflicting vertices.
fn req(kind: Kind, round: u64, author: u16, tag: &[u8]) -> Request {
    Request {
        kind,
        round,
        author,
        digest: *blake3::hash(tag).as_bytes(),
    }
}

fn signed(r: &Response) -> bool {
    matches!(r, Response::Signed(_))
}

#[test]
fn keystore_round_trips_and_refuses_a_wrong_passphrase_or_a_tampered_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("key.json");
    let public = keystore::create(&path, &seed(9), b"correct horse").unwrap();
    let opened = keystore::open(&path, b"correct horse").unwrap();
    assert_eq!(
        KeystoreBackend::new(&opened).public_key(),
        public.as_slice()
    );
    assert!(matches!(
        keystore::open(&path, b"wrong"),
        Err(KeystoreError::Decrypt)
    ));
    // Flip one nibble of the sealed seed: the AEAD tag must catch it.
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let ct = doc["ciphertext"].as_str().unwrap().to_string();
    let first = if ct.starts_with('0') { "1" } else { "0" };
    doc["ciphertext"] = serde_json::Value::String(format!("{first}{}", &ct[1..]));
    std::fs::write(&path, doc.to_string()).unwrap();
    assert!(
        matches!(
            keystore::open(&path, b"correct horse"),
            Err(KeystoreError::Decrypt)
        ),
        "a tampered keystore must not open"
    );
}

#[test]
fn signatures_cover_exactly_the_bytes_validators_verify() {
    let dir = TempDir::new().unwrap();
    let mut s = service(&dir);
    let r = req(Kind::Vertex, 1, 0, b"vertex");
    let Response::Signed(sig) = s.handle(&r) else {
        panic!("refused")
    };
    // ADR-033: VOTE_DOMAIN ‖ digest, the node's own message, nothing more.
    let expected = [maya_dag_bft::VOTE_DOMAIN, r.digest.as_slice()].concat();
    assert_eq!(signed_message(&r), expected);
    verify(SuiteId::MlDsa65, s.public_key(), &expected, &sig).unwrap();
    assert!(
        verify(SuiteId::MlDsa65, s.public_key(), &r.digest, &sig).is_err(),
        "the bare digest must not verify"
    );
}

#[test]
fn every_authors_vote_in_one_round_is_signed() {
    // The finding behind ADR-033: a validator votes for each author's vertex
    // in a round. A one-message-per-round rule refused the second and would
    // have stalled consensus.
    let dir = TempDir::new().unwrap();
    let mut s = service(&dir);
    for author in 0..4 {
        assert!(
            signed(&s.handle(&req(Kind::Vote, 5, author, &author.to_le_bytes()))),
            "vote for author {author} in round 5"
        );
    }
    // A slow author's vertex from an earlier round is still votable.
    assert!(signed(&s.handle(&req(Kind::Vote, 3, 2, b"late"))));
}

#[test]
fn a_node_restored_from_an_old_backup_cannot_sign_a_conflicting_vote() {
    let dir = TempDir::new().unwrap();
    let mut s = service(&dir);
    for round in [2, 4, 6, 8, 10] {
        assert!(signed(&s.handle(&req(Kind::Vote, round, 1, b"honest"))));
    }
    // The restored node has forgotten rounds 6..10 and sees other vertices.
    assert!(
        !signed(&s.handle(&req(Kind::Vote, 6, 1, b"from backup"))),
        "a second digest for a slot already signed"
    );
    // An identical retry is harmless and allowed.
    assert!(signed(&s.handle(&req(Kind::Vote, 8, 1, b"honest"))));
    // Proposals do keep a watermark: an honest validator's own rounds only rise.
    assert!(signed(&s.handle(&req(Kind::Vertex, 9, 0, b"mine"))));
    assert!(
        !signed(&s.handle(&req(Kind::Vertex, 7, 0, b"from backup"))),
        "a proposal below the highest round proposed"
    );
    // A proposal and the author's own vote are one signature, one slot.
    assert!(signed(&s.handle(&req(Kind::Vote, 9, 0, b"mine"))));
    assert!(!signed(&s.handle(&req(Kind::Vote, 9, 0, b"other"))));
}

#[test]
fn two_nodes_with_the_same_key_the_second_is_refused() {
    // One signer, two nodes asking for the same round.
    let dir = TempDir::new().unwrap();
    let mut s = service(&dir);
    assert!(signed(&s.handle(&req(
        Kind::Vertex,
        11,
        0,
        b"node A's vertex"
    ))));
    assert!(!signed(&s.handle(&req(
        Kind::Vertex,
        11,
        0,
        b"node B's vertex"
    ))));

    // Two signers: moving the key to a second machine carries the history via
    // the interchange file, and the second machine refuses what the first signed.
    let dir2 = TempDir::new().unwrap();
    let mut s2 = service(&dir2);
    let exported = s.db().export(s.public_key());
    let text = serde_json::to_string(&exported).unwrap();
    let pk = s2.public_key().to_vec();
    let added = s2
        .db_mut()
        .import(&serde_json::from_str(&text).unwrap(), &pk)
        .unwrap();
    assert_eq!(added, 1);
    assert!(!signed(&s2.handle(&req(
        Kind::Vertex,
        11,
        0,
        b"node B's vertex"
    ))));
    assert!(signed(&s2.handle(&req(Kind::Vertex, 12, 0, b"next"))));
}

#[test]
fn a_crash_between_persist_and_signature_cannot_lead_to_a_double_sign() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("protection.jsonl");
    {
        // The approval is durable, then the process dies before signing.
        let mut db = SlashingDb::open(&path).unwrap();
        db.approve(Kind::Vote, 20, 3, req(Kind::Vote, 20, 3, b"A").digest)
            .unwrap();
    }
    let mut s = Service::new(
        KeystoreBackend::new(&seed(7)),
        SlashingDb::open(&path).unwrap(),
    );
    assert!(
        !signed(&s.handle(&req(Kind::Vote, 20, 3, b"B"))),
        "a different message for the crashed round"
    );
    assert!(
        signed(&s.handle(&req(Kind::Vote, 20, 3, b"A"))),
        "the approved message may still be signed"
    );
}

#[test]
fn a_torn_record_fails_closed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("protection.jsonl");
    {
        let mut db = SlashingDb::open(&path).unwrap();
        db.approve(Kind::Vote, 1, 0, [1; 32]).unwrap();
    }
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"kind\":\"vote\",\"rou")
        .unwrap();
    assert!(
        SlashingDb::open(&path).is_err(),
        "a signer must refuse to start on a damaged history rather than forget it"
    );
}

#[test]
fn hsm_and_kms_backends_say_why_they_are_unavailable() {
    for r in [pkcs11("/usr/lib/softhsm.so", 0), kms("kms://key")] {
        let Err(BackendError::Unavailable(why)) = r else {
            panic!("expected Unavailable")
        };
        assert!(why.contains("ADR-022"));
    }
}

fn pair() -> (TcpStream, TcpStream) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
    let (s, _) = l.accept().unwrap();
    (c, s)
}

#[test]
fn the_channel_authenticates_both_sides_and_carries_signing_requests() {
    let dir = TempDir::new().unwrap();
    let (node_id, signer_id) = (Identity::from_seed(&seed(1)), Identity::from_seed(&seed(2)));
    let node_pk = node_id.public_key().to_vec();
    let signer_pk = signer_id.public_key().to_vec();
    let (c, s) = pair();
    let mut svc = service(&dir);
    let server = std::thread::spawn(move || {
        let mut ch = channel::server(s, &signer_id, &[node_pk]).unwrap();
        for _ in 0..2 {
            let r: Request = serde_json::from_slice(&ch.recv().unwrap()).unwrap();
            ch.send(&serde_json::to_vec(&svc.handle(&r)).unwrap())
                .unwrap();
        }
    });
    let mut ch = channel::client(c, &node_id, &signer_pk).unwrap();
    let mut ask = |r: &Request| -> Response {
        ch.send(&serde_json::to_vec(r).unwrap()).unwrap();
        serde_json::from_slice(&ch.recv().unwrap()).unwrap()
    };
    assert!(signed(&ask(&req(Kind::Vertex, 1, 0, b"v1"))));
    assert!(!signed(&ask(&req(Kind::Vertex, 1, 0, b"v1-conflict"))));
    server.join().unwrap();
}

#[test]
fn unpinned_peers_are_refused_on_both_sides() {
    let (node, signer, stranger) = (
        Identity::from_seed(&seed(1)),
        Identity::from_seed(&seed(2)),
        Identity::from_seed(&seed(3)),
    );
    // A stranger calling the signer: refused before any KEM work.
    let (c, s) = pair();
    let signer_pk = signer.public_key().to_vec();
    let allowed = vec![node.public_key().to_vec()];
    let t = std::thread::spawn(move || channel::server(s, &signer, &allowed).err());
    let _ = channel::client(c, &stranger, &signer_pk);
    assert!(matches!(t.join().unwrap(), Some(ChannelError::Unpinned)));

    // The node reaching an impostor signer: refused by the pin.
    let (c, s) = pair();
    let impostor = Identity::from_seed(&seed(4));
    let node_pk = node.public_key().to_vec();
    std::thread::spawn(move || {
        let _ = channel::server(s, &impostor, &[node_pk]);
    });
    let genuine = Identity::from_seed(&seed(2)).public_key().to_vec();
    assert!(matches!(
        channel::client(c, &node, &genuine),
        Err(ChannelError::Unpinned)
    ));
}

/// A stream that records every byte written, so a test can replay a frame.
struct Recording {
    inner: TcpStream,
    written: Vec<u8>,
}

impl std::io::Read for Recording {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for Recording {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.written.extend_from_slice(buf);
        self.inner.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Server that reads two frames and reports the second's outcome.
fn two_frame_server(
    s: TcpStream,
    signer: Identity,
    node_pk: Vec<u8>,
) -> std::thread::JoinHandle<(Vec<u8>, Option<String>)> {
    std::thread::spawn(move || {
        let mut ch = channel::server(s, &signer, &[node_pk]).unwrap();
        let first = ch.recv().unwrap();
        (first, ch.recv().err().map(|e| e.to_string()))
    })
}

#[test]
fn a_replayed_frame_is_rejected() {
    let (node, signer) = (Identity::from_seed(&seed(1)), Identity::from_seed(&seed(2)));
    let (node_pk, signer_pk) = (node.public_key().to_vec(), signer.public_key().to_vec());
    let (c, s) = pair();
    let t = two_frame_server(s, signer, node_pk);
    let mut ch = channel::client(
        Recording {
            inner: c,
            written: Vec::new(),
        },
        &node,
        &signer_pk,
    )
    .unwrap();
    let before = ch.stream_mut().written.len();
    ch.send(b"round 5 please").unwrap();
    let frame = ch.stream_mut().written[before..].to_vec();
    ch.stream_mut().inner.write_all(&frame).unwrap(); // the same bytes again
    let (first, second) = t.join().unwrap();
    assert_eq!(first, b"round 5 please");
    assert!(
        second.unwrap().contains("frame tag"),
        "a replayed frame must not decrypt under the next counter"
    );
}

#[test]
fn a_forged_frame_is_rejected() {
    let (node, signer) = (Identity::from_seed(&seed(1)), Identity::from_seed(&seed(2)));
    let (node_pk, signer_pk) = (node.public_key().to_vec(), signer.public_key().to_vec());
    let (c, s) = pair();
    let t = two_frame_server(s, signer, node_pk);
    let mut ch = channel::client(c, &node, &signer_pk).unwrap();
    ch.send(b"genuine").unwrap();
    // Correct framing (total length, one field of 32 bytes), garbage ciphertext.
    let mut frame = 36u32.to_le_bytes().to_vec();
    frame.extend_from_slice(&32u32.to_le_bytes());
    frame.extend_from_slice(&[0xAA; 32]);
    ch.stream_mut().write_all(&frame).unwrap();
    let (first, second) = t.join().unwrap();
    assert_eq!(first, b"genuine");
    assert!(second.unwrap().contains("frame tag"));
}

#[test]
fn history_is_bounded_and_survives_compaction() {
    use maya_signer::protection::{MAX_AHEAD, MAX_AUTHORS, RETAIN_ROUNDS};
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("protection.jsonl");
    let top = 3 * RETAIN_ROUNDS + 400;
    {
        let mut db = SlashingDb::open(&path).unwrap();
        for round in 1..=top {
            db.approve(
                Kind::Vertex,
                round,
                0,
                *blake3::hash(&round.to_le_bytes()).as_bytes(),
            )
            .unwrap();
        }
        let floor = db.floor();
        assert_eq!(floor, top - RETAIN_ROUNDS);
        // Below the floor: refused, even though its slot record is gone.
        assert!(db.approve(Kind::Vote, floor - 1, 1, [7; 32]).is_err());
        // Too far ahead, or an author id no committee has.
        assert!(
            db.approve(Kind::Vote, top + MAX_AHEAD + 1, 1, [7; 32])
                .is_err()
        );
        assert!(db.approve(Kind::Vote, top, MAX_AUTHORS, [7; 32]).is_err());
        // Inside the window: fine.
        db.approve(Kind::Vote, top - 5, 1, [7; 32]).unwrap();
    }
    let lines = std::fs::read_to_string(&path).unwrap().lines().count();
    let bound = usize::try_from(2 * (RETAIN_ROUNDS + 2)).unwrap() + 1_024;
    assert!(
        lines <= bound,
        "file compacted: {lines} lines, bound {bound}"
    );
    // After reopening: the history kept still refuses a conflict, the floor
    // still refuses the past, and the proposal watermark survived.
    let mut db = SlashingDb::open(&path).unwrap();
    assert!(db.approve(Kind::Vote, top - 5, 1, [8; 32]).is_err());
    assert!(db.approve(Kind::Vertex, top - 10, 0, [9; 32]).is_err());
    assert!(db.approve(Kind::Vertex, 5, 0, [9; 32]).is_err());
    db.approve(Kind::Vertex, top + 1, 0, [9; 32]).unwrap();
}
