//! Two Merkle paths side by side, one Poseidon2 permutation for each per row.
//!
//! Shared by the credential and sanctions statements, which both need two
//! paths whose *positions* are related: the credential's leaf and its
//! revocation bit sit at the same index of two trees, and a sanctions gap is
//! two leaves at adjacent indices of one tree.
//!
//! Layout, `DEPTH = 16`, 32 rows:
//!
//! | row | path A | path B |
//! |---|---|---|
//! | 0 | leaf pre-image, first hash | leaf pre-image, first hash |
//! | 1 | `compress(row-0 digest, second)` → leaf | the same → leaf |
//! | 2 … 17 | Merkle levels 0 … 15 | Merkle levels 0 … 15 |
//! | 18 … 31 | padding | padding |
//!
//! Each path carries its own bit and sibling columns and an index accumulator
//! `acc`, which starts at 0 and adds `bit · 2^level` on each level row, so on
//! the last level row it holds the leaf's index — small enough (`< 2^16`) to
//! be compared in the field.

use std::borrow::Cow;

use p3_air::AirBuilder;
use p3_field::PrimeCharacteristicRing as _;
use p3_uni_stark::SubAirBuilder;

use crate::gadgets::merkle::MerklePath;
use crate::gadgets::poseidon::{self, POSEIDON_COLS, PermAir};
use crate::gadgets::state;
use crate::hash::{DIGEST, Digest, F, WIDTH, compress, permute};
use p3_air::Air as _;
use p3_matrix::dense::RowMajorMatrix;

/// Tree depth for both trees.
pub const DEPTH: usize = 16;
/// Trace rows.
pub const ROWS: usize = 32;
/// Row of the last Merkle level.
pub const LAST: usize = 1 + DEPTH;

/// Column offsets, from the start of the row.
pub const PERM_A: usize = 0;
/// Path B's permutation.
pub const PERM_B: usize = POSEIDON_COLS;
/// First column after both permutations.
pub const BASE: usize = 2 * POSEIDON_COLS;
/// Path A's level bit.
pub const BIT_A: usize = BASE;
/// Path B's level bit.
pub const BIT_B: usize = BASE + 1;
/// Path A's sibling.
pub const SIB_A: usize = BASE + 2;
/// Path B's sibling.
pub const SIB_B: usize = SIB_A + DIGEST;
/// Path A's index accumulator.
pub const ACC_A: usize = SIB_B + DIGEST;
/// Path B's index accumulator.
pub const ACC_B: usize = ACC_A + 1;
/// First statement-specific column.
pub const STATEMENT: usize = ACC_B + 1;

/// Periodic selectors, in this order.
pub const S_ROW0: usize = 0;
/// Row 1.
pub const S_ROW1: usize = 1;
/// A row whose next row is a Merkle level (rows 1 … 16).
pub const T_LEVEL: usize = 2;
/// A Merkle level row (rows 2 … 17).
pub const S_LEVEL: usize = 3;
/// The last level row.
pub const S_LAST: usize = 4;
/// `2^level` on level rows, 0 elsewhere.
pub const POW: usize = 5;
/// Selector count.
pub const SELECTORS: usize = 6;

/// The periodic selector columns.
#[must_use]
pub fn selectors() -> Vec<Vec<F>> {
    let col = |f: &dyn Fn(usize) -> F| (0..ROWS).map(f).collect::<Vec<F>>();
    let b = |x: bool| F::from_bool(x);
    vec![
        col(&|r| b(r == 0)),
        col(&|r| b(r == 1)),
        col(&|r| b((1..LAST).contains(&r))),
        col(&|r| b((2..=LAST).contains(&r))),
        col(&|r| b(r == LAST)),
        col(&|r| {
            if (2..=LAST).contains(&r) {
                F::from_u32(1 << (r - 2))
            } else {
                F::ZERO
            }
        }),
    ]
}

/// Borrowed periodic columns, for `BaseAir::periodic_columns`.
#[must_use]
pub fn periodic(cols: &[Vec<F>]) -> Cow<'_, [Vec<F>]> {
    Cow::Borrowed(cols)
}

type E<AB> = <AB as AirBuilder>::Expr;

/// A path's view of a row: permutation inputs and digest.
pub struct PathRow<V> {
    /// Permutation input.
    pub inputs: [V; WIDTH],
    /// Truncated output.
    pub digest: [V; DIGEST],
}

/// Path `offset`'s inputs and digest on `row`.
pub fn path_row<V: Copy>(row: &[V], offset: usize) -> PathRow<V> {
    let slice = &row[offset..offset + POSEIDON_COLS];
    PathRow {
        inputs: poseidon::inputs(slice),
        digest: poseidon::digest(slice),
    }
}

/// Both permutations, and both paths' Merkle wiring, bits and accumulators.
pub fn eval_paths<AB: AirBuilder<F = crate::hash::F>>(
    perm: &PermAir,
    builder: &mut AB,
    row: &[AB::Var],
    next: &[AB::Var],
    sel: &[E<AB>],
) {
    {
        let mut sub =
            SubAirBuilder::<AB, PermAir, AB::Var>::new(builder, PERM_A..PERM_A + POSEIDON_COLS);
        perm.eval(&mut sub);
    }
    {
        let mut sub =
            SubAirBuilder::<AB, PermAir, AB::Var>::new(builder, PERM_B..PERM_B + POSEIDON_COLS);
        perm.eval(&mut sub);
    }
    for (perm_at, bit_at, sib_at, acc_at) in
        [(PERM_A, BIT_A, SIB_A, ACC_A), (PERM_B, BIT_B, SIB_B, ACC_B)]
    {
        let (here, there) = (path_row(row, perm_at), path_row(next, perm_at));
        // Row 1 hashes row 0's digest on the left.
        let mut t = builder.when_transition();
        for j in 0..DIGEST {
            t.assert_zero(
                sel[S_ROW0].clone() * (E::<AB>::from(there.inputs[j]) - here.digest[j].into()),
            );
        }
        // Each level: the previous digest and the sibling, ordered by the bit.
        let bit: E<AB> = next[bit_at].into();
        for j in 0..DIGEST {
            let (cur, sib): (E<AB>, E<AB>) = (here.digest[j].into(), next[sib_at + j].into());
            let left = cur.clone() + bit.clone() * (sib.clone() - cur.clone());
            let right = sib.clone() + bit.clone() * (cur - sib);
            t.assert_zero(sel[T_LEVEL].clone() * (E::<AB>::from(there.inputs[j]) - left));
            t.assert_zero(sel[T_LEVEL].clone() * (E::<AB>::from(there.inputs[DIGEST + j]) - right));
        }
        // acc: 0 on row 1, then acc' = acc + bit'·2^level'.
        let own_bit: E<AB> = row[bit_at].into();
        builder
            .assert_zero(sel[S_LEVEL].clone() * own_bit.clone() * (own_bit.clone() - E::<AB>::ONE));
        builder.assert_zero(sel[S_ROW1].clone() * E::<AB>::from(row[acc_at]));
        let mut t = builder.when_transition();
        let acc: E<AB> = row[acc_at].into();
        let next_acc: E<AB> = next[acc_at].into();
        // On a level row the accumulator already includes this row's bit.
        t.assert_zero(
            sel[T_LEVEL].clone() * (next_acc - acc - next[bit_at].into() * next_pow::<AB>(sel)),
        );
    }
}

/// `2^level` of the *next* row, derived from this row's `POW` so the
/// transition needs no second periodic lookup: on row 1 it is 1, and on a
/// level row it doubles.
fn next_pow<AB: AirBuilder>(sel: &[E<AB>]) -> E<AB> {
    sel[S_ROW1].clone() + sel[POW].clone() * E::<AB>::from(AB::F::TWO)
}

/// Rows for one path: two leaf rows, then its Merkle levels.
fn path_rows(
    first: [F; WIDTH],
    second: Digest,
    path: &MerklePath,
) -> (Vec<[F; WIDTH]>, Vec<(F, Digest, F)>) {
    let d0: Digest = permute(first)[..DIGEST].try_into().expect("8");
    let mut states = vec![first, state(&d0, &second)];
    let mut extra = vec![(F::ZERO, [F::ZERO; DIGEST], F::ZERO); 2];
    let mut cur = compress(&d0, &second);
    let mut acc = 0u64;
    for (level, sib) in path.siblings.iter().enumerate() {
        let right = (path.index >> level) & 1 == 1;
        let st = if right {
            state(sib, &cur)
        } else {
            state(&cur, sib)
        };
        acc += u64::from(right) << level;
        states.push(st);
        extra.push((F::from_bool(right), *sib, F::from_u64(acc)));
        cur = compress(
            &st[..DIGEST].try_into().expect("8"),
            &st[DIGEST..].try_into().expect("8"),
        );
    }
    (states, extra)
}

/// The two paths' columns, plus `statement` columns (the same on every row
/// unless the caller overrides row 0 afterwards), as a trace.
#[must_use]
pub fn trace(
    a: ([F; WIDTH], Digest, &MerklePath),
    b: ([F; WIDTH], Digest, &MerklePath),
    statement: &[F],
) -> RowMajorMatrix<F> {
    let (sa, xa) = path_rows(a.0, a.1, a.2);
    let (sb, xb) = path_rows(b.0, b.1, b.2);
    let perm_a = poseidon::permutation_rows(&sa, ROWS);
    let perm_b = poseidon::permutation_rows(&sb, ROWS);
    let width = STATEMENT + statement.len();
    let mut values = Vec::with_capacity(ROWS * width);
    let blank = (F::ZERO, [F::ZERO; DIGEST], F::ZERO);
    for r in 0..ROWS {
        values.extend_from_slice(&perm_a[r]);
        values.extend_from_slice(&perm_b[r]);
        let (ba, sa_, aa) = xa.get(r).copied().unwrap_or(blank);
        let (bb, sb_, ab) = xb.get(r).copied().unwrap_or(blank);
        values.push(ba);
        values.push(bb);
        values.extend_from_slice(&sa_);
        values.extend_from_slice(&sb_);
        // Past the last level the accumulator must keep its value only while
        // a transition constraint looks at it, which none does; zero is fine.
        values.push(aa);
        values.push(ab);
        values.extend_from_slice(statement);
    }
    RowMajorMatrix::new(values, width)
}
