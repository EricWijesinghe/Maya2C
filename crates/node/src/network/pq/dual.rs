//! The dual-KEM combiner: one session key from two unrelated hard problems.
//!
//! # Status
//!
//! **Off by default.** `/maya/dualkem/1.0.0` is offered alongside
//! `/maya/mlkem/1.0.0` and negotiated, and a node only offers it when its
//! configuration says so. See [`DualKemPolicy`] for why it ships disabled and
//! what has to happen before that changes.
//!
//! # What the combiner buys
//!
//! Confidentiality that survives a break in *either* primitive. ML-KEM rests on
//! Module-LWE, a structured lattice assumption; HQC rests on decoding random
//! quasi-cyclic codes. An advance against one is not an advance against the
//! other, so a channel keyed from both stays secret unless both fall — the same
//! reasoning `docs/hybrid-signatures.md` gives for ML-DSA plus SLH-DSA on every
//! transaction, applied to the transport.
//!
//! # What it costs, and this is the part to read
//!
//! **Confidentiality becomes an OR; availability becomes an AND.**
//!
//! The channel is secure if either KEM holds. But the connection *works* only
//! if both implementations are correct. A panic, a mis-derived secret, or an
//! encoding change in either one breaks every connection that negotiated this
//! protocol. `ml-kem` is pinned at a released 0.3.2 against a final FIPS 203;
//! `hqc-kem` is a release candidate tracking a **draft** FIPS 207.
//!
//! Pairing them means the transport's liveness is gated by the less-settled of
//! the two, in order to defend the transport's secrecy against a break in the
//! more-settled one. That can be the right trade. It is not a free one, and it
//! is why this is negotiated rather than required.
//!
//! # Why a KDF combiner and not double encryption
//!
//! "Requires both secrets" could be built by encrypting twice, once under each
//! KEM's secret. That is worse in every respect: it doubles the AEAD cost on
//! every frame, it doubles the nonce management surface, and it has no clean
//! security argument — nesting two AEADs is not a standard construction.
//!
//! A KDF over both secrets gives the property outright. The session key depends
//! on both inputs, so an adversary missing either one faces a PRF with an
//! unknown input, and one AEAD pass per frame is unchanged from today.
//!
//! # Why the transcript, and not just the secrets
//!
//! The combiner hashes **both shared secrets and both full transcripts** —
//! encapsulation keys and ciphertexts alike. Hashing the secrets alone is not
//! sound in general: a combiner over shared secrets is IND-CCA robust only if
//! the underlying KEMs are ciphertext-collision-resistant, which neither
//! specification promises. Binding the ciphertexts removes the assumption
//! rather than relying on it.
//!
//! The single-KEM handshake already did this, so the extension inherits the
//! property rather than inventing it.
//!
//! # Why the context string changes
//!
//! `KDF_CONTEXT` is not the single-KEM one. If both protocols derived under
//! the same context, a dual session and an ML-KEM-only session that happened to
//! share an ML-KEM transcript would derive related keys, and the second KEM
//! would be contributing nothing at exactly the moment it was supposed to
//! matter. A new protocol gets a new context; that is what the string is for.

#[cfg(feature = "hqc")]
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
#[cfg(feature = "hqc")]
use zeroize::Zeroizing;

#[cfg(feature = "hqc")]
use maya_crypto_pq::{hqc, kem};

#[cfg(feature = "hqc")]
use crate::error::NodeError;
#[cfg(feature = "hqc")]
use crate::network::pq::handshake::SessionKeys;

/// Wire version of the dual-KEM handshake.
///
/// Separate from the single-KEM handshake's version. The two protocols are
/// negotiated by libp2p protocol name, so their version bytes never meet — but
/// giving the draft-tracking protocol its own counter means a revision to
/// draft FIPS 207 can be shipped as version 2 here without touching the
/// stable ML-KEM path at all.
pub const VERSION: u8 = 1;

#[cfg(feature = "hqc")]
/// Bytes the responder sends: version, then both encapsulation keys.
pub const RESPONDER_MESSAGE_LEN: usize =
    1 + kem::ENCAPSULATION_KEY_LEN + hqc::ENCAPSULATION_KEY_LEN;

#[cfg(feature = "hqc")]
/// Bytes the initiator sends: version, then both ciphertexts.
pub const INITIATOR_MESSAGE_LEN: usize = 1 + kem::CIPHERTEXT_LEN + hqc::CIPHERTEXT_LEN;

#[cfg(feature = "hqc")]
/// Total handshake bytes on the wire, both directions.
///
/// 15,766 against the single-KEM handshake's 2,274.
///
/// This total does **not** decide whether an extra round trip is paid, and an
/// early draft of this work said it did. Congestion control is per-direction:
/// what has to fit in a window is the largest single message, which is
/// [`RESPONDER_MESSAGE_LEN`] at 5,699 bytes — four segments at a 1,460-byte
/// MSS, against a ten-segment initial window (RFC 6928). It fits.
///
/// So the cost is bandwidth, not latency. `network::pq::measure` reports both.
pub const HANDSHAKE_BYTES: usize = RESPONDER_MESSAGE_LEN + INITIATOR_MESSAGE_LEN;

#[cfg(feature = "hqc")]
/// Domain separator for the dual-KEM key derivation.
///
/// Deliberately not the single-KEM context. See the module documentation.
const KDF_CONTEXT: &str = "maya2c 2026-09-01 p2p ml-kem-768 + hqc-192 dual session keys v1";

#[cfg(feature = "hqc")]
/// Domain separator prefixed to the transcript.
const TRANSCRIPT_DOMAIN: &[u8] = b"maya2c.p2p.dualkem.mlkem768.hqc192.transcript.v1";

/// libp2p protocol name for the dual-KEM handshake.
///
/// A *different* name from `network::pq::PROTOCOL`, not a version bump of it.
/// That is what makes this negotiable: libp2p's multistream-select offers both,
/// and a peer that speaks only the single-KEM protocol still connects. A
/// version bump would have made the old protocol unspeakable and forced a flag
/// day on a network that has no way to coordinate one.
pub const PROTOCOL: &str = "/maya/dualkem/1.0.0";

/// Whether this node offers the dual-KEM protocol.
///
/// # Why the default is off
///
/// The same discipline as shipping a consensus fork at `u64::MAX`. `hqc-kem` is
/// a release candidate against a draft standard; a bug in it takes down every
/// connection that negotiated this protocol, because the combiner needs both
/// secrets to agree. Default-off means the failure mode of a bad HQC release is
/// "nobody used it", not "the network partitioned".
///
/// # What has to be true before the default flips
///
/// Written down so it is a checklist rather than a judgement call:
///
/// 1. `hqc-kem` reaches a non-release-candidate version.
/// 2. Draft FIPS 207 is published as a final standard, or the draft it tracks
///    is stable across at least one revision.
/// 3. The protocol has run enabled on a non-trivial fraction of the network
///    long enough to have seen real churn, without a handshake failure
///    attributable to the HQC half.
///
/// # Why not simply require it
///
/// Requiring it removes the downgrade question — an active attacker cannot
/// force the weaker path if there is no weaker path. That is a real advantage,
/// and it is why this enum has a [`DualKemPolicy::Required`] arm. But requiring
/// it *today* would make network liveness depend on a release candidate, and a
/// transport that is unbreakable and also down is not an improvement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DualKemPolicy {
    /// Do not offer the protocol. Peers see only `/maya/mlkem/1.0.0`.
    #[default]
    Disabled,
    /// Offer it, and use it when the peer also supports it.
    ///
    /// A peer that does not falls back to the single-KEM protocol. That
    /// fallback is a downgrade an active attacker can force by suppressing the
    /// dual protocol from negotiation — which is the price of being able to
    /// roll this out at all, and is why this is not the end state.
    Offered,
    /// Offer it and refuse connections that will not use it.
    ///
    /// No downgrade is possible. Only safe once the conditions above hold.
    Required,
}

/// Why a `--dual-kem` value could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownPolicy(String);

impl core::fmt::Display for UnknownPolicy {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "unknown dual-KEM policy '{}': expected off, preferred, or required",
            self.0
        )
    }
}

impl std::error::Error for UnknownPolicy {}

impl core::str::FromStr for DualKemPolicy {
    type Err = UnknownPolicy;

    /// Parses an operator-facing policy name.
    ///
    /// The operator-facing names are `off`, `preferred`, and `required`; the
    /// variant names `disabled` and `offered` are accepted as aliases so that a
    /// value copied out of a log or a `Debug` line still works. Case is
    /// ignored, because a flag that rejects `Off` teaches nothing.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "off" | "disabled" | "none" => Ok(Self::Disabled),
            "preferred" | "offered" | "on" => Ok(Self::Offered),
            "required" | "strict" => Ok(Self::Required),
            other => Err(UnknownPolicy(other.to_string())),
        }
    }
}

impl core::fmt::Display for DualKemPolicy {
    /// The operator-facing name, so a log line round-trips through
    /// [`DualKemPolicy::from_str`](core::str::FromStr::from_str).
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Disabled => "off",
            Self::Offered => "preferred",
            Self::Required => "required",
        })
    }
}

impl DualKemPolicy {
    /// Parses a policy name into the node's own error type.
    ///
    /// [`core::str::FromStr`] returns [`UnknownPolicy`], which is right for a
    /// standalone parse. Configuration loading wants a `NodeError` so the
    /// failure reads like every other configuration failure, and this is that
    /// conversion in one place rather than at each call site.
    ///
    /// # Errors
    ///
    /// [`crate::error::NodeError::Decode`] naming the accepted values.
    pub fn from_str_checked(value: &str) -> crate::error::Result<Self> {
        use core::str::FromStr as _;
        let policy = Self::from_str(value).map_err(|e| crate::error::NodeError::Decode(e.to_string()))?;
        // A build without HQC (every `production` build, ADR-016) cannot
        // speak the dual protocol, so a configuration asking for it is an
        // error at load time rather than a connection failure later.
        if policy.offers() && !cfg!(feature = "hqc") {
            return Err(crate::error::NodeError::Decode(format!(
                "dual_kem = \"{value}\" needs the `hqc` feature, which this build omits \
                 (production builds do: HQC is a draft standard, ADR-009, ADR-016)"
            )));
        }
        Ok(policy)
    }

    /// Whether the protocol should be advertised to peers.
    #[must_use]
    pub const fn offers(self) -> bool {
        matches!(self, Self::Offered | Self::Required)
    }

    /// Whether a peer that cannot speak it should be refused.
    #[must_use]
    pub const fn requires(self) -> bool {
        matches!(self, Self::Required)
    }
}

#[cfg(feature = "hqc")]
/// The four values a completed dual handshake binds into the session key.
///
/// Grouped into a struct rather than passed as four arguments because the
/// order matters and is consensus between the two peers: a caller that swapped
/// the two ciphertexts would derive a different key and the connection would
/// fail to authenticate its first frame, with nothing to point at.
pub struct Transcript<'a> {
    /// ML-KEM-768 encapsulation key, as sent.
    pub mlkem_encapsulation_key: &'a [u8; kem::ENCAPSULATION_KEY_LEN],
    /// ML-KEM-768 ciphertext, as sent.
    pub mlkem_ciphertext: &'a [u8; kem::CIPHERTEXT_LEN],
    /// HQC-192 encapsulation key, as sent.
    pub hqc_encapsulation_key: &'a [u8; hqc::ENCAPSULATION_KEY_LEN],
    /// HQC-192 ciphertext, as sent.
    pub hqc_ciphertext: &'a [u8; hqc::CIPHERTEXT_LEN],
}

#[cfg(feature = "hqc")]
/// Derives directional session keys from both shared secrets and the full
/// transcript of both key exchanges.
///
/// # Ordering
///
/// ML-KEM first, then HQC, then the transcript. The order is part of the
/// protocol: two peers folding the secrets in different orders derive different
/// keys. It is fixed here in one place so neither side can choose.
///
/// # Why no "which KEM failed" signal
///
/// There is none, because neither KEM can fail. Both use a Fujisaki–Okamoto
/// transform with implicit rejection: a forged ciphertext yields a pseudorandom
/// secret rather than an error. So a mismatch surfaces uniformly, as the first
/// frame failing to authenticate — and an attacker learns which KEM they broke
/// only by breaking it, not by timing the handshake.
#[must_use]
pub fn derive_dual(
    mlkem_secret: &kem::SharedSecret,
    hqc_secret: &hqc::SharedSecret,
    transcript: &Transcript<'_>,
) -> SessionKeys {
    let mut hasher = blake3::Hasher::new_derive_key(KDF_CONTEXT);

    // Both secrets, before anything else. An adversary missing either one is
    // facing a PRF with an unknown input regardless of what follows.
    hasher.update(mlkem_secret.as_bytes());
    hasher.update(hqc_secret.as_bytes());

    hasher.update(TRANSCRIPT_DOMAIN);
    hasher.update(&[VERSION]);
    hasher.update(transcript.mlkem_encapsulation_key);
    hasher.update(transcript.mlkem_ciphertext);
    hasher.update(transcript.hqc_encapsulation_key);
    hasher.update(transcript.hqc_ciphertext);

    // 64 bytes split in half, matching the single-KEM handshake: one XOF read
    // is cheaper than two labelled hashes and the halves are independent.
    let mut okm = Zeroizing::new([0u8; 64]);
    hasher.finalize_xof().fill(okm.as_mut_slice());

    let mut initiator_to_responder = Zeroizing::new([0u8; 32]);
    initiator_to_responder.copy_from_slice(&okm[..32]);

    let mut responder_to_initiator = Zeroizing::new([0u8; 32]);
    responder_to_initiator.copy_from_slice(&okm[32..]);

    SessionKeys {
        initiator_to_responder,
        responder_to_initiator,
    }
}

#[cfg(feature = "hqc")]
/// Runs the responder half: generate both keypairs, send both keys,
/// decapsulate both ciphertexts.
///
/// # Both keypairs are generated before either is sent
///
/// Not for speed — because the two encapsulation keys travel in one message.
/// Sending them separately would let a peer stall between them and hold a
/// half-open handshake, and would put two write syscalls where the congestion
/// window already makes one expensive.
///
/// # Errors
///
/// [`NodeError::PqHandshake`] if the peer sends an unsupported version, a
/// ciphertext that does not decode, closes early, or the stream fails.
pub async fn respond<S>(stream: &mut S) -> Result<SessionKeys, NodeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mlkem_dk, mlkem_ek) = kem::generate_keypair();
    let (hqc_dk, hqc_ek) = hqc::generate_keypair();

    let mlkem_encoded = mlkem_ek.to_bytes();
    let hqc_encoded = hqc_ek.to_bytes();

    let mut outbound = [0u8; RESPONDER_MESSAGE_LEN];
    outbound[0] = VERSION;
    let split = 1 + kem::ENCAPSULATION_KEY_LEN;
    outbound[1..split].copy_from_slice(&mlkem_encoded);
    outbound[split..].copy_from_slice(&hqc_encoded);
    write_all(stream, &outbound).await?;

    let mut inbound = [0u8; INITIATOR_MESSAGE_LEN];
    read_exact(stream, &mut inbound).await?;
    check_version(inbound[0])?;

    let split = 1 + kem::CIPHERTEXT_LEN;
    let mut mlkem_ciphertext = [0u8; kem::CIPHERTEXT_LEN];
    mlkem_ciphertext.copy_from_slice(&inbound[1..split]);
    let mut hqc_ciphertext = [0u8; hqc::CIPHERTEXT_LEN];
    hqc_ciphertext.copy_from_slice(&inbound[split..]);

    // Neither decapsulation branches on validity: both KEMs use a
    // Fujisaki-Okamoto transform with implicit rejection, so a forged
    // ciphertext yields a pseudorandom secret rather than an error. HQC's
    // `decapsulate` can still refuse to *decode* a malformed ciphertext, which
    // is a different thing and leaks only that the peer sent garbage of the
    // right length.
    let mlkem_secret = mlkem_dk.decapsulate(&mlkem_ciphertext);
    let hqc_secret = hqc_dk.decapsulate(&hqc_ciphertext).map_err(|e| {
        NodeError::PqHandshake(format!("peer sent an undecodable HQC ciphertext: {e}"))
    })?;

    Ok(derive_dual(
        &mlkem_secret,
        &hqc_secret,
        &Transcript {
            mlkem_encapsulation_key: &mlkem_encoded,
            mlkem_ciphertext: &mlkem_ciphertext,
            hqc_encapsulation_key: &hqc_encoded,
            hqc_ciphertext: &hqc_ciphertext,
        },
    ))
}

#[cfg(feature = "hqc")]
/// Runs the initiator half: receive both keys, encapsulate to both, send both
/// ciphertexts.
///
/// # Errors
///
/// [`NodeError::PqHandshake`] if the peer sends an unsupported version or an
/// unusable encapsulation key for either KEM, closes early, or the stream
/// fails.
pub async fn initiate<S>(stream: &mut S) -> Result<SessionKeys, NodeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut inbound = [0u8; RESPONDER_MESSAGE_LEN];
    read_exact(stream, &mut inbound).await?;
    check_version(inbound[0])?;

    let split = 1 + kem::ENCAPSULATION_KEY_LEN;
    let mut mlkem_encoded = [0u8; kem::ENCAPSULATION_KEY_LEN];
    mlkem_encoded.copy_from_slice(&inbound[1..split]);
    let mut hqc_encoded = [0u8; hqc::ENCAPSULATION_KEY_LEN];
    hqc_encoded.copy_from_slice(&inbound[split..]);

    // Both of these *can* fail, unlike decapsulation: a received encapsulation
    // key is structured and a peer can send anything.
    let mlkem_ek = kem::EncapsulationKey::from_bytes(&mlkem_encoded).map_err(|e| {
        NodeError::PqHandshake(format!(
            "peer sent an unusable ML-KEM encapsulation key: {e}"
        ))
    })?;
    let hqc_ek = hqc::EncapsulationKey::from_bytes(&hqc_encoded).map_err(|e| {
        NodeError::PqHandshake(format!("peer sent an unusable HQC encapsulation key: {e}"))
    })?;

    let (mlkem_ciphertext, mlkem_secret) = mlkem_ek.encapsulate();
    let (hqc_ciphertext, hqc_secret) = hqc_ek.encapsulate();

    let mut outbound = [0u8; INITIATOR_MESSAGE_LEN];
    outbound[0] = VERSION;
    let split = 1 + kem::CIPHERTEXT_LEN;
    outbound[1..split].copy_from_slice(&mlkem_ciphertext);
    outbound[split..].copy_from_slice(&hqc_ciphertext);
    write_all(stream, &outbound).await?;

    Ok(derive_dual(
        &mlkem_secret,
        &hqc_secret,
        &Transcript {
            mlkem_encapsulation_key: &mlkem_encoded,
            mlkem_ciphertext: &mlkem_ciphertext,
            hqc_encapsulation_key: &hqc_encoded,
            hqc_ciphertext: &hqc_ciphertext,
        },
    ))
}

#[cfg(feature = "hqc")]
fn check_version(version: u8) -> Result<(), NodeError> {
    if version == VERSION {
        Ok(())
    } else {
        Err(NodeError::PqHandshake(format!(
            "peer offered dual-KEM handshake version {version}, this node speaks {VERSION}"
        )))
    }
}

#[cfg(feature = "hqc")]
async fn read_exact<S>(stream: &mut S, buf: &mut [u8]) -> Result<(), NodeError>
where
    S: AsyncRead + Unpin,
{
    stream
        .read_exact(buf)
        .await
        .map_err(|e| NodeError::PqHandshake(format!("dual-KEM handshake read failed: {e}")))
}

#[cfg(feature = "hqc")]
async fn write_all<S>(stream: &mut S, buf: &[u8]) -> Result<(), NodeError>
where
    S: AsyncWrite + Unpin,
{
    stream
        .write_all(buf)
        .await
        .map_err(|e| NodeError::PqHandshake(format!("dual-KEM handshake write failed: {e}")))?;
    // Flushed explicitly, exactly as the single-KEM handshake does, and for a
    // reason this protocol makes sharper.
    //
    // `write_all` fills a buffer; it does not put bytes on the wire. libp2p's
    // noise writer emits a frame on flush or when its buffer fills, and the
    // peer here is blocked in `read_exact` for a precise byte count. An
    // unflushed write is therefore a deadlock, not a slow start.
    //
    // The single-KEM handshake's 1,185-byte message happened to survive an
    // earlier version of this function that omitted the flush; the dual
    // handshake's 5,699-byte message did not, and the failure looked like a
    // negotiation problem rather than a missing flush. Hence this comment.
    stream
        .flush()
        .await
        .map_err(|e| NodeError::PqHandshake(format!("dual-KEM handshake flush failed: {e}")))
}

#[cfg(all(test, feature = "hqc"))]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use futures::io::Cursor;

    /// A complete, agreeing dual exchange. Returns both sides' keys and the
    /// raw material, so a test can perturb one piece and re-derive.
    struct Exchange {
        mlkem_ek: [u8; kem::ENCAPSULATION_KEY_LEN],
        mlkem_ct: [u8; kem::CIPHERTEXT_LEN],
        hqc_ek: [u8; hqc::ENCAPSULATION_KEY_LEN],
        hqc_ct: [u8; hqc::CIPHERTEXT_LEN],
        mlkem_secret: kem::SharedSecret,
        hqc_secret: hqc::SharedSecret,
    }

    fn exchange() -> Exchange {
        let (mlkem_dk, mlkem_ek_obj) = kem::generate_keypair();
        let (mlkem_ct, _) = mlkem_ek_obj.encapsulate();
        let mlkem_secret = mlkem_dk.decapsulate(&mlkem_ct);

        let (hqc_dk, hqc_ek_obj) = hqc::generate_keypair();
        let (hqc_ct, _) = hqc_ek_obj.encapsulate();
        let hqc_secret = hqc_dk.decapsulate(&hqc_ct).expect("well-formed");

        Exchange {
            mlkem_ek: mlkem_ek_obj.to_bytes(),
            mlkem_ct,
            hqc_ek: hqc_ek_obj.to_bytes(),
            hqc_ct,
            mlkem_secret,
            hqc_secret,
        }
    }

    fn derive_from(e: &Exchange) -> SessionKeys {
        derive_dual(
            &e.mlkem_secret,
            &e.hqc_secret,
            &Transcript {
                mlkem_encapsulation_key: &e.mlkem_ek,
                mlkem_ciphertext: &e.mlkem_ct,
                hqc_encapsulation_key: &e.hqc_ek,
                hqc_ciphertext: &e.hqc_ct,
            },
        )
    }

    #[test]
    fn the_two_directional_keys_differ() {
        // One key both ways would put the peers' nonce counters in one space,
        // and a repeated (key, nonce) destroys ChaCha20-Poly1305 outright.
        let keys = derive_from(&exchange());
        assert_ne!(
            keys.initiator_to_responder.as_slice(),
            keys.responder_to_initiator.as_slice()
        );
    }

    #[test]
    fn the_derivation_is_deterministic() {
        let e = exchange();
        let first = derive_from(&e);
        let second = derive_from(&e);
        assert_eq!(
            first.initiator_to_responder.as_slice(),
            second.initiator_to_responder.as_slice()
        );
        assert_eq!(
            first.responder_to_initiator.as_slice(),
            second.responder_to_initiator.as_slice()
        );
    }

    #[test]
    fn a_one_bit_change_in_each_transcript_field_changes_the_keys() {
        // Each of the four transcript pieces is perturbed independently. A
        // combiner that dropped one from the hash would still pass on the
        // other three, so they are checked one at a time rather than together.
        let e = exchange();
        let baseline = derive_from(&e);

        let flipped = |ek: &[u8; kem::ENCAPSULATION_KEY_LEN],
                       ct: &[u8; kem::CIPHERTEXT_LEN],
                       hek: &[u8; hqc::ENCAPSULATION_KEY_LEN],
                       hct: &[u8; hqc::CIPHERTEXT_LEN]| {
            derive_dual(
                &e.mlkem_secret,
                &e.hqc_secret,
                &Transcript {
                    mlkem_encapsulation_key: ek,
                    mlkem_ciphertext: ct,
                    hqc_encapsulation_key: hek,
                    hqc_ciphertext: hct,
                },
            )
        };

        let mut ek = e.mlkem_ek;
        ek[0] ^= 1;
        assert_ne!(
            flipped(&ek, &e.mlkem_ct, &e.hqc_ek, &e.hqc_ct)
                .initiator_to_responder
                .as_slice(),
            baseline.initiator_to_responder.as_slice(),
            "the ML-KEM encapsulation key must be bound"
        );

        let mut ct = e.mlkem_ct;
        ct[0] ^= 1;
        assert_ne!(
            flipped(&e.mlkem_ek, &ct, &e.hqc_ek, &e.hqc_ct)
                .initiator_to_responder
                .as_slice(),
            baseline.initiator_to_responder.as_slice(),
            "the ML-KEM ciphertext must be bound"
        );

        let mut hek = e.hqc_ek;
        hek[0] ^= 1;
        assert_ne!(
            flipped(&e.mlkem_ek, &e.mlkem_ct, &hek, &e.hqc_ct)
                .initiator_to_responder
                .as_slice(),
            baseline.initiator_to_responder.as_slice(),
            "the HQC encapsulation key must be bound"
        );

        let mut hct = e.hqc_ct;
        hct[0] ^= 1;
        assert_ne!(
            flipped(&e.mlkem_ek, &e.mlkem_ct, &e.hqc_ek, &hct)
                .initiator_to_responder
                .as_slice(),
            baseline.initiator_to_responder.as_slice(),
            "the HQC ciphertext must be bound"
        );
    }

    #[test]
    fn changing_either_secret_changes_the_keys() {
        // This is the property the whole design exists for: an adversary who
        // recovers one secret and not the other still cannot derive the key.
        let e = exchange();
        let baseline = derive_from(&e);

        let other = exchange();

        // Different ML-KEM secret, same HQC secret and transcript.
        let keys = derive_dual(
            &other.mlkem_secret,
            &e.hqc_secret,
            &Transcript {
                mlkem_encapsulation_key: &e.mlkem_ek,
                mlkem_ciphertext: &e.mlkem_ct,
                hqc_encapsulation_key: &e.hqc_ek,
                hqc_ciphertext: &e.hqc_ct,
            },
        );
        assert_ne!(
            keys.initiator_to_responder.as_slice(),
            baseline.initiator_to_responder.as_slice(),
            "breaking HQC alone must not yield the session key"
        );

        // Different HQC secret, same ML-KEM secret and transcript.
        let keys = derive_dual(
            &e.mlkem_secret,
            &other.hqc_secret,
            &Transcript {
                mlkem_encapsulation_key: &e.mlkem_ek,
                mlkem_ciphertext: &e.mlkem_ct,
                hqc_encapsulation_key: &e.hqc_ek,
                hqc_ciphertext: &e.hqc_ct,
            },
        );
        assert_ne!(
            keys.initiator_to_responder.as_slice(),
            baseline.initiator_to_responder.as_slice(),
            "breaking ML-KEM alone must not yield the session key"
        );
    }

    #[test]
    fn the_dual_derivation_differs_from_the_single_kem_one() {
        // Different KDF context. If these ever agreed, a dual session and an
        // ML-KEM-only session sharing a transcript would share keys, and the
        // second KEM would contribute nothing.
        let e = exchange();
        let dual = derive_from(&e);
        let single = crate::network::pq::handshake::derive_for_test(
            &e.mlkem_secret,
            &e.mlkem_ek,
            &e.mlkem_ct,
        );
        assert_ne!(
            dual.initiator_to_responder.as_slice(),
            single.initiator_to_responder.as_slice()
        );
    }

    #[test]
    fn the_policy_default_is_disabled() {
        // The rollout discipline, asserted rather than described. If someone
        // changes the default, this is what tells them it was a decision.
        assert_eq!(DualKemPolicy::default(), DualKemPolicy::Disabled);
        assert!(!DualKemPolicy::default().offers());
        assert!(!DualKemPolicy::default().requires());

        assert!(DualKemPolicy::Offered.offers());
        assert!(!DualKemPolicy::Offered.requires());

        assert!(DualKemPolicy::Required.offers());
        assert!(DualKemPolicy::Required.requires());
    }

    /// A pair of in-memory duplex streams.
    ///
    /// Sized well past [`RESPONDER_MESSAGE_LEN`], unlike the single-KEM
    /// handshake's 4 KiB pair: the dual responder message alone is 5,699 bytes,
    /// so a 4 KiB ring would make every test depend on both halves being
    /// pumped concurrently to avoid a stall. That is true here, but relying on
    /// it would turn a framing bug into a hang rather than a failure.
    fn duplex() -> (futures_ringbuf::Endpoint, futures_ringbuf::Endpoint) {
        futures_ringbuf::Endpoint::pair(32_768, 32_768)
    }

    #[tokio::test]
    async fn both_sides_derive_the_same_keys() {
        let (mut a, mut b) = duplex();

        let responder = tokio::spawn(async move { respond(&mut a).await });
        let initiator = initiate(&mut b).await.expect("initiate");
        let responder = responder.await.expect("join").expect("respond");

        assert_eq!(
            initiator.initiator_to_responder.as_slice(),
            responder.initiator_to_responder.as_slice()
        );
        assert_eq!(
            initiator.responder_to_initiator.as_slice(),
            responder.responder_to_initiator.as_slice()
        );
    }

    #[tokio::test]
    async fn two_handshakes_derive_unrelated_keys() {
        // Forward secrecy: fresh keypairs per connection on both KEMs. If
        // either were reused, these would collide.
        let mut seen = Vec::new();
        for _ in 0..2 {
            let (mut a, mut b) = duplex();
            let responder = tokio::spawn(async move { respond(&mut a).await });
            let keys = initiate(&mut b).await.expect("initiate");
            responder.await.expect("join").expect("respond");
            seen.push(keys.initiator_to_responder.to_vec());
        }
        assert_ne!(seen[0], seen[1]);
    }

    #[tokio::test]
    async fn a_wrong_version_is_refused_by_name() {
        let mut stream = Cursor::new({
            let mut message = vec![0u8; RESPONDER_MESSAGE_LEN];
            message[0] = VERSION + 1;
            message
        });
        let error = initiate(&mut stream).await.expect_err("version mismatch");
        assert!(format!("{error}").contains("dual-KEM handshake version"));
    }

    #[tokio::test]
    async fn a_malformed_ml_kem_key_is_refused() {
        // FIPS 203's modulus check, reached through the handshake because this
        // is where a hostile peer meets it.
        //
        // Only the ML-KEM half can be caught here. Draft FIPS 207 defines no
        // structural validity condition on an HQC encapsulation key, so any
        // correctly-sized string is accepted and the mismatch instead surfaces
        // when the first frame fails to authenticate. That asymmetry is pinned
        // in `maya_crypto_pq::hqc`; this test would start passing for the wrong
        // reason if it were asserted here.
        let mut stream = Cursor::new({
            let mut message = vec![0xffu8; RESPONDER_MESSAGE_LEN];
            message[0] = VERSION;
            message
        });
        let error = initiate(&mut stream)
            .await
            .expect_err("malformed ML-KEM key rejected");
        assert!(
            format!("{error}").contains("unusable ML-KEM encapsulation key"),
            "unhelpful error: {error}"
        );
    }

    #[tokio::test]
    async fn a_truncated_handshake_is_an_error_not_a_hang() {
        let mut stream = Cursor::new(vec![VERSION, 0x00, 0x00]);
        let error = initiate(&mut stream).await.expect_err("truncated");
        assert!(format!("{error}").contains("read failed"));
    }

    #[tokio::test]
    async fn the_handshake_survives_a_ring_smaller_than_its_first_message() {
        // A 4 KiB ring against a 5,699-byte responder message, so the write
        // cannot complete in one go and the two halves must interleave. Cheap
        // insurance against a future change that buffers a whole message before
        // sending, which is what the missing flush in `write_all` effectively
        // did.
        let (mut a, mut b) = futures_ringbuf::Endpoint::pair(4096, 4096);
        let responder = tokio::spawn(async move { respond(&mut a).await });
        let initiator = initiate(&mut b).await;
        let responder = responder.await.expect("join");
        assert!(initiator.is_ok(), "initiator: {:?}", initiator.err());
        assert!(responder.is_ok(), "responder: {:?}", responder.err());
    }

    #[test]
    fn the_protocol_name_is_distinct_from_the_single_kem_one() {
        // Distinct name, not a version bump: that is what lets both be offered
        // and a single-KEM peer still connect.
        assert_ne!(PROTOCOL, crate::network::pq::PROTOCOL);
        assert!(libp2p::swarm::StreamProtocol::try_from_owned(PROTOCOL.to_string()).is_ok());
    }

    #[test]
    fn every_policy_name_round_trips() {
        use core::str::FromStr;
        for policy in [
            DualKemPolicy::Disabled,
            DualKemPolicy::Offered,
            DualKemPolicy::Required,
        ] {
            let rendered = policy.to_string();
            assert_eq!(
                DualKemPolicy::from_str(&rendered),
                Ok(policy),
                "{rendered} did not round-trip"
            );
        }
    }

    #[test]
    fn policy_names_are_case_insensitive_and_aliased() {
        use core::str::FromStr;
        assert_eq!(DualKemPolicy::from_str("OFF"), Ok(DualKemPolicy::Disabled));
        assert_eq!(
            DualKemPolicy::from_str("Disabled"),
            Ok(DualKemPolicy::Disabled)
        );
        assert_eq!(
            DualKemPolicy::from_str("preferred"),
            Ok(DualKemPolicy::Offered)
        );
        assert_eq!(
            DualKemPolicy::from_str("offered"),
            Ok(DualKemPolicy::Offered)
        );
        assert_eq!(
            DualKemPolicy::from_str("REQUIRED"),
            Ok(DualKemPolicy::Required)
        );
    }

    #[test]
    fn an_unknown_policy_name_says_what_is_accepted() {
        use core::str::FromStr;
        let error = DualKemPolicy::from_str("maybe").expect_err("not a policy");
        let rendered = error.to_string();
        assert!(rendered.contains("off"), "unhelpful: {rendered}");
        assert!(rendered.contains("preferred"), "unhelpful: {rendered}");
        assert!(rendered.contains("required"), "unhelpful: {rendered}");
    }

    #[test]
    fn the_advertised_message_sizes_match_the_primitives() {
        assert_eq!(RESPONDER_MESSAGE_LEN, 1 + 1184 + 4514);
        assert_eq!(INITIATOR_MESSAGE_LEN, 1 + 1088 + 8978);
        assert_eq!(HANDSHAKE_BYTES, 15_766);
    }
}
