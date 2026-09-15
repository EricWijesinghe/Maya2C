//! A Ring-SIS compression function over `R_q = Z_q[X]/(X^n + 1)`: the research
//! backend.
//!
//! `f_A(x) = Σ_j a_j · x_j  (mod q, X^n + 1)` for public uniform `a_j ∈ R_q` and
//! binary polynomials `x_j` read from the input bits. Two inputs with one output
//! give `Σ_j a_j (x_j − x'_j) = 0` with every coefficient in `{−1, 0, 1}`: a
//! Ring-SIS solution. Leaves and internal nodes use independent matrices, so a
//! leaf/internal confusion is a solution for the concatenated matrix — still
//! Ring-SIS, still no shortcut.
//!
//! ## What it is not
//!
//! It is **linear**, so it is not a random oracle: `f(x) + f(y) = f(x + y)`
//! whenever `x` and `y` have disjoint support. A Merkle tree needs only collision
//! resistance, which linearity does not touch, but nothing else may use this as
//! a hash. And its parameters have no estimator run behind them — see
//! [`crate::params`]. A digest is 448 bytes against BLAKE3's 32, which is the
//! whole reason the node does not commit with it.
//!
//! Schoolbook, as in `htlc-lattice/src/matrix.rs`: the input is binary, so a
//! product is an accumulator over the set bits and one reduction per output.

use sha3::Shake128;
use sha3::digest::{ExtendableOutput, Update, XofReader};

use crate::compress::{Compress, Key, Value};
use crate::error::Defect;
use crate::params::{
    COEFFICIENT_BITS, INTERNAL_COLUMNS, LATTICE_DIGEST_BYTES, LEAF_COLUMNS, LEAF_MARKER_BIT,
    MODULUS, RING_DEGREE,
};

const LEAF_SEED: &[u8] = b"maya2c stateless-core ring-sis leaf matrix v1";
const INTERNAL_SEED: &[u8] = b"maya2c stateless-core ring-sis internal matrix v1";

const N: usize = RING_DEGREE;
const COEFFICIENT_MASK: u16 = (1 << COEFFICIENT_BITS) - 1;

/// A Ring-SIS node digest: one packed element of `R_q`.
pub type LatticeDigest = [u8; LATTICE_DIGEST_BYTES];

/// The two public matrices, expanded from fixed domain strings.
///
/// Transparent: no trapdoor exists because nobody sampled `A` — SHAKE128 did,
/// from a string anyone can read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RingSis {
    leaf: Vec<u32>,
    internal: Vec<u32>,
}

impl Default for RingSis {
    fn default() -> Self {
        Self::new()
    }
}

impl RingSis {
    /// Expands both matrices. About 7,700 coefficients; do it once.
    #[must_use]
    pub fn new() -> Self {
        Self {
            leaf: expand(LEAF_SEED, LEAF_COLUMNS),
            internal: expand(INTERNAL_SEED, INTERNAL_COLUMNS),
        }
    }
}

/// `columns` uniform polynomials by rejection sampling 16-bit candidates.
fn expand(seed: &[u8], columns: usize) -> Vec<u32> {
    let mut xof = Shake128::default();
    xof.update(seed);
    let mut reader = xof.finalize_xof();
    let mut out = Vec::with_capacity(columns * N);
    let mut candidate = [0u8; 2];
    while out.len() < columns * N {
        reader.read(&mut candidate);
        let value = u16::from_le_bytes(candidate) & COEFFICIENT_MASK;
        if u32::from(value) < MODULUS {
            out.push(u32::from(value));
        }
    }
    out
}

/// `f_A(bits)`, packed. `input` is read LSB-first, column-major.
fn apply(matrix: &[u32], input: &[u8]) -> LatticeDigest {
    // Bound: at most `columns × N` terms per slot, each below 2^14, so under
    // 2^27 for 28 columns — far inside i64.
    let mut wide = [0i64; 2 * N];
    for (column, a) in matrix.as_chunks::<N>().0.iter().enumerate() {
        for exponent in 0..N {
            let bit = column * N + exponent;
            if input[bit / 8] >> (bit % 8) & 1 == 1 {
                for (slot, &coefficient) in wide[exponent..exponent + N].iter_mut().zip(a) {
                    *slot += i64::from(coefficient);
                }
            }
        }
    }
    let (low, high) = wide.split_at(N);
    let mut packed = [0u8; LATTICE_DIGEST_BYTES];
    for (index, (&lo, &hi)) in low.iter().zip(high).enumerate() {
        // X^N = -1. `rem_euclid` by a positive modulus lands in [0, q).
        let reduced = (lo - hi).rem_euclid(i64::from(MODULUS));
        write_coefficient(&mut packed, index, u16::try_from(reduced).unwrap_or(0));
    }
    packed
}

fn write_coefficient(packed: &mut LatticeDigest, index: usize, value: u16) {
    for offset in 0..COEFFICIENT_BITS {
        if value >> offset & 1 == 1 {
            let bit = index * COEFFICIENT_BITS + offset;
            packed[bit / 8] |= 1 << (bit % 8);
        }
    }
}

fn read_coefficient(packed: &[u8], index: usize) -> u16 {
    let mut value = 0u16;
    for offset in 0..COEFFICIENT_BITS {
        let bit = index * COEFFICIENT_BITS + offset;
        value |= u16::from(packed[bit / 8] >> (bit % 8) & 1) << offset;
    }
    value
}

impl Compress for RingSis {
    type Digest = LatticeDigest;

    const DIGEST_BYTES: usize = LATTICE_DIGEST_BYTES;

    fn empty(&self) -> Self::Digest {
        [0; LATTICE_DIGEST_BYTES]
    }

    fn leaf(&self, key: &Key, value: &Value) -> Self::Digest {
        let mut input = [0u8; LEAF_COLUMNS * N / 8];
        input[..key.len()].copy_from_slice(key);
        input[key.len()..key.len() + value.len()].copy_from_slice(value);
        input[LEAF_MARKER_BIT / 8] |= 1 << (LEAF_MARKER_BIT % 8);
        apply(&self.leaf, &input)
    }

    fn internal(&self, left: &Self::Digest, right: &Self::Digest) -> Self::Digest {
        let mut input = [0u8; 2 * LATTICE_DIGEST_BYTES];
        input[..LATTICE_DIGEST_BYTES].copy_from_slice(left);
        input[LATTICE_DIGEST_BYTES..].copy_from_slice(right);
        apply(&self.internal, &input)
    }

    fn write_digest(digest: &Self::Digest, out: &mut Vec<u8>) {
        out.extend_from_slice(digest);
    }

    fn read_digest(bytes: &[u8]) -> Result<Self::Digest, Defect> {
        let digest =
            <Self::Digest>::try_from(bytes).map_err(|_| Defect::Malformed("digest length"))?;
        // 14 bits hold values up to 16383 and q is 12289: a coefficient at or
        // above q is a second spelling of a smaller one.
        if (0..N).any(|index| u32::from(read_coefficient(&digest, index)) >= MODULUS) {
            return Err(Defect::NonCanonical("coefficient at or above q"));
        }
        Ok(digest)
    }
}
