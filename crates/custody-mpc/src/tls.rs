//! Carrying the protocol over mutually-authenticated TLS.
//!
//! # What TLS is doing here, and what it is not
//!
//! It is not keeping the shares secret. Shares are sealed to their recipient
//! under ML-KEM-768 before they reach this module ([`crate::seal`]), so a
//! relay, a load balancer, or a compromised coordinator sees ciphertext. That
//! is deliberate: TLS protects a hop, and a share crosses more than one.
//!
//! What TLS does provide, and what nothing else here provides:
//!
//! - **Peer authentication, both ways.** [`server_config`] demands a client
//!   certificate. A custodian talks to the combiner it expects, and the
//!   combiner learns which custodian is on the other end before it accepts a
//!   frame. Without it, "custodian 3 contributed" means "somebody claimed to be
//!   custodian 3".
//! - **Traffic confidentiality.** Who is in a quorum, and when, is not something
//!   an institution wants on the wire in the clear even when the payloads are
//!   opaque.
//!
//! # `ring`, not `aws-lc-rs`
//!
//! Both this module's `rustls` and its `tokio-rustls` are pinned to the `ring`
//! provider with default features off. `aws-lc-rs` is the newer default and
//! needs a C toolchain; this workspace's `infra/docker/Dockerfile` builds a static musl
//! binary and `ring` is already in the tree through `jsonrpsee`. Selecting the
//! provider explicitly, rather than installing a process-global default, also
//! means a host application that made a different choice does not silently
//! change what a custody ceremony negotiates.

use std::io;
use std::sync::Arc;

use rustls::{ClientConfig, RootCertStore, ServerConfig};
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::transport::{Frame, MAX_FRAME_LEN};

/// Writes one length-prefixed frame.
///
/// # Errors
///
/// Whatever the underlying writer returns, or [`io::ErrorKind::InvalidInput`]
/// if the encoded frame exceeds [`MAX_FRAME_LEN`] — which a correct caller
/// cannot produce, and which would otherwise be a frame no reader accepts.
pub async fn write_frame<W>(writer: &mut W, frame: &Frame) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let body = frame.encode();
    if body.len() > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame over the size bound",
        ));
    }
    writer.write_all(&(body.len() as u32).to_be_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await
}

/// Reads one length-prefixed frame.
///
/// The length is checked against [`MAX_FRAME_LEN`] **before** the buffer is
/// allocated. A four-byte prefix that says four gigabytes is one packet; a
/// reader that believes it is an outage.
///
/// # Errors
///
/// Whatever the underlying reader returns, or [`io::ErrorKind::InvalidData`]
/// for an over-long or malformed frame.
pub async fn read_frame<R>(reader: &mut R) -> io::Result<Frame>
where
    R: AsyncRead + Unpin,
{
    let mut length = [0u8; 4];
    reader.read_exact(&mut length).await?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame over the size bound",
        ));
    }

    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await?;
    Frame::decode(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

/// The `ring` provider with its key exchange replaced by `X25519MLKEM768`
/// alone ([`crate::pq_kx`]).
///
/// Only the hybrid is offered: a peer that cannot do it fails the handshake
/// rather than falling back to a classical group a recording could later
/// break. That also makes TLS 1.3 the only version that can complete.
fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::CryptoProvider {
        kx_groups: vec![crate::pq_kx::X25519_MLKEM768],
        ..rustls::crypto::ring::default_provider()
    })
}

/// TLS 1.3 only: the hybrid group has no TLS 1.2 codepoint.
const VERSIONS: &[&rustls::SupportedProtocolVersion] = &[&rustls::version::TLS13];

/// A server configuration that **requires** a client certificate.
///
/// There is no variant of this function that makes client authentication
/// optional. A combiner that accepted anonymous connections would be a
/// combiner that cannot say who contributed, and "who contributed" is the
/// entire audit trail of a custody ceremony.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] if the certificate chain, the key, or the
/// trust roots are not usable.
pub fn server_config(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    custodian_roots: RootCertStore,
) -> io::Result<ServerConfig> {
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(custodian_roots),
        provider(),
    )
    .build()
    .map_err(invalid)?;

    ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(VERSIONS)
        .map_err(invalid)?
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)
        .map_err(invalid)
}

/// A client configuration that presents a custodian certificate.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] if the certificate chain or key is not
/// usable.
pub fn client_config(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    combiner_roots: RootCertStore,
) -> io::Result<ClientConfig> {
    ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(VERSIONS)
        .map_err(invalid)?
        .with_root_certificates(combiner_roots)
        .with_client_auth_cert(chain, key)
        .map_err(invalid)
}

/// Domain string for peer identities.
const IDENTITY_DOMAIN: &[u8] = b"maya2c.custody-mpc.tls-peer-identity.v1";

/// A stable identifier for the authenticated client on `stream`: a hash of its
/// end-entity certificate.
///
/// `None` only if the handshake carried no client certificate, which
/// [`server_config`] makes impossible — but the type says so, rather than a
/// caller assuming it.
#[must_use]
pub fn peer_identity<IO>(stream: &tokio_rustls::server::TlsStream<IO>) -> Option<[u8; 32]> {
    let leaf = stream.get_ref().1.peer_certificates()?.first()?;
    Some(identity_of(leaf))
}

/// The identity a custodian's certificate authenticates as.
///
/// For provisioning a [`CustodianDirectory`] from the certificates an
/// institution issued — the same function [`peer_identity`] applies to the
/// certificate a connection presents, so the two cannot disagree.
#[must_use]
pub fn identity_of(certificate: &CertificateDer<'_>) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(IDENTITY_DOMAIN);
    hasher.update(certificate.as_ref());
    *hasher.finalize().as_bytes()
}

/// Which certificate each custodian index belongs to.
///
/// # This check is the caller's, and it is not optional
///
/// A dealing says `dealer: 3`. Nothing in it proves that: shares are sealed to
/// their recipients, not signed by their dealers. So "custodian 3 dealt this"
/// is exactly as true as "the connection this arrived on is custodian 3", and
/// only the transport knows that. The directory is provisioned out of band —
/// the same place the certificates are issued — and every frame's claimed
/// index is checked against the connection it came from before the frame
/// reaches [`crate::dkg`] or [`crate::session`].
///
/// Skipping it does not leak a key. It lets one authenticated custodian squat
/// another's slot and have the real custodian's dealing refused as a duplicate,
/// which breaks the one operational promise of every error in this crate:
/// that it names who to call.
#[derive(Clone, Debug, Default)]
pub struct CustodianDirectory {
    identities: std::collections::BTreeMap<u8, [u8; 32]>,
}

impl CustodianDirectory {
    /// An empty directory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that custodian `index` authenticates as `identity`.
    pub fn insert(&mut self, index: u8, identity: [u8; 32]) {
        self.identities.insert(index, identity);
    }

    /// Refuses a frame claiming `claimed` unless the connection is that
    /// custodian.
    ///
    /// # Errors
    ///
    /// [`crate::CustodyError::ImpersonatedCustodian`] if the index is unknown
    /// or belongs to a different certificate.
    pub fn authorize(&self, claimed: u8, peer: Option<[u8; 32]>) -> crate::Result<()> {
        match (self.identities.get(&claimed), peer) {
            (Some(expected), Some(actual)) if *expected == actual => Ok(()),
            _ => Err(crate::CustodyError::ImpersonatedCustodian { claimed }),
        }
    }
}

fn invalid<E: core::fmt::Display>(error: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, error.to_string())
}
