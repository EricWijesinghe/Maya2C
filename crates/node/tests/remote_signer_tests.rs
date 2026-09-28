//! ADR-033: a validator key held by the remote signer, as the node uses it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::TcpListener;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use custom_l1_node::consensus::bft::auth::MlDsaAuthenticator;
use custom_l1_node::consensus::bft::remote::{RemoteSigner, ValidatorKey};
use custom_l1_node::crypto::keys::VerifyingKey;
use maya_crypto_pq::suite::MasterSeed;
use maya_dag_bft::{Authenticator, SignContext, SignKind};
use maya_signer::backend::{KeystoreBackend, SignerBackend};
use maya_signer::channel::{self, Identity};
use maya_signer::protection::SlashingDb;
use maya_signer::service::{Request, Service};

const DEADLINE: Duration = Duration::from_secs(5);

fn seed(b: u8) -> MasterSeed {
    MasterSeed::from_bytes([b; 32])
}

/// A signer serving one connection on a loopback port, as `maya2c-signer
/// serve` does. Returns its address, the validator public key it holds and
/// its channel public key.
fn spawn_signer(
    dir: &tempfile::TempDir,
    node: &Identity,
) -> (std::net::SocketAddr, VerifyingKey, Vec<u8>, JoinHandle<()>) {
    let backend = KeystoreBackend::new(&seed(7));
    let public = VerifyingKey::from_bytes(backend.public_key().try_into().unwrap()).unwrap();
    let identity = Identity::from_seed(&seed(2));
    let pin = identity.public_key().to_vec();
    let allowed = vec![node.public_key().to_vec()];
    let db = SlashingDb::open(&dir.path().join("protection.jsonl")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let mut service = Service::new(backend, db);
        let (stream, _) = listener.accept().unwrap();
        let mut ch = channel::server(stream, &identity, &allowed).unwrap();
        while let Ok(frame) = ch.recv() {
            let req: Request = serde_json::from_slice(&frame).unwrap();
            ch.send(&serde_json::to_vec(&service.handle(&req)).unwrap())
                .unwrap();
        }
    });
    (addr, public, pin, handle)
}

fn vote(round: u64, author: u16) -> SignContext {
    SignContext {
        kind: SignKind::Vote,
        round,
        author,
    }
}

#[test]
fn a_remote_signature_verifies_under_the_nodes_unchanged_verifier() {
    let dir = tempfile::tempdir().unwrap();
    let node = Identity::from_seed(&seed(1));
    let (addr, public, pin, _signer) = spawn_signer(&dir, &node);
    let remote = RemoteSigner::connect(addr, node, pin, public.clone(), DEADLINE).unwrap();

    // The validator is member 1 of a two-key committee; the other key is
    // unrelated, so a signature under the wrong index must fail.
    let other = VerifyingKey::from_bytes(
        KeystoreBackend::new(&seed(9))
            .public_key()
            .try_into()
            .unwrap(),
    )
    .unwrap();
    let committee: Arc<[VerifyingKey]> = vec![other, public].into();
    let auth = MlDsaAuthenticator::validator(
        ValidatorKey::Remote(Arc::new(remote)),
        Arc::clone(&committee),
    );
    let observer = MlDsaAuthenticator::observer(committee);

    let digest = [5u8; 32];
    let sig = auth.sign(vote(3, 0), &digest);
    assert!(!sig.is_empty(), "the signer signed");
    assert!(
        observer.verify(1, &digest, &sig),
        "every validator accepts it"
    );
    assert!(
        !observer.verify(0, &digest, &sig),
        "under the right key only"
    );
    assert!(
        !observer.verify(1, &[6u8; 32], &sig),
        "for this digest only"
    );
}

#[test]
fn the_signers_refusal_is_a_missing_vote_and_other_authors_still_sign() {
    let dir = tempfile::tempdir().unwrap();
    let node = Identity::from_seed(&seed(1));
    let (addr, public, pin, _signer) = spawn_signer(&dir, &node);
    let committee: Arc<[VerifyingKey]> = vec![public.clone()].into();
    let remote = RemoteSigner::connect(addr, node, pin, public, DEADLINE).unwrap();
    let auth = MlDsaAuthenticator::validator(ValidatorKey::Remote(Arc::new(remote)), committee);

    assert!(!auth.sign(vote(4, 2), &[1; 32]).is_empty());
    // A conflicting vote for the same (round, author): refused, so empty.
    assert!(auth.sign(vote(4, 2), &[2; 32]).is_empty());
    // The refusal did not break the channel: another author, same round.
    assert!(!auth.sign(vote(4, 3), &[2; 32]).is_empty());
}

#[test]
fn a_signer_that_is_not_the_pinned_one_is_refused_at_startup() {
    let dir = tempfile::tempdir().unwrap();
    let node = Identity::from_seed(&seed(1));
    let (addr, public, _pin, _signer) = spawn_signer(&dir, &node);
    let wrong_pin = Identity::from_seed(&seed(4)).public_key().to_vec();
    assert!(RemoteSigner::connect(addr, node, wrong_pin, public, DEADLINE).is_err());
}

#[test]
fn a_signer_holding_another_key_costs_votes_not_garbage() {
    let dir = tempfile::tempdir().unwrap();
    let node = Identity::from_seed(&seed(1));
    let (addr, _public, pin, _signer) = spawn_signer(&dir, &node);
    // The node is told the validator key is one the signer does not hold.
    let claimed = VerifyingKey::from_bytes(
        KeystoreBackend::new(&seed(9))
            .public_key()
            .try_into()
            .unwrap(),
    )
    .unwrap();
    let remote = RemoteSigner::connect(addr, node, pin, claimed.clone(), DEADLINE).unwrap();
    let auth =
        MlDsaAuthenticator::validator(ValidatorKey::Remote(Arc::new(remote)), vec![claimed].into());
    assert!(auth.sign(vote(1, 0), &[9; 32]).is_empty());
}
