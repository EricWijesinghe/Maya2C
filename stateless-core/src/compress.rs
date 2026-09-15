//! The two-to-one node function a tree is built from, and the shipped backend.
//!
//! A tree is generic over [`Compress`] so that the BLAKE3 backend the node
//! commits with and the Ring-SIS research backend ([`crate::lattice`]) run
//! through one tree, one witness format and one test suite. Sizes then differ
//! only by digest width, which is the comparison `docs/stateless.md` reports.

use crate::error::Defect;
use crate::params::{BLAKE3_DIGEST_BYTES, KEY_BYTES, VALUE_BYTES};

/// A tree key: an account address.
pub type Key = [u8; KEY_BYTES];

/// A leaf value: an encoded account.
pub type Value = [u8; VALUE_BYTES];

/// A collision-resistant node function.
///
/// Leaves and internal nodes must be domain-separated: a digest valid as one
/// must not be producible as the other, or a leaf could be presented as a
/// subtree (the classic Merkle second-preimage attack).
pub trait Compress {
    /// A node digest.
    type Digest: Copy + Eq + core::fmt::Debug;

    /// Encoded bytes in a digest.
    const DIGEST_BYTES: usize;

    /// Digest of a subtree holding no leaves.
    fn empty(&self) -> Self::Digest;

    /// Digest of a subtree holding exactly one leaf.
    fn leaf(&self, key: &Key, value: &Value) -> Self::Digest;

    /// Digest of a subtree split into `left` (next key bit 0) and `right`.
    fn internal(&self, left: &Self::Digest, right: &Self::Digest) -> Self::Digest;

    /// Appends a digest's canonical encoding.
    fn write_digest(digest: &Self::Digest, out: &mut Vec<u8>);

    /// Decodes exactly [`Self::DIGEST_BYTES`] bytes.
    ///
    /// # Errors
    ///
    /// [`Defect::NonCanonical`] if the bytes are not the one encoding of any
    /// digest.
    fn read_digest(bytes: &[u8]) -> Result<Self::Digest, Defect>;
}

/// BLAKE3 with derive-key domains. What the node's state root commits with.
///
/// Post-quantum for the only property a tree needs: Grover and BHT reduce
/// 256-bit collision search to about `2^85` quantum queries on paper, at a
/// memory cost that makes classical search cheaper in practice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Blake3;

const LEAF_DOMAIN: &str = "maya2c stateless-core leaf v1";
const INTERNAL_DOMAIN: &str = "maya2c stateless-core internal v1";

impl Compress for Blake3 {
    type Digest = [u8; BLAKE3_DIGEST_BYTES];

    const DIGEST_BYTES: usize = BLAKE3_DIGEST_BYTES;

    fn empty(&self) -> Self::Digest {
        [0; BLAKE3_DIGEST_BYTES]
    }

    fn leaf(&self, key: &Key, value: &Value) -> Self::Digest {
        let mut hasher = blake3::Hasher::new_derive_key(LEAF_DOMAIN);
        hasher.update(key);
        hasher.update(value);
        *hasher.finalize().as_bytes()
    }

    fn internal(&self, left: &Self::Digest, right: &Self::Digest) -> Self::Digest {
        let mut hasher = blake3::Hasher::new_derive_key(INTERNAL_DOMAIN);
        hasher.update(left);
        hasher.update(right);
        *hasher.finalize().as_bytes()
    }

    fn write_digest(digest: &Self::Digest, out: &mut Vec<u8>) {
        out.extend_from_slice(digest);
    }

    fn read_digest(bytes: &[u8]) -> Result<Self::Digest, Defect> {
        <Self::Digest>::try_from(bytes).map_err(|_| Defect::Malformed("digest length"))
    }
}
