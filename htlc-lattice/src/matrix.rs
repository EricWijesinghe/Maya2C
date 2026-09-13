//! The public matrix `A`, expanded from a seed, and the product `A·s`.
//!
//! ## Why schoolbook multiplication and no NTT
//!
//! `s` is short, so each product term is a 23-bit coefficient times a value in
//! `[-128, 127]`, accumulated in `i64` and reduced once per output coefficient.
//! No modular multiplication, no Montgomery constants, no transform tables — the
//! whole ring arithmetic is an accumulator and one `rem_euclid`. That is about
//! two million additions per verification, which `benches/verify.rs` measures,
//! against an NTT that would be faster and would be a second hand-written
//! implementation of a transform to get wrong on a consensus path.
//!
//! `A` is sampled in the coefficient domain rather than ML-DSA's NTT domain.
//! A uniform matrix is uniform in either, so the hardness argument is the same;
//! the consequence is only that these matrices are not bit-compatible with
//! ML-DSA's `ExpandA`, which nothing here needs them to be.

use sha3::Shake128;
use sha3::digest::{ExtendableOutput, Update, XofReader};

use crate::error::{Error, Result};
use crate::params::{COEFFICIENT_BITS, K, L, N, Q, SEED_BYTES};

/// Domain separator for expanding `A`.
const MATRIX_DOMAIN: &[u8] = b"maya2c htlc-l matrix v1";

/// Keeps the low 23 bits of a 24-bit sample.
const COEFFICIENT_MASK: u32 = (1 << COEFFICIENT_BITS) - 1;

/// Bytes squeezed per refill: 280 candidates. The rejection rate is about one
/// in a thousand, so one block almost always fills a polynomial.
const SAMPLE_BLOCK: usize = 3 * 280;

/// `A`, row-major: `K` rows of `L` polynomials of `N` coefficients in `[0, q)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matrix {
    entries: Vec<u32>,
}

impl Matrix {
    /// Expands `A` from a seed.
    ///
    /// Deterministic: every node that reads the same seed builds the same
    /// matrix, which is what lets a lock carry 32 bytes instead of 30 KB.
    #[must_use]
    pub fn expand(seed: &[u8; SEED_BYTES]) -> Self {
        let mut entries = vec![0u32; K * L * N];
        for row in 0..K {
            for column in 0..L {
                let start = (row * L + column) * N;
                sample_uniform(seed, row, column, &mut entries[start..start + N]);
            }
        }
        Self { entries }
    }

    /// One polynomial of `A`.
    fn entry(&self, row: usize, column: usize) -> &[u32] {
        let start = (row * L + column) * N;
        &self.entries[start..start + N]
    }

    /// `A·s mod q`: `K` polynomials, `K × N` coefficients in `[0, q)`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Length`] unless `s` holds exactly `L × N` coefficients.
    pub fn product(&self, s: &[i8]) -> Result<Vec<u32>> {
        if s.len() != L * N {
            return Err(Error::Length {
                what: "s",
                expected: L * N,
                found: s.len(),
            });
        }
        let mut out = vec![0u32; K * N];
        // Allocated once and cleared per row: the verification loop allocates
        // nothing past this line (execution directive 3).
        let mut wide = [0i64; 2 * N];
        for (row, reduced) in out.as_chunks_mut::<N>().0.iter_mut().enumerate() {
            wide.fill(0);
            for (column, short) in s.as_chunks::<N>().0.iter().enumerate() {
                add_product(&mut wide, self.entry(row, column), short);
            }
            fold(&wide, reduced);
        }
        Ok(out)
    }
}

/// `wide += a · short` over `Z[X]`, before reduction by `X^N + 1`.
///
/// Bound: at most `L × N` terms land in one slot, each below `2^23 × 2^7`, so
/// every slot stays under `2^41` — far inside `i64`.
fn add_product(wide: &mut [i64; 2 * N], a: &[u32], short: &[i8]) {
    for (x, &coefficient) in a.iter().enumerate() {
        let coefficient = i64::from(coefficient);
        for (slot, &small) in wide[x..x + N].iter_mut().zip(short) {
            *slot += coefficient * i64::from(small);
        }
    }
}

/// Reduces by `X^N = -1`, then into `[0, q)`.
fn fold(wide: &[i64; 2 * N], out: &mut [u32]) {
    let (low, high) = wide.split_at(N);
    for ((slot, &lo), &hi) in out.iter_mut().zip(low).zip(high) {
        // `rem_euclid` by a positive modulus lands in [0, q), which fits u32.
        *slot = u32::try_from((lo - hi).rem_euclid(i64::from(Q))).unwrap_or(0);
    }
}

/// Fills one polynomial with coefficients uniform in `[0, q)`.
///
/// Rejection sampling over 23-bit candidates, as FIPS 204's `RejNTTPoly` does.
/// The loop is bounded by SHAKE's output rather than by a counter: a seed whose
/// stream rejects 256 of every 280 candidates block after block does not exist
/// short of breaking SHAKE128.
fn sample_uniform(seed: &[u8; SEED_BYTES], row: usize, column: usize, out: &mut [u32]) {
    let mut xof = Shake128::default();
    xof.update(MATRIX_DOMAIN);
    xof.update(seed);
    // K and L are single digits, so one byte each is an exact encoding.
    xof.update(&[row as u8, column as u8]);
    let mut reader = xof.finalize_xof();

    let mut block = [0u8; SAMPLE_BLOCK];
    let mut filled = 0;
    while filled < out.len() {
        reader.read(&mut block);
        for chunk in block.as_chunks::<3>().0 {
            let candidate = (u32::from(chunk[0])
                | (u32::from(chunk[1]) << 8)
                | (u32::from(chunk[2]) << 16))
                & COEFFICIENT_MASK;
            if candidate < Q {
                out[filled] = candidate;
                filled += 1;
                if filled == out.len() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion_is_deterministic_and_seed_dependent() {
        let a = Matrix::expand(&[1; SEED_BYTES]);
        assert_eq!(a, Matrix::expand(&[1; SEED_BYTES]));
        assert_ne!(a, Matrix::expand(&[2; SEED_BYTES]));
        assert!(a.entries.iter().all(|&c| c < Q));
    }

    #[test]
    fn the_product_is_linear() {
        // A·(s1 + s2) = A·s1 + A·s2 mod q. Catches a wrong fold sign or a
        // dropped high half, either of which still produces values in [0, q).
        let a = Matrix::expand(&[3; SEED_BYTES]);
        let s1: Vec<i8> = (0..L * N).map(|i| ((i * 7) % 9) as i8 - 4).collect();
        let s2: Vec<i8> = (0..L * N).map(|i| ((i * 5) % 9) as i8 - 4).collect();
        let sum: Vec<i8> = s1.iter().zip(&s2).map(|(x, y)| x + y).collect();

        let p1 = a.product(&s1).expect("s1");
        let p2 = a.product(&s2).expect("s2");
        let joint = a.product(&sum).expect("sum");
        for ((x, y), z) in p1.iter().zip(&p2).zip(&joint) {
            assert_eq!((u64::from(*x) + u64::from(*y)) % u64::from(Q), u64::from(*z));
        }
    }

    #[test]
    fn multiplying_by_x_wraps_with_a_sign_flip() {
        // s = X in column 0 only: row r of A·s is a_{r,0}·X, whose top
        // coefficient must wrap to position 0 negated. Pins the X^N = -1 fold.
        let a = Matrix::expand(&[4; SEED_BYTES]);
        let mut s = vec![0i8; L * N];
        s[1] = 1;
        let product = a.product(&s).expect("product");
        let entry = a.entry(0, 0);
        assert_eq!(product[0], (Q - entry[N - 1]) % Q);
        assert_eq!(product[1], entry[0]);
    }

    #[test]
    fn a_short_vector_of_the_wrong_length_is_refused() {
        let a = Matrix::expand(&[0; SEED_BYTES]);
        assert!(matches!(a.product(&[0; 7]), Err(Error::Length { .. })));
    }
}
