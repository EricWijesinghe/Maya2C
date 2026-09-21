//! The joinsplit AIR: two notes in, two notes out, with transparent edges.
//!
//! 128 rows, one Poseidon2 permutation each; roles fixed by periodic selector
//! columns the verifier supplies, so no witness can switch a check off.
//!
//! | rows | role |
//! |---|---|
//! | `36i + 0` | input `i` key: `sk ‖ KEY` → address |
//! | `36i + 1` | input `i` value layer: `lo, hi, 0.. ‖ address` |
//! | `36i + 2` | input `i` commitment: `layer ‖ rho ‖ rand` |
//! | `36i + 3 … 36i + 34` | input `i` Merkle path, 32 levels, up to the anchor |
//! | `36i + 35` | input `i` nullifier: `sk ‖ NULLIFIER, rho, 0..` |
//! | `72 + 2j`, `73 + 2j` | output `j` value layer and commitment |
//! | 76–127 | padding: honest permutations, nothing else |
//!
//! **Dummy inputs.** An unused input slot carries a zero-value note with
//! `is_dummy = 1`: its value is forced to zero and its Merkle root is not
//! compared with the anchor (there is no leaf to find), but its key,
//! commitment and nullifier are computed like any other, so the published
//! nullifier still binds to *something* and nothing on chain tells a dummy
//! from a real spend. Marking a real note dummy gains nothing: its value
//! becomes zero.
//!
//! **Conservation** over 27-bit limbs, with `A = in₀ + in₁ + public_in` and
//! `B = out₀ + out₁ + public_out + fee`:
//!
//! ```text
//! A_lo + 4·2^27 = B_lo + c·2^27        A_hi + c = B_hi + 4        0 ≤ c ≤ 7
//! ```
//!
//! which says `A = B` as integers: `A_lo ≤ 3(2^27−1)`, `B_lo ≤ 4(2^27−1)`, so
//! every side is below `11·2^27 < p ≈ 15·2^27` and nothing wraps.
//!
//! **Public values** (57): anchor, both nullifiers, both output commitments,
//! the limbs of `public_in`, `public_out` and `fee`, and the recipient as
//! eleven 24-bit words — bound by the transcript, so a miner who rewrites the
//! recipient invalidates the proof.

use std::borrow::Cow;

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess as _};
use p3_field::PrimeCharacteristicRing as _;

use crate::gadgets::poseidon::{self, PermAir};
use crate::gadgets::range::recompose;
use crate::gadgets::{Domain, domain};
use crate::hash::{DIGEST, F};

use super::note::{LIMB_BITS, NONCE, VALUE_BITS};
use super::tree::TREE_DEPTH;

/// Trace rows.
pub const ROWS: usize = 128;
/// Inputs and outputs.
pub const INPUTS: usize = 2;
/// Outputs.
pub const OUTPUTS: usize = 2;
/// Rows one input occupies.
pub const INPUT_ROWS: usize = 4 + TREE_DEPTH;
const OUT_BASE: usize = INPUTS * INPUT_ROWS;

// Registers, after the permutation columns.
const IN_REG: usize = DIGEST + 2 * NONCE + 2 + 1; // sk, rho, rand, v(2), dummy
const OUT_REG: usize = DIGEST + 2 * NONCE + 2; // address, rho, rand, v(2)
/// Input `i`'s register block.
#[must_use]
pub const fn in_reg(i: usize) -> usize {
    i * IN_REG
}
/// Output `j`'s register block.
#[must_use]
pub const fn out_reg(j: usize) -> usize {
    INPUTS * IN_REG + j * OUT_REG
}
/// Offsets inside an input block.
pub mod input {
    /// Spending key.
    pub const SK: usize = 0;
    /// Uniqueness nonce.
    pub const RHO: usize = 8;
    /// Blinding.
    pub const RAND: usize = 12;
    /// Value limbs.
    pub const V: usize = 16;
    /// Dummy flag.
    pub const DUMMY: usize = 18;
}
/// Offsets inside an output block.
pub mod output {
    /// Recipient address.
    pub const ADDRESS: usize = 0;
    /// Uniqueness nonce.
    pub const RHO: usize = 8;
    /// Blinding.
    pub const RAND: usize = 12;
    /// Value limbs.
    pub const V: usize = 16;
}
/// Carry bits (3).
pub const CARRY: usize = out_reg(OUTPUTS);
/// Register count.
pub const REGISTERS: usize = CARRY + 3;
/// Per-row Merkle bit.
pub const BIT: usize = REGISTERS;
/// Per-row Merkle sibling.
pub const SIB: usize = BIT + 1;
/// Value bits (row 0): inputs then outputs, `VALUE_BITS` each.
pub const BITS: usize = SIB + DIGEST;
/// Columns after the permutation.
pub const EXTRA: usize = BITS + (INPUTS + OUTPUTS) * VALUE_BITS;

/// Public-value offsets.
pub mod public {
    use super::DIGEST;
    /// Anchor.
    pub const ANCHOR: usize = 0;
    /// Nullifier `i` at `NULLIFIER + i * 8`.
    pub const NULLIFIER: usize = DIGEST;
    /// Commitment `j` at `COMMITMENT + j * 8`.
    pub const COMMITMENT: usize = 3 * DIGEST;
    /// `public_in` limbs.
    pub const PUBLIC_IN: usize = 5 * DIGEST;
    /// `public_out` limbs.
    pub const PUBLIC_OUT: usize = PUBLIC_IN + 2;
    /// Fee limbs.
    pub const FEE: usize = PUBLIC_OUT + 2;
    /// Recipient words.
    pub const RECIPIENT: usize = FEE + 2;
    /// Recipient words count (24 bits each).
    pub const RECIPIENT_WORDS: usize = 11;
    /// Total.
    pub const LEN: usize = RECIPIENT + RECIPIENT_WORDS;
}

// Selectors.
const T_TO_LAYER: usize = 0;
const T_TO_LEFT: usize = 1;
const T_MERKLE: usize = 2;
const S_MERKLE: usize = 3;
const fn s_in(i: usize, role: usize) -> usize {
    4 + i * 5 + role
}
const KEY: usize = 0;
const LAYER: usize = 1;
const CM: usize = 2;
const ROOT: usize = 3;
const NF: usize = 4;
const fn s_out(j: usize, role: usize) -> usize {
    4 + INPUTS * 5 + j * 2 + role
}
const SELECTORS: usize = 4 + INPUTS * 5 + OUTPUTS * 2;

/// The joinsplit AIR.
pub struct JoinSplitAir {
    perm: PermAir,
    selectors: Vec<Vec<F>>,
}

fn column(rows: impl Fn(usize) -> bool) -> Vec<F> {
    (0..ROWS).map(|r| F::from_bool(rows(r))).collect()
}

impl Default for JoinSplitAir {
    fn default() -> Self {
        let base = |i: usize| i * INPUT_ROWS;
        let is_in = |r: usize, off: usize| (0..INPUTS).any(|i| r == base(i) + off);
        let is_out_layer = |r: usize| (0..OUTPUTS).any(|j| r == OUT_BASE + 2 * j);
        let mut s = vec![Vec::new(); SELECTORS];
        s[T_TO_LAYER] = column(|r| is_in(r, 0));
        s[T_TO_LEFT] = column(|r| is_in(r, 1) || is_out_layer(r));
        s[T_MERKLE] =
            column(|r| (0..INPUTS).any(|i| (base(i) + 2..base(i) + 2 + TREE_DEPTH).contains(&r)));
        s[S_MERKLE] =
            column(|r| (0..INPUTS).any(|i| (base(i) + 3..base(i) + 3 + TREE_DEPTH).contains(&r)));
        for i in 0..INPUTS {
            s[s_in(i, KEY)] = column(|r| r == base(i));
            s[s_in(i, LAYER)] = column(|r| r == base(i) + 1);
            s[s_in(i, CM)] = column(|r| r == base(i) + 2);
            s[s_in(i, ROOT)] = column(|r| r == base(i) + 2 + TREE_DEPTH);
            s[s_in(i, NF)] = column(|r| r == base(i) + 3 + TREE_DEPTH);
        }
        for j in 0..OUTPUTS {
            s[s_out(j, 0)] = column(|r| r == OUT_BASE + 2 * j);
            s[s_out(j, 1)] = column(|r| r == OUT_BASE + 2 * j + 1);
        }
        Self {
            perm: poseidon::perm_air(),
            selectors: s,
        }
    }
}

impl BaseAir<F> for JoinSplitAir {
    fn width(&self) -> usize {
        poseidon::width_with(EXTRA)
    }

    fn num_public_values(&self) -> usize {
        public::LEN
    }

    fn num_periodic_columns(&self) -> usize {
        SELECTORS
    }

    fn periodic_columns(&self) -> Cow<'_, [Vec<F>]> {
        Cow::Borrowed(&self.selectors)
    }
}

type E<AB> = <AB as AirBuilder>::Expr;

fn v<AB: AirBuilder<F = F>>(x: AB::Var) -> E<AB> {
    x.into()
}

/// `sel · (a − b) = 0`.
fn eq<AB: AirBuilder<F = F>>(b: &mut AB, sel: &E<AB>, a: E<AB>, c: E<AB>) {
    b.assert_zero(sel.clone() * (a - c));
}

struct Row<'a, AB: AirBuilder<F = F>> {
    x: &'a [AB::Var],
    inp: [AB::Var; 16],
    dig: [AB::Var; DIGEST],
    sel: &'a [E<AB>],
    public: &'a [E<AB>],
}

impl<AB: AirBuilder<F = F>> Air<AB> for JoinSplitAir {
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
        let r = Row::<AB> {
            x: &row[poseidon::POSEIDON_COLS..],
            inp: poseidon::inputs(&row),
            dig: poseidon::digest(&row),
            sel: &sel,
            public: &public,
        };
        let nx = &next[poseidon::POSEIDON_COLS..];
        let next_inp = poseidon::inputs(&next);

        eval_links(builder, &r, nx, &next_inp);
        for i in 0..INPUTS {
            eval_input(builder, &r, i);
        }
        for j in 0..OUTPUTS {
            eval_output(builder, &r, j);
        }
        eval_values(builder, r.x, &public);
    }
}

/// Registers constant; `address` into the layer, each layer into its
/// commitment, each Merkle level from the one below.
fn eval_links<AB: AirBuilder<F = F>>(
    b: &mut AB,
    r: &Row<'_, AB>,
    nx: &[AB::Var],
    next_inp: &[AB::Var; 16],
) {
    let mut t = b.when_transition();
    for c in 0..REGISTERS {
        t.assert_eq(nx[c], r.x[c]);
    }
    let bit = v::<AB>(nx[BIT]);
    for j in 0..DIGEST {
        let d = v::<AB>(r.dig[j]);
        t.assert_zero(r.sel[T_TO_LAYER].clone() * (v::<AB>(next_inp[DIGEST + j]) - d.clone()));
        t.assert_zero(r.sel[T_TO_LEFT].clone() * (v::<AB>(next_inp[j]) - d.clone()));
        let sib = v::<AB>(nx[SIB + j]);
        let left = d.clone() + bit.clone() * (sib.clone() - d.clone());
        let right = sib.clone() + bit.clone() * (d - sib);
        t.assert_zero(r.sel[T_MERKLE].clone() * (v::<AB>(next_inp[j]) - left));
        t.assert_zero(r.sel[T_MERKLE].clone() * (v::<AB>(next_inp[DIGEST + j]) - right));
    }
    let own_bit = v::<AB>(r.x[BIT]);
    b.assert_zero(r.sel[S_MERKLE].clone() * own_bit.clone() * (own_bit - E::<AB>::ONE));
}

fn eval_input<AB: AirBuilder<F = F>>(b: &mut AB, r: &Row<'_, AB>, i: usize) {
    let reg = |off: usize| v::<AB>(r.x[in_reg(i) + off]);
    let inp = |k: usize| v::<AB>(r.inp[k]);
    let (key, nf_tag) = (domain(Domain::Key), domain(Domain::Nullifier)[0]);
    let s = |role| r.sel[s_in(i, role)].clone();
    for k in 0..DIGEST {
        eq(b, &s(KEY), inp(k), reg(input::SK + k));
        eq(b, &s(KEY), inp(DIGEST + k), E::<AB>::from(key[k]));
        eq(b, &s(NF), inp(k), reg(input::SK + k));
    }
    eq(b, &s(LAYER), inp(0), reg(input::V));
    eq(b, &s(LAYER), inp(1), reg(input::V + 1));
    for k in 2..DIGEST {
        eq(b, &s(LAYER), inp(k), E::<AB>::ZERO);
    }
    for k in 0..NONCE {
        eq(b, &s(CM), inp(DIGEST + k), reg(input::RHO + k));
        eq(b, &s(CM), inp(DIGEST + NONCE + k), reg(input::RAND + k));
        eq(b, &s(NF), inp(DIGEST + 1 + k), reg(input::RHO + k));
    }
    eq(b, &s(NF), inp(DIGEST), E::<AB>::from(nf_tag));
    for k in DIGEST + 1 + NONCE..16 {
        eq(b, &s(NF), inp(k), E::<AB>::ZERO);
    }
    let real = E::<AB>::ONE - reg(input::DUMMY);
    for k in 0..DIGEST {
        let root_sel = s(ROOT) * real.clone();
        eq(
            b,
            &root_sel,
            v::<AB>(r.dig[k]),
            r.public[public::ANCHOR + k].clone(),
        );
        eq(
            b,
            &s(NF),
            v::<AB>(r.dig[k]),
            r.public[public::NULLIFIER + i * DIGEST + k].clone(),
        );
    }
}

fn eval_output<AB: AirBuilder<F = F>>(b: &mut AB, r: &Row<'_, AB>, j: usize) {
    let reg = |off: usize| v::<AB>(r.x[out_reg(j) + off]);
    let inp = |k: usize| v::<AB>(r.inp[k]);
    let (layer, cm) = (r.sel[s_out(j, 0)].clone(), r.sel[s_out(j, 1)].clone());
    eq(b, &layer, inp(0), reg(output::V));
    eq(b, &layer, inp(1), reg(output::V + 1));
    for k in 2..DIGEST {
        eq(b, &layer, inp(k), E::<AB>::ZERO);
    }
    for k in 0..DIGEST {
        eq(b, &layer, inp(DIGEST + k), reg(output::ADDRESS + k));
        eq(
            b,
            &cm,
            v::<AB>(r.dig[k]),
            r.public[public::COMMITMENT + j * DIGEST + k].clone(),
        );
    }
    for k in 0..NONCE {
        eq(b, &cm, inp(DIGEST + k), reg(output::RHO + k));
        eq(b, &cm, inp(DIGEST + NONCE + k), reg(output::RAND + k));
    }
}

/// Row 0: bit decomposition of every note limb, dummies hold nothing, and the
/// limb-wise conservation equation.
fn eval_values<AB: AirBuilder<F = F>>(b: &mut AB, x: &[AB::Var], public: &[E<AB>]) {
    let mut first = b.when_first_row();
    let limb_regs: Vec<usize> = (0..INPUTS)
        .map(|i| in_reg(i) + input::V)
        .chain((0..OUTPUTS).map(|j| out_reg(j) + output::V))
        .collect();
    for (n, &reg) in limb_regs.iter().enumerate() {
        let bits = &x[BITS + n * VALUE_BITS..BITS + (n + 1) * VALUE_BITS];
        for bit in bits {
            first.assert_bool(*bit);
        }
        first.assert_eq(x[reg], recompose::<AB>(&bits[..LIMB_BITS]));
        first.assert_eq(x[reg + 1], recompose::<AB>(&bits[LIMB_BITS..]));
    }
    for i in 0..INPUTS {
        let dummy = x[in_reg(i) + input::DUMMY];
        first.assert_bool(dummy);
        first.assert_zero(v::<AB>(dummy) * v::<AB>(x[in_reg(i) + input::V]));
        first.assert_zero(v::<AB>(dummy) * v::<AB>(x[in_reg(i) + input::V + 1]));
    }
    let c_bits = &x[CARRY..CARRY + 3];
    for bit in c_bits {
        first.assert_bool(*bit);
    }
    let c = recompose::<AB>(c_bits);
    let radix = E::<AB>::from(F::from_u32(1 << LIMB_BITS));
    let four = E::<AB>::from(F::from_u32(4));
    let limb = |reg: usize, hi: usize| v::<AB>(x[reg + hi]);
    let a = |hi: usize| {
        (0..INPUTS)
            .map(|i| limb(in_reg(i) + input::V, hi))
            .fold(public[public::PUBLIC_IN + hi].clone(), |s, t| s + t)
    };
    let bsum = |hi: usize| {
        (0..OUTPUTS).map(|j| limb(out_reg(j) + output::V, hi)).fold(
            public[public::PUBLIC_OUT + hi].clone() + public[public::FEE + hi].clone(),
            |s, t| s + t,
        )
    };
    first.assert_eq(
        a(0) + four.clone() * radix.clone(),
        bsum(0) + c.clone() * radix,
    );
    first.assert_eq(a(1) + c, bsum(1) + four);
}
