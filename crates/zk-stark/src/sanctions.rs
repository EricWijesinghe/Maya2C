//! Proving an identifier is **not** on a sanctions list, without revealing it.
//!
//! > I know a 32-byte identifier `x` and two leaves `lo < x < hi` at adjacent
//! > positions `i`, `i + 1` of the sorted list committed to by `root`.
//!
//! Adjacency is what makes a gap a proof: two *neighbours* of a sorted list
//! leave no room for `x` between them. The two sentinels (all-zero and
//! all-`0xff`) make every identifier bracketable. Identifiers compare as
//! big-endian bytes — `<[u8; 32]>::cmp` natively — held in the AIR as sixteen
//! 16-bit limbs, and `x − lo − 1 ≥ 0` and `hi − x − 1 ≥ 0` are proved by
//! limb-wise borrow chains whose differences are range-checked, so a negative
//! result cannot hide in the field.
//!
//! What this does not prove: that `x` is the party in the payment (bind it
//! by committing to `x` in the payment), or that `root` is the right list
//! (the verifier must know which root it expects).

use std::borrow::Cow;

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess as _};
use p3_field::PrimeCharacteristicRing as _;

use crate::dual::{self, S_LAST, S_ROW0, S_ROW1, STATEMENT};
use crate::gadgets::merkle::MerklePath;
use crate::gadgets::poseidon::{self, PermAir};
use crate::gadgets::range::recompose;
use crate::hash::{DIGEST, Digest, F, WIDTH, compress, permute};
use crate::pool::tree::merkle_path;
use crate::{Proof, ZkError};

/// Bytes in an identifier.
pub const IDENTIFIER_BYTES: usize = 32;
/// An identifier.
pub type Identifier = [u8; IDENTIFIER_BYTES];

const LIMBS: usize = 16;
const LIMB_BITS: usize = 16;
const X: usize = STATEMENT;
const LO: usize = X + LIMBS;
const HI: usize = LO + LIMBS;
const BORROW_L: usize = HI + LIMBS;
const BORROW_H: usize = BORROW_L + LIMBS;
const DL: usize = BORROW_H + LIMBS;
const DH: usize = DL + LIMBS;
const XBITS: usize = DH + LIMBS;
const DLBITS: usize = XBITS + LIMBS * LIMB_BITS;
const DHBITS: usize = DLBITS + LIMBS * LIMB_BITS;
const END: usize = DHBITS + LIMBS * LIMB_BITS;

const LOW_SENTINEL: Identifier = [0x00; IDENTIFIER_BYTES];
const HIGH_SENTINEL: Identifier = [0xff; IDENTIFIER_BYTES];

fn limbs(id: &Identifier) -> [u32; LIMBS] {
    core::array::from_fn(|k| u32::from(u16::from_be_bytes([id[2 * k], id[2 * k + 1]])))
}

fn state_of(id: &Identifier) -> [F; WIDTH] {
    limbs(id).map(F::from_u32)
}

/// The tree leaf for an identifier: `compress(P(limbs)[..8], 0)`.
#[must_use]
pub fn leaf(id: &Identifier) -> Digest {
    let d0: Digest = permute(state_of(id))[..DIGEST].try_into().expect("8");
    compress(&d0, &[F::ZERO; DIGEST])
}

/// A published list: sorted, deduplicated, with both sentinels.
#[derive(Clone, Debug)]
pub struct SanctionsList {
    entries: Vec<Identifier>,
}

impl SanctionsList {
    /// Builds a list. Sorting here, rather than trusting the caller, is what
    /// makes adjacency meaningful.
    ///
    /// # Errors
    ///
    /// [`ZkError::Unsatisfied`] for a sentinel entry or a list too big for a
    /// depth-16 tree.
    pub fn build(entries: impl IntoIterator<Item = Identifier>) -> Result<Self, ZkError> {
        let mut all: Vec<Identifier> = entries.into_iter().collect();
        if all
            .iter()
            .any(|e| *e == LOW_SENTINEL || *e == HIGH_SENTINEL)
        {
            return Err(ZkError::Unsatisfied("an entry equals a sentinel"));
        }
        all.push(LOW_SENTINEL);
        all.push(HIGH_SENTINEL);
        all.sort_unstable();
        all.dedup();
        if all.len() > 1 << dual::DEPTH {
            return Err(ZkError::Unsatisfied("list does not fit a depth-16 tree"));
        }
        Ok(Self { entries: all })
    }

    /// Whether `id` is listed.
    #[must_use]
    pub fn contains(&self, id: &Identifier) -> bool {
        self.entries.binary_search(id).is_ok()
    }

    fn leaves(&self) -> Vec<Digest> {
        self.entries.iter().map(leaf).collect()
    }

    fn path(&self, index: usize) -> Result<MerklePath, ZkError> {
        let full = merkle_path(&self.leaves(), index as u64)?;
        Ok(MerklePath {
            siblings: full.siblings[..dual::DEPTH].to_vec(),
            index: full.index,
        })
    }

    /// The root proofs are checked against.
    ///
    /// # Errors
    ///
    /// Only if the list is empty, which `build` prevents.
    pub fn root(&self) -> Result<Digest, ZkError> {
        Ok(self.path(0)?.root(&leaf(&self.entries[0])))
    }

    /// The witness that `id` is absent.
    ///
    /// # Errors
    ///
    /// [`ZkError::Unsatisfied`] if `id` **is** listed — there is no gap.
    pub fn absence_witness(&self, id: &Identifier) -> Result<AbsenceWitness, ZkError> {
        let Err(at) = self.entries.binary_search(id) else {
            return Err(ZkError::Unsatisfied("the identifier is on the list"));
        };
        let (lo, hi) = (at - 1, at);
        Ok(AbsenceWitness {
            identifier: *id,
            lo: self.entries[lo],
            hi: self.entries[hi],
            lo_path: self.path(lo)?,
            hi_path: self.path(hi)?,
        })
    }
}

/// The prover's private inputs.
#[derive(Clone, Debug)]
pub struct AbsenceWitness {
    /// The identifier being cleared.
    pub identifier: Identifier,
    /// The listed entry just below it.
    pub lo: Identifier,
    /// The listed entry just above it.
    pub hi: Identifier,
    /// `lo`'s path.
    pub lo_path: MerklePath,
    /// `hi`'s path.
    pub hi_path: MerklePath,
}

/// The absence AIR. Public values: the root.
pub struct AbsenceAir {
    perm: PermAir,
    selectors: Vec<Vec<F>>,
}

impl Default for AbsenceAir {
    fn default() -> Self {
        Self {
            perm: poseidon::perm_air(),
            selectors: dual::selectors(),
        }
    }
}

impl BaseAir<F> for AbsenceAir {
    fn width(&self) -> usize {
        END
    }
    fn num_public_values(&self) -> usize {
        DIGEST
    }
    fn num_periodic_columns(&self) -> usize {
        dual::SELECTORS
    }
    fn periodic_columns(&self) -> Cow<'_, [Vec<F>]> {
        dual::periodic(&self.selectors)
    }
}

type E<AB> = <AB as AirBuilder>::Expr;

impl<AB: AirBuilder<F = F>> Air<AB> for AbsenceAir {
    fn eval(&self, b: &mut AB) {
        let main = b.main();
        let (row, next) = (main.current_slice().to_vec(), main.next_slice().to_vec());
        let sel: Vec<E<AB>> = b.periodic_values().iter().map(|&p| p.into()).collect();
        let public: Vec<E<AB>> = b.public_values().iter().map(|&p| p.into()).collect();
        dual::eval_paths(&self.perm, b, &row, &next, &sel);
        let (a, h) = (
            dual::path_row(&row, dual::PERM_A),
            dual::path_row(&row, dual::PERM_B),
        );
        let v = |c: usize| -> E<AB> { row[c].into() };

        let mut t = b.when_transition();
        for c in STATEMENT..END {
            t.assert_eq(next[c], row[c]);
        }
        for k in 0..WIDTH {
            b.assert_zero(sel[S_ROW0].clone() * (E::<AB>::from(a.inputs[k]) - v(LO + k)));
            b.assert_zero(sel[S_ROW0].clone() * (E::<AB>::from(h.inputs[k]) - v(HI + k)));
        }
        for k in 0..DIGEST {
            b.assert_zero(sel[S_ROW1].clone() * E::<AB>::from(a.inputs[DIGEST + k]));
            b.assert_zero(sel[S_ROW1].clone() * E::<AB>::from(h.inputs[DIGEST + k]));
            b.assert_zero(sel[S_LAST].clone() * (E::<AB>::from(a.digest[k]) - public[k].clone()));
            b.assert_zero(sel[S_LAST].clone() * (E::<AB>::from(h.digest[k]) - public[k].clone()));
        }
        // Adjacent: hi's index is lo's plus one.
        b.assert_zero(sel[S_LAST].clone() * (v(dual::ACC_B) - v(dual::ACC_A) - E::<AB>::ONE));

        let mut first = b.when_first_row();
        for bit in &row[XBITS..END] {
            first.assert_bool(*bit);
        }
        for k in 0..LIMBS {
            let bits = |base: usize| &row[base + k * LIMB_BITS..base + (k + 1) * LIMB_BITS];
            first.assert_eq(row[X + k], recompose::<AB>(bits(XBITS)));
            first.assert_eq(row[DL + k], recompose::<AB>(bits(DLBITS)));
            first.assert_eq(row[DH + k], recompose::<AB>(bits(DHBITS)));
            first.assert_bool(row[BORROW_L + k]);
            first.assert_bool(row[BORROW_H + k]);
        }
        // x − lo − 1 and hi − x − 1, least significant limb (15) first.
        let radix = E::<AB>::from(F::from_u32(1 << LIMB_BITS));
        for (big, small, borrow, diff) in [(X, LO, BORROW_L, DL), (HI, X, BORROW_H, DH)] {
            for k in (0..LIMBS).rev() {
                let borrow_in = if k == LIMBS - 1 {
                    E::<AB>::ONE
                } else {
                    v(borrow + k + 1)
                };
                first.assert_eq(
                    v(big + k) - v(small + k) - borrow_in + v(borrow + k) * radix.clone(),
                    v(diff + k),
                );
            }
            // No borrow out of the most significant limb: the difference is ≥ 0.
            first.assert_zero(v(borrow));
        }
    }
}

/// The subtraction `a − b − 1` limb-wise: (borrows, differences).
fn subtract(a: &[u32; LIMBS], b: &[u32; LIMBS]) -> ([u32; LIMBS], [u32; LIMBS]) {
    let (mut borrows, mut diffs) = ([0u32; LIMBS], [0u32; LIMBS]);
    let mut borrow_in = 1i64;
    for k in (0..LIMBS).rev() {
        let mut d = i64::from(a[k]) - i64::from(b[k]) - borrow_in;
        let out = i64::from(d < 0);
        d += out << LIMB_BITS;
        borrows[k] = u32::try_from(out).expect("0 or 1");
        diffs[k] = u32::try_from(d).expect("below 2^16");
        borrow_in = out;
    }
    (borrows, diffs)
}

/// The trace, without checks.
#[must_use]
pub fn trace(w: &AbsenceWitness) -> p3_matrix::dense::RowMajorMatrix<F> {
    let (x, lo, hi) = (limbs(&w.identifier), limbs(&w.lo), limbs(&w.hi));
    let (bl, dl) = subtract(&x, &lo);
    let (bh, dh) = subtract(&hi, &x);
    let mut s: Vec<F> = [x, lo, hi, bl, bh, dl, dh]
        .iter()
        .flat_map(|a| a.map(F::from_u32))
        .collect();
    for group in [x, dl, dh] {
        for limb in group {
            s.extend((0..LIMB_BITS).map(|i| F::from_bool((limb >> i) & 1 == 1)));
        }
    }
    dual::trace(
        (state_of(&w.lo), [F::ZERO; DIGEST], &w.lo_path),
        (state_of(&w.hi), [F::ZERO; DIGEST], &w.hi_path),
        &s,
    )
}

/// Proves `w.identifier` is absent from the list with `root`.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] unless `lo < x < hi`, both are under `root`, and
/// they are adjacent.
pub fn prove(w: &AbsenceWitness, root: &Digest) -> Result<Proof, ZkError> {
    if !(w.lo < w.identifier && w.identifier < w.hi) {
        return Err(ZkError::Unsatisfied(
            "the identifier is not between lo and hi",
        ));
    }
    if w.hi_path.index != w.lo_path.index + 1
        || w.lo_path.root(&leaf(&w.lo)) != *root
        || w.hi_path.root(&leaf(&w.hi)) != *root
    {
        return Err(ZkError::Unsatisfied(
            "lo and hi are not adjacent leaves under the root",
        ));
    }
    crate::prove(&AbsenceAir::default(), trace(w), root)
}

/// Verifies an absence proof against `root`.
///
/// # Errors
///
/// [`ZkError::Rejected`] or [`ZkError::Malformed`].
pub fn verify(proof: &Proof, root: &Digest) -> Result<(), ZkError> {
    crate::verify(&AbsenceAir::default(), proof, root)
}
