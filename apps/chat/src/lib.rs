//! Maya Chat — a detached, post-quantum, peer-to-peer messenger (ADR-031).
//!
//! - [`identity`] — ML-DSA-65 identities whose address is the chain's own,
//!   and signed X-Wing (ML-KEM-768 + X25519) prekey bundles.
//! - [`session`] — authenticated handshakes and one-to-one sessions with a
//!   symmetric chain per direction; each message key used once.
//! - [`relay`] — store-and-forward mailboxes and prekey directories.
//! - [`client`] — a user's sessions over a relay.
//! - [`net`] — the `/maya-chat/1` libp2p protocol and a relay node.
//!
//! RESEARCH: unaudited. Do not rely on it for sensitive conversations until
//! it has had an external review. Group chat (MLS) and metadata privacy
//! (mixnet) are later decisions, named in ADR-031.

pub mod client;
pub mod identity;
pub mod net;
pub mod relay;
pub mod session;

/// A chat (and chain) address.
pub type Address = [u8; 32];

/// Why a chat step was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChatError {
    /// The operating system's generator failed.
    #[error("no entropy")]
    Entropy,
    /// The signer refused.
    #[error("signing failed")]
    Signature,
    /// A signature that does not verify, or a key that is not the address's.
    #[error("bad signature")]
    BadSignature,
    /// A malformed key or ciphertext.
    #[error("malformed key or ciphertext")]
    BadKey,
    /// Past its expiry, or too far in the future.
    #[error("expired or not yet valid")]
    Expired,
    /// Addressed to someone else.
    #[error("not addressed to this identity")]
    NotForMe,
    /// A message for another session.
    #[error("message for another session")]
    WrongSession,
    /// A message already read.
    #[error("message already received")]
    Replayed,
    /// A message too far ahead of the last one received.
    #[error("message too far ahead")]
    TooFarAhead,
    /// Authentication failed: forged or corrupted.
    #[error("message failed to authenticate")]
    Decrypt,
    /// No session with that peer.
    #[error("no session with that peer")]
    NoSession,
    /// Encoding failed.
    #[error("encoding failed")]
    Encoding,
    /// A relay rule refused it.
    #[error("refused: {0}")]
    Refused(&'static str),
    /// The network failed.
    #[error("network: {0}")]
    Network(String),
}

/// Seconds since the Unix epoch.
#[must_use]
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
