//! Sizes, tags and the lattice parameter set.
//!
//! ## The lattice parameters are not borrowed, and that is why they are dark
//!
//! `htlc-lattice` borrows ML-DSA-65's parameters because inventing a set means
//! inventing a security argument. A Ring-SIS *compression function* has no
//! standardised set to borrow: FIPS 204 fixes a signature, not a hash. SWIFFT's
//! published parameters (`n = 64`, `p = 257`, `m = 16`) do not compress two of
//! their own digests into one — two 576-bit digests are 1152 bits against a
//! 1024-bit input — so they cannot build a tree.
//!
//! The set below is chosen for exact 2:1 compression over `n = 256`:
//! `q = 12289` fits 14 bits, a digest is `256 × 14 = 3584` bits, and two digests
//! fill `28` binary polynomials. Collisions are Ring-SIS solutions with
//! `‖x − x'‖∞ ≤ 1` over a `1 × 28` matrix. **No lattice estimator has been run on
//! it.** That is the reason [`crate::lattice`] is a research backend and the
//! node commits with [`crate::compress::Blake3`]. See `docs/stateless.md`.

/// Bytes in a tree key: an account address.
pub const KEY_BYTES: usize = 32;

/// Bits in a tree key, and so the deepest a leaf can sit.
pub const KEY_BITS: usize = KEY_BYTES * 8;

/// Bytes in a leaf value: balance then nonce, little-endian.
pub const VALUE_BYTES: usize = 16;

/// Largest encoded witness a decoder will look at.
///
/// An 8 MiB gossip block holds about 600 hybrid-signed transfers; opening two
/// keys each at 20 levels is about 0.8 MiB. A block past the cap is not
/// invalid — it is one a stateless node cannot follow, and says so.
pub const MAX_WITNESS_BYTES: usize = 2 * 1024 * 1024;

/// Most nodes a decoder will allocate.
///
/// The byte cap alone is not a memory cap: an empty subtree encodes in one
/// byte and decodes to a boxed node of about 56. An honest witness averages
/// about 17 encoded bytes per node (one split byte and one 33-byte sibling per
/// level), so a budget of one node per 16 bytes admits every honest witness and
/// bounds a hostile one to about 7 MiB of nodes.
pub const MAX_WITNESS_NODES: usize = MAX_WITNESS_BYTES / 16;

/// Wire tag: a subtree holding no leaves.
pub const TAG_EMPTY: u8 = 0;
/// Wire tag: a subtree the witness does not open, given by its digest.
pub const TAG_OPAQUE: u8 = 1;
/// Wire tag: a subtree holding exactly one leaf.
pub const TAG_LEAF: u8 = 2;
/// Wire tag: a subtree split on the next key bit.
pub const TAG_INTERNAL: u8 = 3;

/// Bytes in a BLAKE3 node digest.
pub const BLAKE3_DIGEST_BYTES: usize = 32;

/// Ring degree `n` of `R_q = Z_q[X]/(X^n + 1)`.
pub const RING_DEGREE: usize = 256;

/// Modulus `q`. Prime, `q ≡ 1 (mod 2n)`.
pub const MODULUS: u32 = 12_289;

/// Bits one reduced coefficient takes in a digest.
pub const COEFFICIENT_BITS: usize = 14;

/// Bytes in a Ring-SIS node digest: one ring element, packed.
pub const LATTICE_DIGEST_BYTES: usize = RING_DEGREE * COEFFICIENT_BITS / 8;

/// Binary polynomials an internal node's input fills: two digests' bits.
pub const INTERNAL_COLUMNS: usize = 2 * LATTICE_DIGEST_BYTES * 8 / RING_DEGREE;

/// Binary polynomials a leaf's input fills: key, value and one marker bit.
pub const LEAF_COLUMNS: usize = 2;

/// Bit set after a leaf's payload so no leaf input is the zero vector.
///
/// `A · 0 = 0` is the empty digest. Without the marker, the all-zero key
/// holding the all-zero value would hash to "no leaves here".
pub const LEAF_MARKER_BIT: usize = (KEY_BYTES + VALUE_BYTES) * 8;

const _: () = {
    assert!(MODULUS < 1 << COEFFICIENT_BITS);
    assert!((MODULUS - 1).is_multiple_of(2 * RING_DEGREE as u32));
    assert!((RING_DEGREE * COEFFICIENT_BITS).is_multiple_of(8));
    assert!(LATTICE_DIGEST_BYTES == 448);
    assert!(INTERNAL_COLUMNS * RING_DEGREE == 2 * LATTICE_DIGEST_BYTES * 8);
    assert!(INTERNAL_COLUMNS == 28);
    assert!(LEAF_MARKER_BIT < LEAF_COLUMNS * RING_DEGREE);
};
