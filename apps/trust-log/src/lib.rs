//! Trust log: statements the foundation makes, in a form anyone can check.
//!
//! Three tools from CATALOG §J share one shape — an ML-DSA-87 signed record
//! whose digest is anchored on chain:
//!
//! - [`release`] (`release-log-anchor`): the SHA-256 of a release's
//!   `SHA256SUMS`, so a swapped or back-dated release fails verification even
//!   when the release host is compromised. It warns; it never installs.
//! - [`incident`] (`signed-incidents`): a hash-chained log of outages and
//!   attacks; editing, removing or reordering any record breaks the chain.
//! - [`canary`] (`foundation-canary`): a statement carrying a recent block id,
//!   so it cannot be signed in advance, and a lapse check.
//!
//! **Anchoring** uses the existing `AnchorAttestation` transaction (schema
//! label + 32-byte root), the only tx kind that records an arbitrary digest
//! today. This crate does not build or submit that transaction — no
//! notary tool exists yet (CATALOG row `notary`); it prints the schema and
//! root to anchor and checks an anchor already on chain ([`anchor`]).
//!
//! It reads the chain over JSON-RPC only and links no node crate. What it
//! does not verify: the block an RPC endpoint returns is trusted as the
//! endpoint's word — point it at a node you run, or at several.

pub mod anchor;
pub mod canary;
pub mod chain;
pub mod incident;
pub mod key;
pub mod release;
pub mod signed;

/// Everything that can go wrong. Messages never carry key material.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file or the network could not be read or written.
    #[error("i/o: {0}")]
    Io(String),
    /// Input that does not parse (hex, JSON, a field out of bounds).
    #[error("malformed: {0}")]
    Malformed(String),
    /// A signature, digest or key that does not match.
    #[error("signature: {0}")]
    Signature(String),
    /// The chain does not show what the record claims.
    #[error("anchor: {0}")]
    Anchor(String),
    /// A release whose files do not match the signed statement.
    #[error("release mismatch: {0}")]
    Release(String),
    /// The incident log was edited, truncated or reordered.
    #[error("incident chain broken: {0}")]
    Chain(String),
    /// The canary is stale or not provably recent.
    #[error("canary lapsed: {0}")]
    Lapsed(String),
}

/// Crate result.
pub type Result<T> = std::result::Result<T, Error>;

/// Decodes exactly `N` bytes of hex.
///
/// # Errors
///
/// [`Error::Malformed`] for bad hex or the wrong length.
pub fn hex_array<const N: usize>(what: &str, text: &str) -> Result<[u8; N]> {
    let bytes = hex::decode(text.trim()).map_err(|e| Error::Malformed(format!("{what}: {e}")))?;
    bytes
        .try_into()
        .map_err(|b: Vec<u8>| Error::Malformed(format!("{what}: {} bytes, want {N}", b.len())))
}
