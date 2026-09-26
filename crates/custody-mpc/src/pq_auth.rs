//! Post-quantum peer authentication, bound to one TLS session.
//!
//! # The gap this closes
//!
//! [`crate::tls`] negotiates `X25519MLKEM768`, so a recording of a ceremony
//! stays confidential against a future quantum computer. Its *authentication*
//! does not: both sides prove who they are with X.509 certificates, and rustls
//! (like every mainstream TLS stack today) signs and verifies those with
//! classical ECDSA, Ed25519 or RSA. An adversary who can forge one of those
//! signatures can stand in the middle of a live ceremony. Master Prompt 2 §7
//! asks for shares to travel over *mutually authenticated PQ* TLS, and the
//! certificate half cannot give that.
//!
//! # How: sign the session, not the certificate
//!
//! Once the handshake completes, each side signs the session's TLS exporter
//! (RFC 8446 §7.5, the value RFC 9266 calls `tls-exporter`) with a long-term
//! ML-DSA-87 key and sends `public_key ‖ signature`. The other side checks the
//! key is one it expects, then the signature.
//!
//! Why that is enough:
//!
//! - **The exporter is unique to this session.** It is derived from the
//!   handshake's shared secret, which includes ML-KEM-768. A man in the middle
//!   runs two sessions, one with each victim, with two different exporters — so
//!   the honest peer's signature is over the wrong one for the other side, and
//!   the attacker cannot produce the right one without the ML-DSA key.
//! - **Replay fails for the same reason.** A signature captured from session A
//!   says nothing about session B.
//! - **Reflection fails because the role is signed.** An initiator's signature
//!   covers the initiator byte, so it cannot be bounced back as the
//!   responder's.
//!
//! The certificate still does what it did (routing, revocation, the PKI an
//! institution already runs); this adds the part that holds against a quantum
//! adversary. [`identity_of_key`] is what a [`crate::tls::CustodianDirectory`]
//! should be provisioned with, so "custodian 3 contributed" rests on the
//! ML-DSA key, not the classical certificate.
//!
//! # Order, and why it is fixed
//!
//! The initiator (the TLS client) writes first and the responder reads first.
//! Each message is ~7.2 KB. If both sides wrote before reading, a transport
//! whose buffer is smaller than that would deadlock with both writers blocked.
//!
//! # A deadline, always
//!
//! The whole exchange runs under [`AUTH_TIMEOUT`] ([`authenticate_within`] to
//! choose another). A peer with a valid certificate that completes the
//! handshake and then sends nothing -- or one byte short -- would otherwise hold
//! `read_exact` for ever, and a combiner that serves custodians one at a time
//! would serve nobody else.
//!
//! # Call it on every connection
//!
//! Including a resumed TLS session. Resumption in TLS 1.3 re-runs the key
//! exchange, so the exporter is fresh, but a resumed socket is still a new
//! stream: nothing here remembers that a peer authenticated before, and nothing
//! should. A caller that cached "authenticated" by session id would be trusting
//! a connection no ML-DSA signature covers.

use core::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use maya_crypto_pq::suite::{MasterSeed, MlDsa87, SignatureSuite, SuiteId};

use crate::error::{CustodyError, Result};

/// The RFC 8446 exporter label. Distinct from every other use of the exporter,
/// so this value is never also a key somebody else derived.
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-maya2c-custody-pq-peer-auth-v1";

/// Domain for the signed transcript.
const TRANSCRIPT_DOMAIN: &[u8] = b"maya2c.custody-mpc.pq-peer-auth.transcript.v1";

/// Domain for the identity a key authenticates as.
const IDENTITY_DOMAIN: &str = "maya2c 2026-09-27 custody pq peer identity v1";

/// Length of the exporter value both sides sign.
pub const EXPORTER_LEN: usize = 32;

/// How long [`authenticate`] waits for the whole exchange. Two ML-DSA-87
/// signatures and ~14 KB each way take milliseconds on a LAN; ten seconds
/// leaves room for a slow WAN link and none for a peer that has stopped.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// The suite peers authenticate with: NIST category 5, the node's mainnet
/// default. Fixed, not negotiated — a negotiable suite is a downgrade.
pub const SUITE: SuiteId = SuiteId::MlDsa87;

/// Which end of the connection this is. The TLS client initiates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The TLS client: writes its proof first.
    Initiator,
    /// The TLS server: reads the initiator's proof first.
    Responder,
}

impl Role {
    fn byte(self) -> u8 {
        match self {
            Self::Initiator => 0x01,
            Self::Responder => 0x02,
        }
    }

    fn peer(self) -> Self {
        match self {
            Self::Initiator => Self::Responder,
            Self::Responder => Self::Initiator,
        }
    }
}

/// A long-term ML-DSA-87 identity. The key zeroizes on drop and never prints.
pub struct PqIdentity {
    key: <MlDsa87 as SignatureSuite>::SigningKey,
}

impl PqIdentity {
    /// The identity a 32-byte seed determines.
    #[must_use]
    pub fn from_seed(seed: &MasterSeed) -> Self {
        Self {
            key: MlDsa87::signing_key_from_seed(seed),
        }
    }

    /// The public key a peer pins.
    #[must_use]
    pub fn public_key(&self) -> Vec<u8> {
        MlDsa87::public_key(&self.key)
    }
}

impl core::fmt::Debug for PqIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PqIdentity")
            .field("key", &"<redacted>")
            .finish()
    }
}

/// The 32-byte identity an ML-DSA-87 public key authenticates as.
#[must_use]
pub fn identity_of_key(public_key: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(IDENTITY_DOMAIN);
    hasher.update(public_key);
    *hasher.finalize().as_bytes()
}

/// What `role`'s signature covers: the role, the signer's own identity, and
/// the session.
///
/// The exporter alone already makes a proof unusable in another session. The
/// signer's identity is inside too, as defence in depth: if an operator's
/// proxy ever handed one exporter to two backends, a proof would still name
/// the one key it was made with.
fn transcript(role: Role, signer_public_key: &[u8], exporter: &[u8; EXPORTER_LEN]) -> Vec<u8> {
    let mut out = Vec::with_capacity(TRANSCRIPT_DOMAIN.len() + 1 + 32 + EXPORTER_LEN);
    out.extend_from_slice(TRANSCRIPT_DOMAIN);
    out.push(role.byte());
    out.extend_from_slice(&identity_of_key(signer_public_key));
    out.extend_from_slice(exporter);
    out
}

/// The exporter of a client-side TLS session.
///
/// # Errors
///
/// [`CustodyError::PeerAuthentication`] if the session cannot export (a
/// handshake that has not completed).
pub fn client_exporter<IO>(
    stream: &tokio_rustls::client::TlsStream<IO>,
) -> Result<[u8; EXPORTER_LEN]> {
    stream
        .get_ref()
        .1
        .export_keying_material([0u8; EXPORTER_LEN], EXPORTER_LABEL, None)
        .map_err(|_| CustodyError::PeerAuthentication("the TLS session cannot export"))
}

/// The exporter of a server-side TLS session.
///
/// # Errors
///
/// As [`client_exporter`].
pub fn server_exporter<IO>(
    stream: &tokio_rustls::server::TlsStream<IO>,
) -> Result<[u8; EXPORTER_LEN]> {
    stream
        .get_ref()
        .1
        .export_keying_material([0u8; EXPORTER_LEN], EXPORTER_LABEL, None)
        .map_err(|_| CustodyError::PeerAuthentication("the TLS session cannot export"))
}

/// Runs the exchange on `stream` under [`AUTH_TIMEOUT`] and returns the
/// peer's identity.
///
/// `accept` decides whether a public key belongs to a peer this side will talk
/// to at all; it runs *before* the signature is verified, so an unknown key
/// costs one hash rather than an ML-DSA-87 verification.
///
/// # Errors
///
/// [`CustodyError::PeerAuthentication`] if the peer's key is not accepted, its
/// signature does not verify over this session's exporter, the stream fails,
/// or the exchange does not finish in time.
pub async fn authenticate<S, F>(
    stream: &mut S,
    exporter: &[u8; EXPORTER_LEN],
    role: Role,
    me: &PqIdentity,
    accept: F,
) -> Result<[u8; 32]>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: Fn(&[u8]) -> bool,
{
    authenticate_within(AUTH_TIMEOUT, stream, exporter, role, me, accept).await
}

/// [`authenticate`] with a deadline of the caller's choosing.
///
/// # Errors
///
/// As [`authenticate`].
pub async fn authenticate_within<S, F>(
    deadline: Duration,
    stream: &mut S,
    exporter: &[u8; EXPORTER_LEN],
    role: Role,
    me: &PqIdentity,
    accept: F,
) -> Result<[u8; 32]>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: Fn(&[u8]) -> bool,
{
    tokio::time::timeout(deadline, exchange(stream, exporter, role, me, &accept))
        .await
        .map_err(|_| {
            CustodyError::PeerAuthentication("the peer did not complete authentication in time")
        })?
}

async fn exchange<S, F>(
    stream: &mut S,
    exporter: &[u8; EXPORTER_LEN],
    role: Role,
    me: &PqIdentity,
    accept: &F,
) -> Result<[u8; 32]>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: Fn(&[u8]) -> bool,
{
    match role {
        Role::Initiator => {
            send(stream, exporter, role, me).await?;
            receive(stream, exporter, role.peer(), accept).await
        }
        Role::Responder => {
            let peer = receive(stream, exporter, role.peer(), accept).await?;
            send(stream, exporter, role, me).await?;
            Ok(peer)
        }
    }
}

async fn send<S>(
    stream: &mut S,
    exporter: &[u8; EXPORTER_LEN],
    role: Role,
    me: &PqIdentity,
) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let mut message = me.public_key();
    let signature = MlDsa87::sign(&me.key, &transcript(role, &message, exporter))
        .map_err(|_| CustodyError::PeerAuthentication("signing failed"))?;
    message.extend_from_slice(&signature);
    stream
        .write_all(&message)
        .await
        .map_err(|_| CustodyError::PeerAuthentication("the stream failed while sending"))?;
    stream
        .flush()
        .await
        .map_err(|_| CustodyError::PeerAuthentication("the stream failed while sending"))
}

/// Reads exactly one proof. Both lengths are the registry's, fixed before a
/// byte arrives, so a hostile peer cannot choose how much this allocates.
async fn receive<S, F>(
    stream: &mut S,
    exporter: &[u8; EXPORTER_LEN],
    peer_role: Role,
    accept: &F,
) -> Result<[u8; 32]>
where
    S: AsyncRead + Unpin,
    F: Fn(&[u8]) -> bool,
{
    let info = SUITE.info();
    let mut message = vec![0u8; info.public_key_len + info.signature_len];
    stream
        .read_exact(&mut message)
        .await
        .map_err(|_| CustodyError::PeerAuthentication("the peer sent no complete proof"))?;
    let (public_key, signature) = message.split_at(info.public_key_len);
    if !accept(public_key) {
        return Err(CustodyError::PeerAuthentication(
            "the peer's key is not one this side expects",
        ));
    }
    MlDsa87::verify(
        public_key,
        &transcript(peer_role, public_key, exporter),
        signature,
    )
    .map_err(|_| {
        CustodyError::PeerAuthentication("the peer's signature does not cover this session")
    })?;
    Ok(identity_of_key(public_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_roles_sign_different_transcripts() {
        let exporter = [7u8; EXPORTER_LEN];
        assert_ne!(
            transcript(Role::Initiator, b"key", &exporter),
            transcript(Role::Responder, b"key", &exporter)
        );
        assert_ne!(
            transcript(Role::Initiator, b"key", &exporter),
            transcript(Role::Initiator, b"other key", &exporter)
        );
        assert_eq!(Role::Initiator.peer(), Role::Responder);
    }

    #[test]
    fn an_identity_never_prints_its_key() {
        let me = PqIdentity::from_seed(&MasterSeed::from_bytes([1; 32]));
        assert!(!format!("{me:?}").contains(&format!("{:?}", &me.public_key()[..4])));
        assert!(format!("{me:?}").contains("redacted"));
    }
}
