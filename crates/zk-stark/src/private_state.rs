//! Private contract state: a hidden value, updated by a public amount
//! (Master Prompt 27 §1).
//!
//! A contract keeps a field as a commitment `C = P(lo, hi, 0.. ‖ r)[..8]`;
//! the value and blinding never leave the owner's device. To debit it, the
//! owner proves on their device that
//!
//! ```text
//! C_old opens to v,  C_new opens to v - amount,  v - amount >= 0
//! ```
//!
//! with only `C_old`, `C_new` and `amount` public. The chain checks the proof
//! and stores `C_new`. A credit is the same statement with the two
//! commitments swapped: `C_old` opens to `v' - amount`.
//!
//! **No wraparound.** `BabyBear`'s modulus is about `2^31`, so a value is two
//! 29-bit limbs and the subtraction carries an explicit borrow bit:
//! `lo_old + borrow*2^29 = lo_new + lo_amt` and
//! `hi_old = hi_new + hi_amt + borrow`. Every term stays below `2^30`, far
//! under the modulus, so no equation can hold "mod p" without holding over
//! the integers. That caps a private field at `2^58` base units.
//!
//! Row 0 is the old commitment's permutation with the old value's bits and
//! the borrow; row 1 is the new commitment's with the new value's bits.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess as _};
use p3_field::{PrimeCharacteristicRing as _, PrimeField32 as _};
use p3_matrix::dense::RowMajorMatrix;

use crate::gadgets::poseidon::{self, PermAir};
use crate::gadgets::range::recompose;
use crate::hash::{DIGEST, Digest, F, WIDTH};
use crate::{Proof, ZkError};

/// Bits per limb: two limbs of 29 keep every sum below `2^30`.
pub const LIMB_BITS: usize = 29;
/// Largest value a private field holds, exclusive: `2^58`.
pub const VALUE_BITS: usize = 2 * LIMB_BITS;

const ROWS: usize = 2;
/// Extra columns: the value's bits, the borrow, then the amount's bits (the
/// last two on row 0 only).
const EXTRA: usize = VALUE_BITS + 1 + VALUE_BITS;
const BORROW: usize = poseidon::POSEIDON_COLS + VALUE_BITS;
const AMOUNT: usize = BORROW + 1;
/// Public values: `C_old`, `C_new`, the amount's two limbs.
const PUBLIC: usize = 2 * DIGEST + 2;

/// The transition AIR.
pub struct TransitionAir {
    perm: PermAir,
}

impl Default for TransitionAir {
    fn default() -> Self {
        Self {
            perm: poseidon::perm_air(),
        }
    }
}

impl BaseAir<F> for TransitionAir {
    fn width(&self) -> usize {
        poseidon::width_with(EXTRA)
    }

    fn num_public_values(&self) -> usize {
        PUBLIC
    }
}

fn limb_scale() -> F {
    F::from_u32(1 << LIMB_BITS)
}

impl<AB: AirBuilder<F = F>> Air<AB> for TransitionAir {
    fn eval(&self, builder: &mut AB) {
        poseidon::eval_permutation(&self.perm, builder);
        let main = builder.main();
        let (cur, next) = (main.current_slice().to_vec(), main.next_slice().to_vec());
        let public: Vec<AB::Expr> = builder.public_values().iter().map(|&v| v.into()).collect();
        let bits = |row: &[AB::Var]| {
            row[poseidon::POSEIDON_COLS..poseidon::POSEIDON_COLS + VALUE_BITS].to_vec()
        };
        let (old_bits, new_bits) = (bits(&cur), bits(&next));
        let borrow = cur[BORROW];
        let amount_bits = cur[AMOUNT..AMOUNT + VALUE_BITS].to_vec();

        let mut first = builder.when_first_row();
        for b in old_bits
            .iter()
            .chain(&new_bits)
            .chain(&amount_bits)
            .chain(core::iter::once(&borrow))
        {
            first.assert_bool(*b);
        }
        let (old_in, new_in) = (poseidon::inputs(&cur), poseidon::inputs(&next));
        let limbs = |bits: &[AB::Var]| {
            (
                recompose::<AB>(&bits[..LIMB_BITS]),
                recompose::<AB>(&bits[LIMB_BITS..]),
            )
        };
        let ((old_lo, old_hi), (new_lo, new_hi)) = (limbs(&old_bits), limbs(&new_bits));
        // Each commitment's input is its value's limbs, then zeros.
        for (input, (lo, hi)) in [(&old_in, (&old_lo, &old_hi)), (&new_in, (&new_lo, &new_hi))] {
            first.assert_eq(input[0], lo.clone());
            first.assert_eq(input[1], hi.clone());
            for i in &input[2..DIGEST] {
                first.assert_zero(*i);
            }
        }
        // Each commitment is its public value.
        let (old_d, new_d) = (poseidon::digest(&cur), poseidon::digest(&next));
        for (i, (o, n)) in old_d.iter().zip(&new_d).enumerate() {
            first.assert_eq(*o, public[i].clone());
            first.assert_eq(*n, public[DIGEST + i].clone());
        }
        // The public amount limbs are the amount's own bits: each below 2^29
        // in the circuit itself, not only in the Rust wrapper that computes
        // them. Without this a caller assembling public inputs directly could
        // pass a limb near p and satisfy the equations by wraparound (review).
        let (amt_lo, amt_hi) = (public[2 * DIGEST].clone(), public[2 * DIGEST + 1].clone());
        first.assert_eq(recompose::<AB>(&amount_bits[..LIMB_BITS]), amt_lo.clone());
        first.assert_eq(recompose::<AB>(&amount_bits[LIMB_BITS..]), amt_hi.clone());
        // old = new + amount, limb by limb with the borrow.
        let scale = AB::Expr::from(limb_scale());
        first.assert_eq(old_lo + borrow.into() * scale, new_lo + amt_lo);
        first.assert_eq(old_hi, new_hi + amt_hi + borrow.into());
    }
}

/// The two 29-bit limbs of `value`.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] at or above `2^58`.
pub fn limbs(value: u64) -> Result<[F; 2], ZkError> {
    if value >> VALUE_BITS != 0 {
        return Err(ZkError::Unsatisfied("private value at or above 2^58"));
    }
    let mask = (1u64 << LIMB_BITS) - 1;
    Ok([F::from_u64(value & mask), F::from_u64(value >> LIMB_BITS)])
}

fn limb_digest(value: u64) -> Result<Digest, ZkError> {
    let [lo, hi] = limbs(value)?;
    let mut d = [F::ZERO; DIGEST];
    d[0] = lo;
    d[1] = hi;
    Ok(d)
}

/// A commitment as 32 bytes: eight little-endian canonical words. What a
/// contract stores and passes to the host.
#[must_use]
pub fn to_bytes(commitment: &Digest) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(commitment) {
        *chunk = word.as_canonical_u32().to_le_bytes();
    }
    out
}

/// The inverse of [`to_bytes`]. Refuses a word at or above the modulus, so a
/// commitment has exactly one encoding.
///
/// # Errors
///
/// [`ZkError::Malformed`] for a non-canonical word.
pub fn from_bytes(bytes: &[u8; 32]) -> Result<Digest, ZkError> {
    let mut out = [F::ZERO; DIGEST];
    for (slot, chunk) in out.iter_mut().zip(bytes.as_chunks::<4>().0) {
        let word = u32::from_le_bytes(*chunk);
        if word >= crate::hash::MODULUS {
            return Err(ZkError::Malformed("non-canonical commitment word".into()));
        }
        *slot = F::from_u32(word);
    }
    Ok(out)
}

/// The commitment to `value` under `blind`.
///
/// # Errors
///
/// A value at or above `2^58`.
pub fn commit(value: u64, blind: &Digest) -> Result<Digest, ZkError> {
    Ok(crate::hash::compress(&limb_digest(value)?, blind))
}

fn public_values(old: &Digest, new: &Digest, amount: u64) -> Result<Vec<F>, ZkError> {
    let [lo, hi] = limbs(amount)?;
    Ok(old.iter().chain(new).copied().chain([lo, hi]).collect())
}

fn trace(
    old: u64,
    old_blind: &Digest,
    new: u64,
    new_blind: &Digest,
    borrow: bool,
    amount: u64,
) -> Result<RowMajorMatrix<F>, ZkError> {
    let bits_of = |v: u64| (0..VALUE_BITS).map(move |i| F::from_u64((v >> i) & 1));
    let old_extra: Vec<F> = bits_of(old)
        .chain([F::from_bool(borrow)])
        .chain(bits_of(amount))
        .collect();
    let new_extra: Vec<F> = bits_of(new)
        .chain(core::iter::repeat_n(F::ZERO, 1 + VALUE_BITS))
        .collect();
    let states: [[F; WIDTH]; 2] = [
        crate::gadgets::state(&limb_digest(old)?, old_blind),
        crate::gadgets::state(&limb_digest(new)?, new_blind),
    ];
    Ok(crate::gadgets::assemble(
        &states,
        &[old_extra, new_extra],
        EXTRA,
        ROWS,
    ))
}

/// Proves that the field committed as `commit(old, old_blind)` becomes
/// `commit(old - amount, new_blind)`. Returns the proof and the new
/// commitment.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] if `amount > old` or a value is out of range.
pub fn prove_debit(
    old: u64,
    old_blind: &Digest,
    amount: u64,
    new_blind: &Digest,
) -> Result<(Proof, Digest), ZkError> {
    let new = old
        .checked_sub(amount)
        .ok_or(ZkError::Unsatisfied("debit exceeds the private value"))?;
    let mask = (1u64 << LIMB_BITS) - 1;
    let borrow = (old & mask) < (amount & mask);
    let (c_old, c_new) = (commit(old, old_blind)?, commit(new, new_blind)?);
    let proof = crate::prove(
        &TransitionAir::default(),
        trace(old, old_blind, new, new_blind, borrow, amount)?,
        &public_values(&c_old, &c_new, amount)?,
    )?;
    Ok((proof, c_new))
}

/// Proves a credit: `commit(old, old_blind)` becomes
/// `commit(old + amount, new_blind)`. The same statement read backwards:
/// the new value, debited by `amount`, is the old one.
///
/// # Errors
///
/// As [`prove_debit`], or an overflow past `2^58`.
pub fn prove_credit(
    old: u64,
    old_blind: &Digest,
    amount: u64,
    new_blind: &Digest,
) -> Result<(Proof, Digest), ZkError> {
    let new = old
        .checked_add(amount)
        .ok_or(ZkError::Unsatisfied("credit overflows"))?;
    let (proof, _) = prove_debit(new, new_blind, amount, old_blind)?;
    Ok((proof, commit(new, new_blind)?))
}

/// Checks a debit: `new` hides `old`'s value minus `amount`, non-negative.
///
/// # Errors
///
/// [`ZkError::Rejected`] or [`ZkError::Malformed`].
pub fn verify_debit(proof: &Proof, old: &Digest, new: &Digest, amount: u64) -> Result<(), ZkError> {
    crate::verify(
        &TransitionAir::default(),
        proof,
        &public_values(old, new, amount)?,
    )
}

/// Checks a credit: `new` hides `old`'s value plus `amount`.
///
/// # Errors
///
/// As [`verify_debit`].
pub fn verify_credit(
    proof: &Proof,
    old: &Digest,
    new: &Digest,
    amount: u64,
) -> Result<(), ZkError> {
    verify_debit(proof, new, old, amount)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn blind(tag: u32) -> Digest {
        core::array::from_fn(|i| F::from_u32(tag * 97 + u32::try_from(i).unwrap()))
    }

    #[test]
    fn a_debit_proves_and_nothing_else_verifies() {
        let (old, b0, b1) = (5_000_000_000u64, blind(1), blind(2)); // crosses a limb
        let c_old = commit(old, &b0).unwrap();
        let (proof, c_new) = prove_debit(old, &b0, 1_234_567_891, &b1).unwrap();
        assert_eq!(c_new, commit(old - 1_234_567_891, &b1).unwrap());
        verify_debit(&proof, &c_old, &c_new, 1_234_567_891).unwrap();
        assert!(
            verify_debit(&proof, &c_old, &c_new, 1_234_567_890).is_err(),
            "another amount"
        );
        assert!(
            verify_debit(&proof, &c_new, &c_old, 1_234_567_891).is_err(),
            "swapped"
        );
        assert!(
            verify_credit(&proof, &c_old, &c_new, 1_234_567_891).is_err(),
            "a debit is not a credit"
        );
    }

    #[test]
    fn a_credit_round_trips_and_an_overdraft_cannot_be_proved() {
        let (b0, b1) = (blind(3), blind(4));
        let c_old = commit(10, &b0).unwrap();
        let (proof, c_new) = prove_credit(10, &b0, 32, &b1).unwrap();
        verify_credit(&proof, &c_old, &c_new, 32).unwrap();
        assert!(
            prove_debit(10, &b0, 11, &b1).is_err(),
            "no proof of a negative balance"
        );
        assert!(prove_debit(1 << 58, &b0, 1, &b1).is_err(), "out of range");
    }

    #[test]
    fn a_forged_trace_does_not_prove() {
        // Claim 10 - 20 = 2^58 - 10 by wrapping: the limbs and borrow cannot
        // satisfy the integer equations, so no proof comes out.
        let (b0, b1) = (blind(5), blind(6));
        let wrapped = (1u64 << VALUE_BITS) - 10;
        let c_old = commit(10, &b0).unwrap();
        let c_new = commit(wrapped, &b1).unwrap();
        let bad = trace(10, &b0, wrapped, &b1, true, 20).unwrap();
        let publics = public_values(&c_old, &c_new, 20).unwrap();
        // Debug builds check constraints and panic; release builds produce a
        // proof the verifier refuses. Either way it is not accepted — the
        // crate's other negative tests use the same pattern.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::prove(&TransitionAir::default(), bad, &publics)
        }));
        if let Ok(Ok(proof)) = outcome {
            assert!(verify_debit(&proof, &c_old, &c_new, 20).is_err());
        }
    }

    #[test]
    fn a_public_amount_limb_above_2_29_cannot_be_proved() {
        // Review, HIGH: the amount limbs are public inputs. With both values 0
        // and amt_hi = p - 1 the limb equation holds in the field, but the
        // in-circuit decomposition of the amount into 29-bit limbs refuses it.
        use p3_field::PrimeCharacteristicRing as _;
        let (b0, b1) = (blind(7), blind(8));
        let (c_old, c_new) = (commit(0, &b0).unwrap(), commit(0, &b1).unwrap());
        let mut publics = public_values(&c_old, &c_new, 0).unwrap();
        publics[2 * DIGEST + 1] = F::ZERO - F::ONE;
        let bad = trace(0, &b0, 0, &b1, false, 0).unwrap();
        let quiet = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::prove(&TransitionAir::default(), bad, &publics)
        }));
        std::panic::set_hook(quiet);
        if let Ok(Ok(proof)) = outcome {
            assert!(crate::verify(&TransitionAir::default(), &proof, &publics).is_err());
        }
    }
}
