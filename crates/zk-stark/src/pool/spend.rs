//! The spend AIR: transfer (note → note) and unshield (note → public value).
//!
//! 32 rows, one Poseidon2 permutation each, roles fixed by *periodic*
//! selector columns — constants the verifier supplies, so a prover cannot
//! switch a check off by choosing a witness:
//!
//! | row | role | permutation input |
//! |---|---|---|
//! | 0 | owner key | `sk ‖ KEY` → `pk` |
//! | 1 | input value layer | `lo, hi, 0.. ‖ pk` (pk from row 0) |
//! | 2 | input commitment | `layer ‖ rho ‖ r` → `cm_in` |
//! | 3–18 | Merkle path, 16 levels | `cm_in` up to the root |
//! | 19 | nullifier | `sk ‖ NULLIFIER, rho, 0..` → `nf` |
//! | 20 | output value layer (transfer) | `lo', hi', 0.. ‖ pk'` |
//! | 21 | output commitment (transfer) | `layer' ‖ rho' ‖ r'` → `cm_out` |
//! | 22–31 | padding | honest permutations, nothing else checked |
//!
//! The witness registers (`sk`, the input note's fields, the output note's
//! fields, the carry) are constant down the trace; the value limbs are
//! range-checked by bit decomposition on row 0; value is conserved limb-wise
//! with an explicit carry, so no equation can wrap the modulus.
//!
//! Public values: `root(8) ‖ nf(8) ‖ out(8) ‖ fee`, where `out` is `cm_out`
//! for a transfer and `(lo, hi, 0..)` of the unshielded amount otherwise.

use std::borrow::Cow;

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess as _};
use p3_field::PrimeCharacteristicRing as _;

use crate::gadgets::poseidon::{self, PermAir};
use crate::gadgets::range::{LIMB_BITS, MAX_BITS, recompose};
use crate::gadgets::{Domain, domain};
use crate::hash::{DIGEST, F};

use super::note::NONCE;
use super::tree::DEPTH;

/// Trace rows.
pub const ROWS: usize = 32;
const ROW_KEY: usize = 0;
const ROW_LAYER_IN: usize = 1;
const ROW_CM_IN: usize = 2;
const ROW_MERKLE: usize = 3;
const ROW_ROOT: usize = ROW_MERKLE + DEPTH - 1;
const ROW_NF: usize = ROW_ROOT + 1;
const ROW_LAYER_OUT: usize = ROW_NF + 1;
const ROW_CM_OUT: usize = ROW_LAYER_OUT + 1;

// Register columns, after the permutation columns.
pub(crate) const SK: usize = 0;
pub(crate) const RHO_IN: usize = SK + DIGEST;
pub(crate) const R_IN: usize = RHO_IN + NONCE;
pub(crate) const VIN: usize = R_IN + NONCE; // lo, hi
pub(crate) const PK_OUT: usize = VIN + 2;
pub(crate) const RHO_OUT: usize = PK_OUT + DIGEST;
pub(crate) const R_OUT: usize = RHO_OUT + NONCE;
pub(crate) const VOUT: usize = R_OUT + NONCE; // lo, hi
pub(crate) const CARRY: usize = VOUT + 2;
pub(crate) const REGISTERS: usize = CARRY + 1;
// Per-row columns.
pub(crate) const BIT: usize = REGISTERS;
pub(crate) const SIB: usize = BIT + 1;
pub(crate) const BITS_IN: usize = SIB + DIGEST;
pub(crate) const BITS_OUT: usize = BITS_IN + MAX_BITS;
/// Gadget columns after the permutation.
pub const EXTRA: usize = BITS_OUT + MAX_BITS;

/// Public-value offsets.
pub const PUB_ROOT: usize = 0;
/// Nullifier.
pub const PUB_NF: usize = DIGEST;
/// Output commitment, or the unshielded amount's limbs.
pub const PUB_OUT: usize = 2 * DIGEST;
/// Fee.
pub const PUB_FEE: usize = 3 * DIGEST;
/// Number of public values.
pub const PUBLIC: usize = PUB_FEE + 1;

/// What the spend produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A new shielded note.
    Transfer,
    /// A public amount leaving the pool.
    Unshield,
}

// Selector indices.
const S_KEY: usize = 0;
const S_LAYER_IN: usize = 1;
const S_CM_IN: usize = 2;
const T_TO_LAYER: usize = 3; // local row feeds next row's right half (pk)
const T_TO_LEFT: usize = 4; // local digest is next row's left half
const T_MERKLE: usize = 5; // next row is a Merkle level fed by local digest
const S_MERKLE: usize = 6;
const S_ROOT: usize = 7;
const S_NF: usize = 8;
const S_LAYER_OUT: usize = 9;
const S_CM_OUT: usize = 10;
const SELECTORS: usize = 11;

/// The spend AIR.
pub struct SpendAir {
    mode: Mode,
    perm: PermAir,
    selectors: Vec<Vec<F>>,
}

fn column(rows: impl Fn(usize) -> bool) -> Vec<F> {
    (0..ROWS).map(|r| F::from_bool(rows(r))).collect()
}

impl SpendAir {
    /// The AIR for `mode`.
    #[must_use]
    pub fn new(mode: Mode) -> Self {
        let out = mode == Mode::Transfer;
        let mut selectors = vec![Vec::new(); SELECTORS];
        selectors[S_KEY] = column(|r| r == ROW_KEY);
        selectors[S_LAYER_IN] = column(|r| r == ROW_LAYER_IN);
        selectors[S_CM_IN] = column(|r| r == ROW_CM_IN);
        selectors[T_TO_LAYER] = column(|r| r == ROW_KEY);
        selectors[T_TO_LEFT] = column(|r| r == ROW_LAYER_IN || (out && r == ROW_LAYER_OUT));
        selectors[T_MERKLE] = column(|r| (ROW_CM_IN..ROW_ROOT).contains(&r));
        selectors[S_MERKLE] = column(|r| (ROW_MERKLE..=ROW_ROOT).contains(&r));
        selectors[S_ROOT] = column(|r| r == ROW_ROOT);
        selectors[S_NF] = column(|r| r == ROW_NF);
        selectors[S_LAYER_OUT] = column(|r| out && r == ROW_LAYER_OUT);
        selectors[S_CM_OUT] = column(|r| out && r == ROW_CM_OUT);
        Self {
            mode,
            perm: poseidon::perm_air(),
            selectors,
        }
    }

    /// The mode.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }
}

impl BaseAir<F> for SpendAir {
    fn width(&self) -> usize {
        poseidon::width_with(EXTRA)
    }

    fn num_public_values(&self) -> usize {
        PUBLIC
    }

    fn num_periodic_columns(&self) -> usize {
        SELECTORS
    }

    fn periodic_columns(&self) -> Cow<'_, [Vec<F>]> {
        Cow::Borrowed(&self.selectors)
    }
}

type E<AB> = <AB as AirBuilder>::Expr;

/// `sel · (a − b) = 0` for each pair.
fn when_eq<AB: AirBuilder<F = F>>(
    builder: &mut AB,
    sel: &E<AB>,
    pairs: impl IntoIterator<Item = (E<AB>, E<AB>)>,
) {
    for (a, b) in pairs {
        builder.assert_zero(sel.clone() * (a - b));
    }
}

fn v<AB: AirBuilder<F = F>>(x: AB::Var) -> E<AB> {
    x.into()
}

impl<AB: AirBuilder<F = F>> Air<AB> for SpendAir {
    fn eval(&self, builder: &mut AB) {
        poseidon::eval_permutation(&self.perm, builder);
        let main = builder.main();
        let (row, next) = (main.current_slice().to_vec(), main.next_slice().to_vec());
        let sel: Vec<E<AB>> = builder
            .periodic_values()
            .iter()
            .map(|&p| p.into())
            .collect();
        let public: Vec<E<AB>> = builder.public_values().iter().map(|&p| p.into()).collect();
        let (x, nx) = (
            &row[poseidon::POSEIDON_COLS..],
            &next[poseidon::POSEIDON_COLS..],
        );
        let (inp, dig) = (poseidon::inputs(&row), poseidon::digest(&row));
        let (next_inp, zero) = (poseidon::inputs(&next), E::<AB>::ZERO);

        // Registers are constant down the trace.
        let mut transition = builder.when_transition();
        for c in 0..REGISTERS {
            transition.assert_eq(nx[c], x[c]);
        }

        self.eval_input_note(builder, &sel, &inp, x);
        self.eval_links(builder, &sel, &dig, &next_inp, nx);
        self.eval_nullifier_and_root(builder, &sel, &inp, &dig, x, &public);
        if self.mode == Mode::Transfer {
            eval_output_note(builder, &sel, &inp, &dig, x, &public);
        }
        eval_values(builder, x, &public, self.mode, &zero);
    }
}

impl SpendAir {
    fn eval_input_note<AB: AirBuilder<F = F>>(
        &self,
        b: &mut AB,
        sel: &[E<AB>],
        inp: &[AB::Var; 16],
        x: &[AB::Var],
    ) {
        let key = domain(Domain::Key);
        when_eq::<AB>(
            b,
            &sel[S_KEY],
            (0..DIGEST).map(|j| (v::<AB>(inp[j]), v::<AB>(x[SK + j]))),
        );
        when_eq::<AB>(
            b,
            &sel[S_KEY],
            (0..DIGEST).map(|j| (v::<AB>(inp[DIGEST + j]), E::<AB>::from(key[j]))),
        );
        when_eq::<AB>(
            b,
            &sel[S_LAYER_IN],
            [
                (v::<AB>(inp[0]), v::<AB>(x[VIN])),
                (v::<AB>(inp[1]), v::<AB>(x[VIN + 1])),
            ],
        );
        when_eq::<AB>(
            b,
            &sel[S_LAYER_IN],
            (2..DIGEST).map(|j| (v::<AB>(inp[j]), E::<AB>::ZERO)),
        );
        when_eq::<AB>(
            b,
            &sel[S_CM_IN],
            (0..NONCE).map(|j| (v::<AB>(inp[DIGEST + j]), v::<AB>(x[RHO_IN + j]))),
        );
        when_eq::<AB>(
            b,
            &sel[S_CM_IN],
            (0..NONCE).map(|j| (v::<AB>(inp[DIGEST + NONCE + j]), v::<AB>(x[R_IN + j]))),
        );
    }

    /// Row-to-row wiring: `pk` into the value layer, a layer into its
    /// commitment, and each Merkle level from the one below.
    fn eval_links<AB: AirBuilder<F = F>>(
        &self,
        b: &mut AB,
        sel: &[E<AB>],
        dig: &[AB::Var; DIGEST],
        next_inp: &[AB::Var; 16],
        nx: &[AB::Var],
    ) {
        let mut t = b.when_transition();
        when_eq::<_>(
            &mut t,
            &sel[T_TO_LAYER],
            (0..DIGEST).map(|j| (v::<AB>(next_inp[DIGEST + j]), v::<AB>(dig[j]))),
        );
        when_eq::<_>(
            &mut t,
            &sel[T_TO_LEFT],
            (0..DIGEST).map(|j| (v::<AB>(next_inp[j]), v::<AB>(dig[j]))),
        );
        let bit = v::<AB>(nx[BIT]);
        for j in 0..DIGEST {
            let (cur, sib) = (v::<AB>(dig[j]), v::<AB>(nx[SIB + j]));
            let left = cur.clone() + bit.clone() * (sib.clone() - cur.clone());
            let right = sib.clone() + bit.clone() * (cur - sib);
            t.assert_zero(sel[T_MERKLE].clone() * (v::<AB>(next_inp[j]) - left));
            t.assert_zero(sel[T_MERKLE].clone() * (v::<AB>(next_inp[DIGEST + j]) - right));
        }
    }

    fn eval_nullifier_and_root<AB: AirBuilder<F = F>>(
        &self,
        b: &mut AB,
        sel: &[E<AB>],
        inp: &[AB::Var; 16],
        dig: &[AB::Var; DIGEST],
        x: &[AB::Var],
        public: &[E<AB>],
    ) {
        let bit = v::<AB>(x[BIT]);
        b.assert_zero(sel[S_MERKLE].clone() * bit.clone() * (bit - E::<AB>::ONE));
        when_eq::<AB>(
            b,
            &sel[S_ROOT],
            (0..DIGEST).map(|j| (v::<AB>(dig[j]), public[PUB_ROOT + j].clone())),
        );
        let tag = domain(Domain::Nullifier)[0];
        when_eq::<AB>(
            b,
            &sel[S_NF],
            (0..DIGEST).map(|j| (v::<AB>(inp[j]), v::<AB>(x[SK + j]))),
        );
        when_eq::<AB>(b, &sel[S_NF], [(v::<AB>(inp[DIGEST]), E::<AB>::from(tag))]);
        when_eq::<AB>(
            b,
            &sel[S_NF],
            (0..NONCE).map(|j| (v::<AB>(inp[DIGEST + 1 + j]), v::<AB>(x[RHO_IN + j]))),
        );
        when_eq::<AB>(
            b,
            &sel[S_NF],
            (DIGEST + 1 + NONCE..16).map(|j| (v::<AB>(inp[j]), E::<AB>::ZERO)),
        );
        when_eq::<AB>(
            b,
            &sel[S_NF],
            (0..DIGEST).map(|j| (v::<AB>(dig[j]), public[PUB_NF + j].clone())),
        );
    }
}

fn eval_output_note<AB: AirBuilder<F = F>>(
    b: &mut AB,
    sel: &[E<AB>],
    inp: &[AB::Var; 16],
    dig: &[AB::Var; DIGEST],
    x: &[AB::Var],
    public: &[E<AB>],
) {
    when_eq::<AB>(
        b,
        &sel[S_LAYER_OUT],
        [
            (v::<AB>(inp[0]), v::<AB>(x[VOUT])),
            (v::<AB>(inp[1]), v::<AB>(x[VOUT + 1])),
        ],
    );
    when_eq::<AB>(
        b,
        &sel[S_LAYER_OUT],
        (2..DIGEST).map(|j| (v::<AB>(inp[j]), E::<AB>::ZERO)),
    );
    when_eq::<AB>(
        b,
        &sel[S_LAYER_OUT],
        (0..DIGEST).map(|j| (v::<AB>(inp[DIGEST + j]), v::<AB>(x[PK_OUT + j]))),
    );
    when_eq::<AB>(
        b,
        &sel[S_CM_OUT],
        (0..NONCE).map(|j| (v::<AB>(inp[DIGEST + j]), v::<AB>(x[RHO_OUT + j]))),
    );
    when_eq::<AB>(
        b,
        &sel[S_CM_OUT],
        (0..NONCE).map(|j| (v::<AB>(inp[DIGEST + NONCE + j]), v::<AB>(x[R_OUT + j]))),
    );
    when_eq::<AB>(
        b,
        &sel[S_CM_OUT],
        (0..DIGEST).map(|j| (v::<AB>(dig[j]), public[PUB_OUT + j].clone())),
    );
}

/// Range checks and limb-wise conservation, on row 0.
fn eval_values<AB: AirBuilder<F = F>>(
    b: &mut AB,
    x: &[AB::Var],
    public: &[E<AB>],
    mode: Mode,
    zero: &E<AB>,
) {
    let mut first = b.when_first_row();
    for bit in &x[BITS_IN..BITS_OUT + MAX_BITS] {
        first.assert_bool(*bit);
    }
    first.assert_eq(x[VIN], recompose::<AB>(&x[BITS_IN..BITS_IN + LIMB_BITS]));
    first.assert_eq(
        x[VIN + 1],
        recompose::<AB>(&x[BITS_IN + LIMB_BITS..BITS_OUT]),
    );
    first.assert_eq(x[VOUT], recompose::<AB>(&x[BITS_OUT..BITS_OUT + LIMB_BITS]));
    first.assert_eq(
        x[VOUT + 1],
        recompose::<AB>(&x[BITS_OUT + LIMB_BITS..BITS_OUT + MAX_BITS]),
    );
    first.assert_bool(x[CARRY]);

    // vin_lo + carry·2^30 = vout_lo + fee ;  vin_hi = vout_hi + carry
    let radix = E::<AB>::from(F::from_u32(1 << LIMB_BITS));
    let carry = v::<AB>(x[CARRY]);
    first.assert_eq(
        v::<AB>(x[VIN]) + carry.clone() * radix,
        v::<AB>(x[VOUT]) + public[PUB_FEE].clone(),
    );
    first.assert_eq(v::<AB>(x[VIN + 1]), v::<AB>(x[VOUT + 1]) + carry);

    // Unshield: the "output" limbs are the public amount.
    if mode == Mode::Unshield {
        first.assert_eq(v::<AB>(x[VOUT]), public[PUB_OUT].clone());
        first.assert_eq(v::<AB>(x[VOUT + 1]), public[PUB_OUT + 1].clone());
        for j in 2..DIGEST {
            first.assert_eq(public[PUB_OUT + j].clone(), zero.clone());
        }
    }
}
