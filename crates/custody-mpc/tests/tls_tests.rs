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

use std::sync::Arc;

use maya_custody_mpc::dkg::{Custodian, CustodianShare, Dealing, Roster, VaultPolicy};
use maya_custody_mpc::error::CustodyError;
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
use rustls::RootCertStore;
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

    let anonymous = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
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
