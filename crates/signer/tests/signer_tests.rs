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

fn req(kind: Kind, round: u64, payload: &[u8]) -> Request {
    Request {
        kind,
        round,
        payload: payload.to_vec(),
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
fn signatures_verify_and_cover_the_domain_separated_message() {
    let dir = TempDir::new().unwrap();
    let mut s = service(&dir);
    let r = req(Kind::Vertex, 1, b"vertex digest");
    let Response::Signed(sig) = s.handle(&r) else {
        panic!("refused")
    };
    verify(SuiteId::MlDsa65, s.public_key(), &signed_message(&r), &sig).unwrap();
    assert!(
        verify(SuiteId::MlDsa65, s.public_key(), b"vertex digest", &sig).is_err(),
        "the raw payload must not verify"
    );
}

#[test]
fn a_node_restored_from_an_old_backup_cannot_resign_a_past_round() {
    let dir = TempDir::new().unwrap();
    let mut s = service(&dir);
    for round in [2, 4, 6, 8, 10] {
        assert!(signed(&s.handle(&req(Kind::Vote, round, b"honest"))));
    }
    // The restored node has forgotten rounds 6..10 and builds different votes.
    assert!(
        !signed(&s.handle(&req(Kind::Vote, 6, b"from backup"))),
        "conflicting re-sign of a signed round"
    );
    assert!(
        !signed(&s.handle(&req(Kind::Vote, 7, b"from backup"))),
        "unsigned round below the watermark"
    );
    // An identical retry is harmless and allowed.
    assert!(signed(&s.handle(&req(Kind::Vote, 8, b"honest"))));
    // Rounds are per kind: a vertex at round 3 is still fine.
    assert!(signed(&s.handle(&req(Kind::Vertex, 3, b"vertex"))));
    assert!(signed(&s.handle(&req(Kind::Vote, 11, b"next"))));
}

#[test]
fn two_nodes_with_the_same_key_the_second_is_refused() {
    // One signer, two nodes asking for the same round.
    let dir = TempDir::new().unwrap();
    let mut s = service(&dir);
    assert!(signed(&s.handle(&req(
        Kind::Vertex,
        11,
        b"node A's vertex"
    ))));
    assert!(!signed(&s.handle(&req(
        Kind::Vertex,
        11,
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
        b"node B's vertex"
    ))));
    assert!(signed(&s2.handle(&req(Kind::Vertex, 12, b"next"))));
}

#[test]
fn a_crash_between_persist_and_signature_cannot_lead_to_a_double_sign() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("protection.jsonl");
    {
        // The approval is durable, then the process dies before signing.
        let mut db = SlashingDb::open(&path).unwrap();
        db.approve(
            Kind::Vote,
            20,
            *blake3::hash(&signed_message(&req(Kind::Vote, 20, b"A"))).as_bytes(),
        )
        .unwrap();
    }
    let mut s = Service::new(
        KeystoreBackend::new(&seed(7)),
        SlashingDb::open(&path).unwrap(),
    );
    assert!(
        !signed(&s.handle(&req(Kind::Vote, 20, b"B"))),
        "a different message for the crashed round"
    );
    assert!(
        signed(&s.handle(&req(Kind::Vote, 20, b"A"))),
        "the approved message may still be signed"
    );
}

#[test]
fn a_torn_record_fails_closed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("protection.jsonl");
    {
        let mut db = SlashingDb::open(&path).unwrap();
        db.approve(Kind::Vote, 1, [1; 32]).unwrap();
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
    assert!(signed(&ask(&req(Kind::Vertex, 1, b"v1"))));
    assert!(!signed(&ask(&req(Kind::Vertex, 1, b"v1-conflict"))));
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
