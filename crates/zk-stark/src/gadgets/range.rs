//! Range proof: a committed value is below `2^bits`.
//!
//! The value is split into two 30-bit limbs so every limb, and every sum the
//! constraints form, stays below the BabyBear modulus (`p ≈ 2^31`) — there is
//! no wraparound for a prover to hide an out-of-range value in. The
//! commitment is `C = P(lo, hi, 0, 0, 0, 0, 0, 0 ‖ r)[..8]` with an 8-element
//! blinding `r`, so the proof reveals `C` and nothing else about the value.
//!
//! Row 0 carries the statement; row 1 is an honest permutation of zero that
//! only exists because the hiding PCS needs more than one row.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess as _};
use p3_field::PrimeCharacteristicRing as _;
use p3_matrix::dense::RowMajorMatrix;

use super::poseidon::{self, PermAir};
use crate::hash::{DIGEST, Digest, F, WIDTH};
use crate::{Proof, ZkError};

/// Bits per limb.
pub const LIMB_BITS: usize = 30;
/// Most bits this gadget can bound: two limbs.
pub const MAX_BITS: usize = 2 * LIMB_BITS;

const ROWS: usize = 2;
const EXTRA: usize = MAX_BITS;

/// The range AIR.
pub struct RangeAir {
    bits: usize,
    perm: PermAir,
}

impl RangeAir {
    /// A range proof for values below `2^bits`.
    ///
    /// # Errors
    ///
    /// [`ZkError::Unsatisfied`] if `bits` is 0 or above [`MAX_BITS`].
    pub fn new(bits: usize) -> Result<Self, ZkError> {
        if bits == 0 || bits > MAX_BITS {
            return Err(ZkError::Unsatisfied("range bits must be 1..=60"));
        }
        Ok(Self {
            bits,
            perm: poseidon::perm_air(),
        })
    }
}

impl BaseAir<F> for RangeAir {
    fn width(&self) -> usize {
        poseidon::width_with(EXTRA)
    }

    fn num_public_values(&self) -> usize {
        DIGEST
    }
}

/// Recomposes `bits[..n]` little-endian.
pub(crate) fn recompose<AB: AirBuilder<F = F>>(bits: &[AB::Var]) -> AB::Expr {
    bits.iter().enumerate().fold(AB::Expr::ZERO, |acc, (i, b)| {
        acc + (*b).into() * AB::Expr::from(F::from_u32(1 << i))
    })
}

impl<AB: AirBuilder<F = F>> Air<AB> for RangeAir {
    fn eval(&self, builder: &mut AB) {
        poseidon::eval_permutation(&self.perm, builder);
        let main = builder.main();
        let row = main.current_slice();
        let inputs = poseidon::inputs(row);
        let digest = poseidon::digest(row);
        let bits: Vec<AB::Var> = row[poseidon::POSEIDON_COLS..].to_vec();
        let public: Vec<AB::Expr> = builder.public_values().iter().map(|&v| v.into()).collect();

        let mut first = builder.when_first_row();
        for b in &bits {
            first.assert_bool(*b);
        }
        // Bits at or above the bound are zero: the value is below 2^bits.
        for b in &bits[self.bits..] {
            first.assert_zero(*b);
        }
        first.assert_eq(inputs[0], recompose::<AB>(&bits[..LIMB_BITS]));
        first.assert_eq(inputs[1], recompose::<AB>(&bits[LIMB_BITS..]));
        for input in &inputs[2..DIGEST] {
            first.assert_zero(*input);
        }
        for (d, p) in digest.iter().zip(public) {
            first.assert_eq(*d, p);
        }
    }
}

/// The two limbs of `value`, which must be below `2^60`.
#[must_use]
pub fn limbs(value: u64) -> [F; 2] {
    let mask = (1u64 << LIMB_BITS) - 1;
    [F::from_u64(value & mask), F::from_u64(value >> LIMB_BITS)]
}

/// `C = P(lo, hi, 0.. ‖ blind)[..8]`.
#[must_use]
pub fn commit(value: u64, blind: &Digest) -> Digest {
    crate::hash::compress(&limb_digest(value), blind)
}

fn limb_digest(value: u64) -> Digest {
    let [lo, hi] = limbs(value);
    let mut d = [F::ZERO; DIGEST];
    d[0] = lo;
    d[1] = hi;
    d
}

/// The trace for `value` and `blind`, without any check — the negative tests
/// build bad traces through this.
#[must_use]
pub fn trace(value: u64, blind: &Digest) -> RowMajorMatrix<F> {
    let state: [F; WIDTH] = super::state(&limb_digest(value), blind);
    let bits: Vec<F> = (0..MAX_BITS)
        .map(|i| F::from_u64((value >> i) & 1))
        .collect();
    super::assemble(&[state], &[bits], EXTRA, ROWS)
}

/// Proves `value < 2^bits` for the commitment `commit(value, blind)`.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] if the value is out of range.
pub fn prove(value: u64, blind: &Digest, bits: usize) -> Result<(Proof, Digest), ZkError> {
    let air = RangeAir::new(bits)?;
    if value >> bits != 0 {
        return Err(ZkError::Unsatisfied("value is not below 2^bits"));
    }
    let commitment = commit(value, blind);
    let proof = crate::prove(&air, trace(value, blind), &commitment)?;
    Ok((proof, commitment))
}

/// Verifies that `commitment` hides a value below `2^bits`.
///
/// # Errors
///
/// [`ZkError::Rejected`] or [`ZkError::Malformed`].
pub fn verify(proof: &Proof, commitment: &Digest, bits: usize) -> Result<(), ZkError> {
    crate::verify(&RangeAir::new(bits)?, proof, commitment)
}
