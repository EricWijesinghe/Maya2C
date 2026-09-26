//! A 3-of-5 signing ceremony carried over real mutually-authenticated TLS.
//!
//! # Why this test exists in this form
//!
//! Everything else in this crate's suite is arithmetic and codecs, which are
//! the same on a loopback socket as they are in a data centre. This one is here
//! for the parts that are not: that a rustls configuration built by
//! [`maya_custody_mpc::tls`] actually completes a handshake, that it *refuses*
//! to complete one without a client certificate, and that a 1,153-byte sealed
//! share survives the framing under real reads and writes.
//!
//! A protocol whose TLS binding is written but never run is a protocol whose TLS
//! binding does not work. This repository already ships things that were written
//! and could not be exercised here — every Speculos test in
//! `apps/ledger-maya2c/tests/ledger_tests.rs` is `#[ignore]`d, and `docs/dag-pow.md`
//! says outright that its CUDA numbers were never measured because this tree has
//! no GPU. Saying so at the top of the file is the right way to ship an unrun
//! artifact. This one is not one of them: it runs.
//!
//! # The certificates are generated here, per test
//!
//! `rcgen` is a dev-dependency. Nothing shipped generates a certificate: an
//! institution brings its own PKI, and a library that minted custody
//! certificates would be a library that decided who a custodian is.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use maya_crypto_pq::suite::MasterSeed;
use maya_custody_mpc::dkg::{Custodian, CustodianShare, Dealing, Roster, VaultPolicy};
use maya_custody_mpc::error::CustodyError;
use maya_custody_mpc::pq_auth::{
    PqIdentity, Role, authenticate, authenticate_within, client_exporter, identity_of_key,
    server_exporter,
};
use maya_custody_mpc::pq_kx::X25519_MLKEM768;
use maya_custody_mpc::session::{SigningSession, VaultDescriptor, respond};
use maya_custody_mpc::tls::{
    CustodianDirectory, client_config, identity_of, peer_identity, read_frame, server_config,
    write_frame,
};
use maya_custody_mpc::transport::Frame;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::{NamedGroup, RootCertStore};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// A certificate authority and the two leaves it issues.
struct Pki {
    root: CertificateDer<'static>,
    server_chain: Vec<CertificateDer<'static>>,
    server_key: PrivateKeyDer<'static>,
    client_chain: Vec<CertificateDer<'static>>,
    client_key: PrivateKeyDer<'static>,
    /// A second custodian's certificate, from the same CA.
    other_chain: Vec<CertificateDer<'static>>,
    other_key: PrivateKeyDer<'static>,
}

fn pki() -> Pki {
    let ca_key = KeyPair::generate().expect("ca key");
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("ca params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "maya2c custody test ca");
    let ca_cert = ca_params.self_signed(&ca_key).expect("ca cert");
    let issuer = Issuer::new(ca_params, ca_key);

    let leaf = |usage: ExtendedKeyUsagePurpose, name: &str| {
        let key = KeyPair::generate().expect("leaf key");
        let mut params = CertificateParams::new(vec!["localhost".to_string()]).expect("params");
        params.extended_key_usages = vec![usage];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.distinguished_name.push(DnType::CommonName, name);
        let cert = params.signed_by(&key, &issuer).expect("leaf cert");
        let der = PrivateKeyDer::try_from(key.serialize_der()).expect("pkcs8");
        (vec![cert.der().clone()], der)
    };

    let (server_chain, server_key) = leaf(ExtendedKeyUsagePurpose::ServerAuth, "combiner");
    let (client_chain, client_key) = leaf(ExtendedKeyUsagePurpose::ClientAuth, "custodian");
    let (other_chain, other_key) = leaf(ExtendedKeyUsagePurpose::ClientAuth, "custodian-2");

    Pki {
        root: ca_cert.der().clone(),
        server_chain,
        server_key,
        client_chain,
        client_key,
        other_chain,
        other_key,
    }
}

fn roots(certificate: &CertificateDer<'static>) -> RootCertStore {
    let mut store = RootCertStore::empty();
    store.add(certificate.clone()).expect("root");
    store
}

/// A finished 3-of-5 vault, so the network tests can get to the point.
fn vault() -> (VaultDescriptor, Vec<CustodianShare>) {
    let policy = VaultPolicy::new(3, 5).expect("policy");
    let members: Vec<Custodian> = (1..=5)
        .map(|i| Custodian::begin(policy, i).expect("begin"))
        .collect();
    let announcements: Vec<_> = members.iter().map(Custodian::announce).collect();
    let roster = Roster::assemble(policy, &announcements).expect("roster");
    let dealings: Vec<Dealing> = members
        .iter()
        .map(|c| c.deal(&roster).expect("deal"))
        .collect();
    let held: Vec<CustodianShare> = members
        .iter()
        .map(|c| c.accept(&roster, &dealings).expect("accept"))
        .collect();
    let descriptor = VaultDescriptor::establish(&held[..3]).expect("establish");
    (descriptor, held)
}

#[tokio::test]
async fn a_quorum_signs_over_mutual_tls() {
    let pki = pki();
    let (descriptor, held) = vault();

    let acceptor = TlsAcceptor::from(Arc::new(
        server_config(
            pki.server_chain.clone(),
            pki.server_key.clone_key(),
            roots(&pki.root),
        )
        .expect("server config"),
    ));
    let connector = TlsConnector::from(Arc::new(
        client_config(
            pki.client_chain.clone(),
            pki.client_key.clone_key(),
            roots(&pki.root),
        )
        .expect("client config"),
    ));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr");

    // The combiner. It opens one session, hands out the request, and folds in
    // whatever comes back until it has a quorum.
    let mut session = SigningSession::open(descriptor.clone(), b"pay alice 10".to_vec());
    let request = session.request();

    let combiner = tokio::spawn(async move {
        for _ in 0..3 {
            let (socket, _) = listener.accept().await.expect("accept");
            let mut stream = acceptor.accept(socket).await.expect("handshake");
            // The hop is post-quantum: the only group either side offers.
            assert_eq!(
                stream
                    .get_ref()
                    .1
                    .negotiated_key_exchange_group()
                    .map(rustls::crypto::SupportedKxGroup::name),
                Some(NamedGroup::X25519MLKEM768)
            );
            write_frame(&mut stream, &Frame::Request(Box::new(request.clone())))
                .await
                .expect("write request");
            let Frame::Contribution(sealed) = read_frame(&mut stream).await.expect("read") else {
                panic!("a custodian answered with something other than a contribution");
            };
            session.accept_sealed(&sealed).expect("accept");
        }
        session
    });

    // Three custodians dial in. Custodians 2 and 4 never do -- they are the
    // simulated outage, and a 3-of-5 vault does not care.
    for index in [0usize, 2, 4] {
        let stream = TcpStream::connect(address).await.expect("connect");
        let name = ServerName::try_from("localhost").expect("name");
        let mut stream = connector
            .connect(name, stream)
            .await
            .expect("client handshake");

        let Frame::Request(request) = read_frame(&mut stream).await.expect("read request") else {
            panic!("the combiner sent something other than a request");
        };
        let sealed = respond(&request, &held[index]).expect("respond");
        write_frame(&mut stream, &Frame::Contribution(sealed))
            .await
            .expect("write contribution");
    }

    let session = combiner.await.expect("combiner");
    assert_eq!(session.contributors().len(), 3);

    let signature = session.sign().expect("sign");

    // The same signature the in-process path produces. If the transport had
    // altered a single byte of a share, the commitment check would have refused
    // the reconstruction -- but this asserts the stronger thing: it did not.
    let mut local = SigningSession::open(descriptor, b"pay alice 10".to_vec());
    for index in [0usize, 2, 4] {
        local.contribute(&held[index]).expect("contribute");
    }
    assert_eq!(signature, local.sign().expect("local"));
}

#[tokio::test]
async fn a_client_without_a_certificate_is_refused() {
    // The property the combiner's audit trail rests on. Without client
    // authentication, "custodian 3 contributed" means "somebody claimed to be
    // custodian 3", and `server_config` offers no way to turn it off.
    let pki = pki();

    let acceptor = TlsAcceptor::from(Arc::new(
        server_config(pki.server_chain, pki.server_key, roots(&pki.root)).expect("server config"),
    ));

    // The hybrid group, so the only thing this client lacks is a certificate.
    let anonymous =
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::CryptoProvider {
            kx_groups: vec![X25519_MLKEM768],
            ..rustls::crypto::ring::default_provider()
        }))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("versions")
        .with_root_certificates(roots(&pki.root))
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(anonymous));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr");

    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept");
        acceptor.accept(socket).await.map(|_| ())
    });

    let stream = TcpStream::connect(address).await.expect("connect");
    let name = ServerName::try_from("localhost").expect("name");
    // One side or the other reports it depending on which notices first; what
    // matters is that no session is established.
    let client = connector.connect(name, stream).await;

    assert!(
        server.await.expect("join").is_err() || client.is_err(),
        "an anonymous client must not complete a handshake with the combiner"
    );
}

#[tokio::test]
async fn a_contribution_claiming_another_custodians_index_is_refused() {
    // Nothing in a sealed contribution proves who sent it, so the combiner
    // checks the claimed index against the certificate the connection
    // authenticated with. Custodian 1 is accepted as custodian 1; custodian 2,
    // presenting its own valid certificate but claiming to be custodian 1, is
    // refused -- rather than taking custodian 1's slot and having the real one
    // rejected as a duplicate.
    let pki = pki();
    let (descriptor, held) = vault();

    let mut directory = CustodianDirectory::new();
    directory.insert(1, identity_of(&pki.client_chain[0]));
    directory.insert(2, identity_of(&pki.other_chain[0]));

    let acceptor = TlsAcceptor::from(Arc::new(
        server_config(pki.server_chain, pki.server_key, roots(&pki.root)).expect("server config"),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr");

    let session = SigningSession::open(descriptor, b"tx".to_vec());
    let request = session.request();
    let combiner = tokio::spawn(async move {
        let mut verdicts = Vec::new();
        for _ in 0..2 {
            let (socket, _) = listener.accept().await.expect("accept");
            let mut stream = acceptor.accept(socket).await.expect("handshake");
            let peer = peer_identity(&stream);
            write_frame(&mut stream, &Frame::Request(Box::new(request.clone())))
                .await
                .expect("write request");
            let Frame::Contribution(sealed) = read_frame(&mut stream).await.expect("read") else {
                panic!("expected a contribution");
            };
            verdicts.push(directory.authorize(sealed.dealer, peer));
        }
        verdicts
    });

    // The genuine custodian 1, then custodian 2 relabelling its response as 1.
    for (chain, key, share, claim) in [
        (
            pki.client_chain.clone(),
            pki.client_key.clone_key(),
            &held[0],
            1u8,
        ),
        (
            pki.other_chain.clone(),
            pki.other_key.clone_key(),
            &held[1],
            1u8,
        ),
    ] {
        let connector = TlsConnector::from(Arc::new(
            client_config(chain, key, roots(&pki.root)).expect("client config"),
        ));
        let stream = TcpStream::connect(address).await.expect("connect");
        let name = ServerName::try_from("localhost").expect("name");
        let mut stream = connector.connect(name, stream).await.expect("handshake");
        let Frame::Request(request) = read_frame(&mut stream).await.expect("read") else {
            panic!("expected a request");
        };
        let mut sealed = respond(&request, share).expect("respond");
        sealed.dealer = claim;
        write_frame(&mut stream, &Frame::Contribution(sealed))
            .await
            .expect("write");
    }

    let verdicts = combiner.await.expect("combiner");
    assert_eq!(verdicts[0], Ok(()), "custodian 1 is custodian 1");
    assert_eq!(
        verdicts[1],
        Err(CustodyError::ImpersonatedCustodian { claimed: 1 }),
        "custodian 2 is not custodian 1, whatever its frame says"
    );
}

#[test]
fn an_index_missing_from_the_directory_is_refused() {
    let directory = CustodianDirectory::new();
    assert_eq!(
        directory.authorize(3, Some([7; 32])),
        Err(CustodyError::ImpersonatedCustodian { claimed: 3 })
    );
}

#[tokio::test]
async fn a_classical_only_custodian_cannot_connect() {
    // A custodian with a valid certificate but only the classical groups:
    // the combiner offers nothing it can agree on, so there is no fallback to
    // a key exchange a recording could later break.
    let pki = pki();
    let acceptor = TlsAcceptor::from(Arc::new(
        server_config(pki.server_chain, pki.server_key, roots(&pki.root)).expect("server config"),
    ));
    let classical = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("versions")
    .with_root_certificates(roots(&pki.root))
    .with_client_auth_cert(pki.client_chain, pki.client_key)
    .expect("client auth");
    let connector = TlsConnector::from(Arc::new(classical));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept");
        acceptor.accept(socket).await.map(|_| ())
    });
    let stream = TcpStream::connect(address).await.expect("connect");
    let name = ServerName::try_from("localhost").expect("name");
    let client = connector.connect(name, stream).await;

    assert!(server.await.expect("join").is_err(), "the combiner refuses");
    assert!(client.is_err(), "and the custodian learns so");
}

/// How one custodian behaves in [`ceremony_with_faults`].
#[derive(Clone, Copy)]
enum Behaviour {
    /// Answers correctly.
    Honest,
    /// Completes the handshake, reads the request, and dies.
    Crashes,
    /// Answers with a contribution corrupted in transit.
    Corrupts,
    /// Holds a valid TLS certificate from the institution's CA, but an ML-DSA
    /// key the combiner has never been told about.
    Impostor,
}

fn pq(seed: u8) -> PqIdentity {
    PqIdentity::from_seed(&MasterSeed::from_bytes([seed; 32]))
}

/// Custodian `index`'s long-term key (0-based, as `held` is).
fn custodian_pq(index: usize) -> PqIdentity {
    pq(0x30 + u8::try_from(index).expect("small"))
}

/// Runs one session over TLS with five custodians behaving as told, and
/// returns what the combiner ended with.
///
/// Every connection is authenticated post-quantum ([`maya_custody_mpc::pq_auth`])
/// after the handshake, and every contribution is checked against the
/// directory by the ML-DSA identity that authenticated -- not by the classical
/// certificate -- so what signs is what a quantum adversary could not fake.
async fn ceremony_with_faults(behaviours: [Behaviour; 5]) -> SigningSession {
    let pki = pki();
    let (descriptor, held) = vault();
    let combiner_pq = pq(0x2f);
    let combiner_pk = combiner_pq.public_key();
    let mut directory = CustodianDirectory::new();
    let mut roster_keys = Vec::new();
    for index in 0..5usize {
        let key = custodian_pq(index).public_key();
        directory.insert(
            u8::try_from(index + 1).expect("small"),
            identity_of_key(&key),
        );
        roster_keys.push(key);
    }
    let acceptor = TlsAcceptor::from(Arc::new(
        server_config(pki.server_chain, pki.server_key, roots(&pki.root)).expect("server config"),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr");

    let mut session = SigningSession::open(descriptor, b"pay bob 7".to_vec());
    let request = session.request();
    let combiner = tokio::spawn(async move {
        for _ in 0..5 {
            let (socket, _) = listener.accept().await.expect("accept");
            let mut stream = acceptor.accept(socket).await.expect("handshake");
            let exporter = server_exporter(&stream).expect("exporter");
            // A peer that fails post-quantum authentication never sees the
            // request: the connection ends here.
            let Ok(peer) = authenticate(
                &mut stream,
                &exporter,
                Role::Responder,
                &combiner_pq,
                |pk| roster_keys.iter().any(|known| known.as_slice() == pk),
            )
            .await
            else {
                continue;
            };
            write_frame(&mut stream, &Frame::Request(Box::new(request.clone())))
                .await
                .expect("write request");
            // A dead peer and a refused contribution are both survivable:
            // the combiner notes nothing and waits for the next custodian.
            if let Ok(Frame::Contribution(sealed)) = read_frame(&mut stream).await
                && directory.authorize(sealed.dealer, Some(peer)).is_ok()
            {
                let _refused = session.accept_sealed(&sealed);
            }
        }
        session
    });

    for (index, behaviour) in behaviours.into_iter().enumerate() {
        let connector = TlsConnector::from(Arc::new(
            client_config(
                pki.client_chain.clone(),
                pki.client_key.clone_key(),
                roots(&pki.root),
            )
            .expect("client config"),
        ));
        let stream = TcpStream::connect(address).await.expect("connect");
        let name = ServerName::try_from("localhost").expect("name");
        let mut stream = connector.connect(name, stream).await.expect("handshake");
        let exporter = client_exporter(&stream).expect("exporter");
        let me = match behaviour {
            Behaviour::Impostor => pq(0xee),
            _ => custodian_pq(index),
        };
        let authenticated = authenticate(&mut stream, &exporter, Role::Initiator, &me, |pk| {
            pk == combiner_pk.as_slice()
        })
        .await;
        if matches!(behaviour, Behaviour::Impostor) {
            assert!(authenticated.is_err(), "an unknown ML-DSA key is refused");
            continue;
        }
        assert_eq!(authenticated, Ok(identity_of_key(&combiner_pk)));
        let Frame::Request(request) = read_frame(&mut stream).await.expect("read") else {
            panic!("expected a request");
        };
        match behaviour {
            Behaviour::Impostor => unreachable!("handled above"),
            Behaviour::Crashes => drop(stream),
            Behaviour::Honest | Behaviour::Corrupts => {
                let mut sealed = respond(&request, &held[index]).expect("respond");
                if matches!(behaviour, Behaviour::Corrupts) {
                    let middle = sealed.body.len() / 2;
                    sealed.body[middle] ^= 0x01;
                }
                write_frame(&mut stream, &Frame::Contribution(sealed))
                    .await
                    .expect("write");
            }
        }
    }
    combiner.await.expect("combiner")
}

#[tokio::test]
async fn three_of_five_signs_through_a_crash_and_a_corrupted_share() {
    use Behaviour::{Corrupts, Crashes, Honest};
    let session = ceremony_with_faults([Crashes, Corrupts, Honest, Honest, Honest]).await;
    assert_eq!(
        session.contributors().len(),
        3,
        "only the three honest count"
    );
    session
        .sign()
        .expect("three honest custodians are a quorum");
}

#[tokio::test]
async fn three_failures_leave_a_three_of_five_vault_unable_to_sign() {
    use Behaviour::{Corrupts, Crashes, Honest};
    let session = ceremony_with_faults([Crashes, Corrupts, Crashes, Honest, Honest]).await;
    assert_eq!(session.contributors().len(), 2);
    assert!(matches!(
        session.sign(),
        Err(CustodyError::ShortOfThreshold {
            received: 2,
            threshold: 3
        })
    ));
}

#[tokio::test]
async fn three_of_five_signs_with_an_impostor_and_a_crash_under_pq_authentication() {
    // Custodian 2's slot is taken by somebody holding a valid certificate from
    // the institution's own CA -- exactly what a forged classical signature
    // would give an attacker -- but no ML-DSA key the combiner knows.
    use Behaviour::{Crashes, Honest, Impostor};
    let session = ceremony_with_faults([Honest, Impostor, Crashes, Honest, Honest]).await;
    assert_eq!(session.contributors().len(), 3);
    session
        .sign()
        .expect("three authenticated custodians are a quorum");
}

// ---------------------------------------------------------------------------
// pq_auth, one connection at a time
// ---------------------------------------------------------------------------

/// One TLS connection. The server authenticates as `server`, pinning
/// `client_pin`; the client authenticates as `client`, pinning `server_pin`,
/// over `forged_exporter` if given and the session's own otherwise.
async fn pq_connection(
    server: PqIdentity,
    client_pin: Vec<u8>,
    client: PqIdentity,
    server_pin: Vec<u8>,
    forged_exporter: Option<[u8; 32]>,
) -> (
    maya_custody_mpc::Result<[u8; 32]>,
    maya_custody_mpc::Result<[u8; 32]>,
) {
    let pki = pki();
    let acceptor = TlsAcceptor::from(Arc::new(
        server_config(pki.server_chain, pki.server_key, roots(&pki.root)).expect("server config"),
    ));
    let connector = TlsConnector::from(Arc::new(
        client_config(pki.client_chain, pki.client_key, roots(&pki.root)).expect("client config"),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr");

    let responder = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept");
        let mut stream = acceptor.accept(socket).await.expect("handshake");
        let exporter = server_exporter(&stream).expect("exporter");
        authenticate(&mut stream, &exporter, Role::Responder, &server, |pk| {
            pk == client_pin.as_slice()
        })
        .await
    });

    let socket = TcpStream::connect(address).await.expect("connect");
    let name = ServerName::try_from("localhost").expect("name");
    let mut stream = connector.connect(name, socket).await.expect("handshake");
    let exporter = forged_exporter.unwrap_or_else(|| client_exporter(&stream).expect("exporter"));
    let initiated = authenticate(&mut stream, &exporter, Role::Initiator, &client, |pk| {
        pk == server_pin.as_slice()
    })
    .await;
    drop(stream);
    (initiated, responder.await.expect("responder"))
}

#[tokio::test]
async fn both_ends_authenticate_with_ml_dsa_bound_to_the_session() {
    let (server, client) = (pq(1), pq(2));
    let (server_pk, client_pk) = (server.public_key(), client.public_key());
    let (initiated, responded) =
        pq_connection(server, client_pk.clone(), client, server_pk.clone(), None).await;
    assert_eq!(initiated, Ok(identity_of_key(&server_pk)));
    assert_eq!(responded, Ok(identity_of_key(&client_pk)));
}

#[tokio::test]
async fn a_server_with_an_unexpected_ml_dsa_key_is_refused() {
    // The TLS certificate is valid; the post-quantum key is not the pinned one.
    let (server, client) = (pq(3), pq(4));
    let client_pk = client.public_key();
    let pinned_elsewhere = pq(5).public_key();
    let (initiated, _) = pq_connection(server, client_pk, client, pinned_elsewhere, None).await;
    assert!(matches!(
        initiated,
        Err(CustodyError::PeerAuthentication(_))
    ));
}

#[tokio::test]
async fn a_proof_made_for_another_session_is_refused() {
    // The honest client's key, signing an exporter that is not this session's:
    // what a relay or a replay of a captured proof would present.
    let (server, client) = (pq(6), pq(7));
    let (server_pk, client_pk) = (server.public_key(), client.public_key());
    let (_, responded) =
        pq_connection(server, client_pk, client, server_pk, Some([0x55; 32])).await;
    assert_eq!(
        responded,
        Err(CustodyError::PeerAuthentication(
            "the peer's signature does not cover this session"
        ))
    );
}

/// A server that completes the TLS handshake and then runs `behave` on the raw
/// stream instead of authenticating; the client authenticates as `client`,
/// pinning `server_pin`, under `deadline`.
async fn against_a_misbehaving_server<B, Fut>(
    client: PqIdentity,
    server_pin: Vec<u8>,
    deadline: std::time::Duration,
    behave: B,
) -> maya_custody_mpc::Result<[u8; 32]>
where
    B: FnOnce(tokio_rustls::server::TlsStream<TcpStream>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let pki = pki();
    let acceptor = TlsAcceptor::from(Arc::new(
        server_config(pki.server_chain, pki.server_key, roots(&pki.root)).expect("server config"),
    ));
    let connector = TlsConnector::from(Arc::new(
        client_config(pki.client_chain, pki.client_key, roots(&pki.root)).expect("client config"),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept");
        behave(acceptor.accept(socket).await.expect("handshake")).await;
    });
    let socket = TcpStream::connect(address).await.expect("connect");
    let name = ServerName::try_from("localhost").expect("name");
    let mut stream = connector.connect(name, socket).await.expect("handshake");
    let exporter = client_exporter(&stream).expect("exporter");
    let result = authenticate_within(
        deadline,
        &mut stream,
        &exporter,
        Role::Initiator,
        &client,
        |pk| pk == server_pin.as_slice(),
    )
    .await;
    server.abort();
    result
}

#[tokio::test]
async fn a_proof_reflected_back_to_its_sender_is_refused() {
    // The server echoes the client's own proof. Pin the client's own key as
    // the "server", so the only thing standing between the echo and success
    // is the signed role byte.
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let client = pq(8);
    let own_key = client.public_key();
    let info = maya_crypto_pq::suite::SuiteId::MlDsa87.info();
    let result = against_a_misbehaving_server(
        client,
        own_key,
        std::time::Duration::from_secs(10),
        move |mut stream| async move {
            let mut proof = vec![0u8; info.public_key_len + info.signature_len];
            stream
                .read_exact(&mut proof)
                .await
                .expect("read the client's proof");
            stream.write_all(&proof).await.expect("echo it");
            stream.flush().await.expect("flush");
            // Keep the connection open until the client has judged the echo.
            let mut rest = [0u8; 1];
            let _eof = stream.read(&mut rest).await;
        },
    )
    .await;
    assert_eq!(
        result,
        Err(CustodyError::PeerAuthentication(
            "the peer's signature does not cover this session"
        ))
    );
}

#[tokio::test]
async fn a_peer_that_goes_silent_is_timed_out() {
    // Valid certificate, completed handshake, then nothing: without a
    // deadline this would hold a sequential combiner for ever.
    use tokio::io::AsyncReadExt as _;
    let started = std::time::Instant::now();
    let result = against_a_misbehaving_server(
        pq(9),
        pq(10).public_key(),
        std::time::Duration::from_millis(300),
        |mut stream| async move {
            let mut sink = vec![0u8; 16 * 1024];
            loop {
                if matches!(stream.read(&mut sink).await, Ok(0) | Err(_)) {
                    break;
                }
            }
        },
    )
    .await;
    assert_eq!(
        result,
        Err(CustodyError::PeerAuthentication(
            "the peer did not complete authentication in time"
        ))
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}
