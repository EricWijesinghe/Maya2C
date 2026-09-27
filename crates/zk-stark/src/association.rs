//! Association-set inclusion: "my funds came from an approved set", without
//! saying which deposit (Master Prompt 27 §2; the Privacy Pools shape).
//!
//! A depositor holds a secret `s`. Their deposit is the leaf
//! `L = compress(s, DEPOSIT)`; an association-set provider publishes the root
//! of a tree of approved leaves. The proof shows
//!
//! ```text
//! L = compress(s, DEPOSIT) is in the tree with root R
//! nf = compress(s, NULLIFIER)
//! ```
//!
//! with only `R` and `nf` public. `L`, `s` and the path stay private, so the
//! verifier learns "one of the approved deposits" and nothing narrower; `nf`
//! binds the proof to one spend, so it cannot be replayed for a second
//! withdrawal (the pool already refuses a seen nullifier).
//!
//! The complement — "not from a flagged set" — is `crate::sanctions`.
//!
//! Layout: `ROWS` rows, a power of two. Rows `0..ROWS-1` are Merkle levels
//! (tree depth `ROWS - 1`), permutation A hashing each level; the last row's
//! `cur` is the root. On the last row permutation A derives the leaf and
//! permutation B the nullifier, both from the secret. The secret and the leaf
//! are carried unchanged through every row.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess as _};
use p3_field::PrimeCharacteristicRing as _;
use p3_matrix::dense::RowMajorMatrix;

use crate::gadgets::Domain;
use crate::gadgets::merkle::{MerklePath, eval_level};
use crate::gadgets::poseidon::{self, POSEIDON_COLS, PermAir};
use crate::hash::{DIGEST, Digest, F, WIDTH, compress};
use crate::{Proof, ZkError};

/// Rows in the trace; the tree has `ROWS - 1` levels (32,768 leaves).
pub const ROWS: usize = 16;
/// Tree depth.
pub const DEPTH: usize = ROWS - 1;

const B: usize = POSEIDON_COLS; // permutation B's first column
const X: usize = 2 * POSEIDON_COLS; // first gadget column
const BIT: usize = X;
const CUR: usize = X + 1;
const SIB: usize = CUR + DIGEST;
const LEAF: usize = SIB + DIGEST;
const SECRET: usize = LEAF + DIGEST;
const WIDTH_ALL: usize = SECRET + DIGEST;

fn tag(d: Domain) -> Digest {
    crate::gadgets::domain(d)
}

/// A deposit's leaf.
pub fn leaf(secret: &Digest) -> Digest {
    compress(secret, &tag(Domain::AssociationLeaf))
}

/// A deposit's nullifier.
pub fn nullifier(secret: &Digest) -> Digest {
    compress(secret, &tag(Domain::AssociationNullifier))
}

/// The AIR.
pub struct AssociationAir {
    perm: PermAir,
}

impl Default for AssociationAir {
    fn default() -> Self {
        Self {
            perm: poseidon::perm_air(),
        }
    }
}

impl BaseAir<F> for AssociationAir {
    fn width(&self) -> usize {
        WIDTH_ALL
    }

    fn num_public_values(&self) -> usize {
        2 * DIGEST
    }
}

impl<AB: AirBuilder<F = F>> Air<AB> for AssociationAir {
    fn eval(&self, builder: &mut AB) {
        poseidon::eval_permutation(&self.perm, builder);
        poseidon::eval_permutation_at(&self.perm, builder, B);
        let main = builder.main();
        let (row, next) = (main.current_slice().to_vec(), main.next_slice().to_vec());
        let public: Vec<AB::Expr> = builder.public_values().iter().map(|&v| v.into()).collect();
        let (root, nf) = (&public[..DIGEST], &public[DIGEST..]);
        let col = |r: &[AB::Var], at: usize| -> Vec<AB::Var> { r[at..at + DIGEST].to_vec() };
        let (cur, sib, leaf_c, secret) = (
            col(&row, CUR),
            col(&row, SIB),
            col(&row, LEAF),
            col(&row, SECRET),
        );
        let (a_in, a_out) = (
            poseidon::inputs(&row[..POSEIDON_COLS]),
            poseidon::digest(&row[..POSEIDON_COLS]),
        );
        let (b_in, b_out) = (
            poseidon::inputs(&row[B..B + POSEIDON_COLS]),
            poseidon::digest(&row[B..B + POSEIDON_COLS]),
        );

        // Levels: every row but the last hashes `cur`/`sib` by `bit`, and the
        // next row's `cur` is the result; the secret and leaf never change.
        let mut t = builder.when_transition();
        eval_level(&mut t, &a_in, row[BIT], &cur, &sib);
        for j in 0..DIGEST {
            t.assert_eq(next[CUR + j], a_out[j]);
            t.assert_eq(next[LEAF + j], leaf_c[j]);
            t.assert_eq(next[SECRET + j], secret[j]);
        }
        // The walk starts at the leaf.
        let mut first = builder.when_first_row();
        for j in 0..DIGEST {
            first.assert_eq(cur[j], leaf_c[j]);
        }
        // The last row: its `cur` is the root; A derives the leaf and B the
        // nullifier from the same secret.
        let (dep, nul) = (
            tag(Domain::AssociationLeaf),
            tag(Domain::AssociationNullifier),
        );
        let mut last = builder.when_last_row();
        for j in 0..DIGEST {
            last.assert_eq(cur[j], root[j].clone());
            last.assert_eq(a_in[j], secret[j]);
            last.assert_eq(a_in[DIGEST + j], AB::Expr::from(dep[j]));
            last.assert_eq(a_out[j], leaf_c[j]);
            last.assert_eq(b_in[j], secret[j]);
            last.assert_eq(b_in[DIGEST + j], AB::Expr::from(nul[j]));
            last.assert_eq(b_out[j], nf[j].clone());
        }
    }
}

/// The trace, without any check.
#[must_use]
pub fn trace(secret: &Digest, path: &MerklePath) -> RowMajorMatrix<F> {
    let the_leaf = leaf(secret);
    let (mut a_states, mut rows_extra) = (Vec::with_capacity(ROWS), Vec::with_capacity(ROWS));
    let mut cur = the_leaf;
    for (level, sib) in path.siblings.iter().enumerate() {
        let right = (path.index >> level) & 1 == 1;
        let state = if right {
            crate::gadgets::state(sib, &cur)
        } else {
            crate::gadgets::state(&cur, sib)
        };
        let mut extra = vec![F::from_bool(right)];
        extra.extend_from_slice(&cur);
        extra.extend_from_slice(sib);
        a_states.push(state);
        rows_extra.push(extra);
        cur = if right {
            compress(sib, &cur)
        } else {
            compress(&cur, sib)
        };
    }
    // Last row: `cur` is the root, A derives the leaf.
    a_states.push(crate::gadgets::state(secret, &tag(Domain::AssociationLeaf)));
    let mut extra = vec![F::ZERO];
    extra.extend_from_slice(&cur);
    extra.extend(core::iter::repeat_n(F::ZERO, DIGEST));
    rows_extra.push(extra);
    let mut b_states = vec![[F::ZERO; WIDTH]; ROWS - 1];
    b_states.push(crate::gadgets::state(
        secret,
        &tag(Domain::AssociationNullifier),
    ));

    let (a_rows, b_rows) = (
        poseidon::permutation_rows(&a_states, ROWS),
        poseidon::permutation_rows(&b_states, ROWS),
    );
    let mut values = Vec::with_capacity(ROWS * WIDTH_ALL);
    for ((a, b), extra) in a_rows.iter().zip(&b_rows).zip(&rows_extra) {
        values.extend_from_slice(a);
        values.extend_from_slice(b);
        values.extend_from_slice(extra);
        values.extend_from_slice(&the_leaf);
        values.extend_from_slice(secret);
    }
    RowMajorMatrix::new(values, WIDTH_ALL)
}

/// Proves the deposit of `secret`, at `path` in the approved tree, is in it.
/// Returns the proof, the root and the nullifier.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] for a path that is not `DEPTH` long.
pub fn prove(secret: &Digest, path: &MerklePath) -> Result<(Proof, Digest, Digest), ZkError> {
    if path.siblings.len() != DEPTH {
        return Err(ZkError::Unsatisfied("association path must be DEPTH long"));
    }
    let root = path.root(&leaf(secret));
    let nf = nullifier(secret);
    let public: Vec<F> = root.iter().chain(&nf).copied().collect();
    let proof = crate::prove(&AssociationAir::default(), trace(secret, path), &public)?;
    Ok((proof, root, nf))
}

/// Checks that some deposit in the tree with `root` has nullifier `nf`.
///
/// # Errors
///
/// [`ZkError::Rejected`] or [`ZkError::Malformed`].
pub fn verify(proof: &Proof, root: &Digest, nf: &Digest) -> Result<(), ZkError> {
    let public: Vec<F> = root.iter().chain(nf).copied().collect();
    crate::verify(&AssociationAir::default(), proof, &public)
}

/// The root of a depth-`DEPTH` tree over `leaves`, and the path to `index`.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] for an empty set or an index outside it.
pub fn tree(leaves: &[Digest], index: u64) -> Result<(Digest, MerklePath), ZkError> {
    let full = crate::pool::tree::merkle_path(leaves, index)?;
    let path = MerklePath {
        siblings: full.siblings[..DEPTH].to_vec(),
        index,
    };
    let leaf = *leaves
        .get(usize::try_from(index).unwrap_or(usize::MAX))
        .ok_or(ZkError::Unsatisfied("index outside the set"))?;
    Ok((path.root(&leaf), path))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn secret(n: u32) -> Digest {
        core::array::from_fn(|i| F::from_u32(n * 31 + u32::try_from(i).unwrap() + 7))
    }

    fn approved() -> Vec<Digest> {
        (0..20).map(|i| leaf(&secret(i))).collect()
    }

    #[test]
    fn a_member_proves_without_revealing_which_and_the_nullifier_binds() {
        let set = approved();
        let (root, path) = tree(&set, 13).unwrap();
        let t = std::time::Instant::now();
        let (proof, r, nf) = prove(&secret(13), &path).unwrap();
        let prove_time = t.elapsed();
        assert_eq!(r, root);
        let t = std::time::Instant::now();
        verify(&proof, &root, &nf).unwrap();
        println!(
            "association-set inclusion (depth {DEPTH}): prove {prove_time:?}, verify {:?}, proof {} bytes",
            t.elapsed(),
            proof.as_bytes().len()
        );
        assert!(
            verify(&proof, &root, &nullifier(&secret(12))).is_err(),
            "another nullifier"
        );
        let (other_root, _) = tree(&set[..10], 3).unwrap();
        assert!(verify(&proof, &other_root, &nf).is_err(), "another set");
    }

    #[test]
    fn an_outsider_cannot_prove_membership() {
        // Its leaf is not in the tree: the path it holds is someone else's,
        // so the root it reaches is not the approved one.
        let set = approved();
        let (root, path) = tree(&set, 4).unwrap();
        let outsider = secret(999);
        let bad = trace(&outsider, &path);
        let public: Vec<F> = root.iter().chain(&nullifier(&outsider)).copied().collect();
        let quiet = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::prove(&AssociationAir::default(), bad, &public)
        }));
        std::panic::set_hook(quiet);
        if let Ok(Ok(proof)) = outcome {
            assert!(verify(&proof, &root, &nullifier(&outsider)).is_err());
        }
    }
}
