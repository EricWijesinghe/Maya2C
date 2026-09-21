//! The mint AIR: public value in, a hidden note out.
//!
//! Two rows: row 0 is the value layer `lo, hi, 0.. ‖ pk`, row 1 the
//! commitment `layer ‖ rho ‖ r`. Public values: `cm(8) ‖ lo ‖ hi`. The value
//! is public (it is leaving a transparent balance), so its range is checked
//! natively by the verifier; the owner and nonces stay hidden.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess as _};
use p3_matrix::dense::RowMajorMatrix;

use crate::gadgets::poseidon::{self, PermAir};
use crate::hash::{DIGEST, F};

use super::note::{NONCE, Note, value_digest};

const PK: usize = 0;
const RHO: usize = PK + DIGEST;
const R: usize = RHO + NONCE;
const EXTRA: usize = R + NONCE;
/// Public values: `cm ‖ lo ‖ hi`.
pub const PUBLIC: usize = DIGEST + 2;

/// The mint AIR.
pub struct MintAir {
    perm: PermAir,
}

impl Default for MintAir {
    fn default() -> Self {
        Self {
            perm: poseidon::perm_air(),
        }
    }
}

impl BaseAir<F> for MintAir {
    fn width(&self) -> usize {
        poseidon::width_with(EXTRA)
    }

    fn num_public_values(&self) -> usize {
        PUBLIC
    }
}

impl<AB: AirBuilder<F = F>> Air<AB> for MintAir {
    fn eval(&self, builder: &mut AB) {
        poseidon::eval_permutation(&self.perm, builder);
        let main = builder.main();
        let (row, next) = (main.current_slice().to_vec(), main.next_slice().to_vec());
        let (x, nx) = (
            &row[poseidon::POSEIDON_COLS..],
            &next[poseidon::POSEIDON_COLS..],
        );
        let (inp, dig, next_inp) = (
            poseidon::inputs(&row),
            poseidon::digest(&row),
            poseidon::inputs(&next),
        );
        let public: Vec<AB::Expr> = builder.public_values().iter().map(|&p| p.into()).collect();

        let mut t = builder.when_transition();
        for c in 0..EXTRA {
            t.assert_eq(nx[c], x[c]);
        }
        for j in 0..DIGEST {
            t.assert_eq(next_inp[j], dig[j]);
        }

        let mut first = builder.when_first_row();
        first.assert_eq(inp[0], public[DIGEST].clone());
        first.assert_eq(inp[1], public[DIGEST + 1].clone());
        for j in 2..DIGEST {
            first.assert_zero(inp[j]);
        }
        for j in 0..DIGEST {
            first.assert_eq(inp[DIGEST + j], x[PK + j]);
        }

        let mut last = builder.when_last_row();
        for j in 0..NONCE {
            last.assert_eq(inp[DIGEST + j], x[RHO + j]);
            last.assert_eq(inp[DIGEST + NONCE + j], x[R + j]);
        }
        for j in 0..DIGEST {
            last.assert_eq(dig[j], public[j].clone());
        }
    }
}

/// The mint trace for `note`, without any check.
#[must_use]
pub fn trace(note: &Note) -> RowMajorMatrix<F> {
    let mut regs = note.owner.to_vec();
    regs.extend_from_slice(&note.rho);
    regs.extend_from_slice(&note.blind);
    let layer = crate::gadgets::state(&value_digest(note.value), &note.owner);
    let commit = crate::gadgets::state(&note.value_layer(), &note.nonce_digest());
    crate::gadgets::assemble(&[layer, commit], &[regs.clone(), regs], EXTRA, 2)
}

/// `cm ‖ lo ‖ hi`.
#[must_use]
pub fn public(note: &Note) -> Vec<F> {
    let mut out = note.commitment().to_vec();
    out.extend_from_slice(&value_digest(note.value)[..2]);
    out
}
