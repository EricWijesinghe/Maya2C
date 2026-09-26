//! Merkle path: a leaf is a member of the tree with a given root.
//!
//! One row per level, `depth` rows, `depth` a power of two. Row `i` holds the
//! node entering level `i` (`cur`), its sibling, and the path bit: with the
//! bit 0 the permutation input is `cur ‖ sib`, with the bit 1 `sib ‖ cur`, and
//! the next row's `cur` is this row's digest. The first row's `cur` is the
//! public leaf, the last row's digest the public root.
//!
//! Set membership is this statement with the set committed as the tree.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess as _};
use p3_field::PrimeCharacteristicRing as _;
use p3_matrix::dense::RowMajorMatrix;

use super::poseidon::{self, PermAir};
use crate::hash::{DIGEST, Digest, F, compress};
use crate::{Proof, ZkError};

const BIT: usize = 0;
const CUR: usize = 1;
const SIB: usize = CUR + DIGEST;
const EXTRA: usize = SIB + DIGEST;

/// A path from a leaf to the root: one sibling per level, leaf level first,
/// and the leaf's index whose bit `i` says which side level `i` is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MerklePath {
    /// Siblings, leaf level first.
    pub siblings: Vec<Digest>,
    /// The leaf's index; bit `i` set means the node is the *right* child at level `i`.
    pub index: u64,
}

impl MerklePath {
    /// The root this path and `leaf` hash to.
    pub fn root(&self, leaf: &Digest) -> Digest {
        self.siblings
            .iter()
            .enumerate()
            .fold(*leaf, |cur, (level, sib)| {
                if (self.index >> level) & 1 == 1 {
                    compress(sib, &cur)
                } else {
                    compress(&cur, sib)
                }
            })
    }
}

/// The Merkle path AIR for trees of `depth` levels.
pub struct MerkleAir {
    depth: usize,
    perm: PermAir,
}

impl MerkleAir {
    /// # Errors
    ///
    /// [`ZkError::Unsatisfied`] unless `depth` is a power of two, 2 to 64.
    pub fn new(depth: usize) -> Result<Self, ZkError> {
        if !depth.is_power_of_two() || !(2..=64).contains(&depth) {
            return Err(ZkError::Unsatisfied(
                "depth must be a power of two in 2..=64",
            ));
        }
        Ok(Self {
            depth,
            perm: poseidon::perm_air(),
        })
    }

    /// The tree depth.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.depth
    }
}

impl BaseAir<F> for MerkleAir {
    fn width(&self) -> usize {
        poseidon::width_with(EXTRA)
    }

    fn num_public_values(&self) -> usize {
        2 * DIGEST
    }
}

/// Constrains one level's permutation input to be `cur`/`sib` ordered by
/// `bit`, and `bit` to be boolean. Shared with the pool AIRs.
pub(crate) fn eval_level<AB: AirBuilder<F = F>>(
    builder: &mut AB,
    inputs: &[AB::Var; 16],
    bit: AB::Var,
    cur: &[AB::Var],
    sib: &[AB::Var],
) {
    builder.assert_bool(bit);
    for j in 0..DIGEST {
        let (c, s): (AB::Expr, AB::Expr) = (cur[j].into(), sib[j].into());
        let b: AB::Expr = bit.into();
        builder.assert_eq(inputs[j], c.clone() + b.clone() * (s.clone() - c.clone()));
        builder.assert_eq(inputs[DIGEST + j], s.clone() + b * (c - s));
    }
}

impl<AB: AirBuilder<F = F>> Air<AB> for MerkleAir {
    fn eval(&self, builder: &mut AB) {
        poseidon::eval_permutation(&self.perm, builder);
        let main = builder.main();
        let (row, next) = (main.current_slice(), main.next_slice());
        let extra = &row[poseidon::POSEIDON_COLS..];
        let next_cur = &next[poseidon::POSEIDON_COLS + CUR..poseidon::POSEIDON_COLS + CUR + DIGEST];
        let inputs = poseidon::inputs(row);
        let digest = poseidon::digest(row);
        let public: Vec<AB::Expr> = builder.public_values().iter().map(|&v| v.into()).collect();

        eval_level(
            builder,
            &inputs,
            extra[BIT],
            &extra[CUR..CUR + DIGEST],
            &extra[SIB..SIB + DIGEST],
        );

        let mut first = builder.when_first_row();
        for j in 0..DIGEST {
            first.assert_eq(extra[CUR + j], public[j].clone());
        }
        let mut transition = builder.when_transition();
        for j in 0..DIGEST {
            transition.assert_eq(next_cur[j], digest[j]);
        }
        let mut last = builder.when_last_row();
        for j in 0..DIGEST {
            last.assert_eq(digest[j], public[DIGEST + j].clone());
        }
    }
}

/// The trace for `leaf` along `path`, without any check.
#[must_use]
pub fn trace(leaf: &Digest, path: &MerklePath) -> RowMajorMatrix<F> {
    let mut states = Vec::with_capacity(path.siblings.len());
    let mut extras = Vec::with_capacity(path.siblings.len());
    let mut cur = *leaf;
    for (level, sib) in path.siblings.iter().enumerate() {
        let right = (path.index >> level) & 1 == 1;
        let state = if right {
            super::state(sib, &cur)
        } else {
            super::state(&cur, sib)
        };
        let mut extra = vec![F::from_bool(right)];
        extra.extend_from_slice(&cur);
        extra.extend_from_slice(sib);
        states.push(state);
        extras.push(extra);
        cur = compress(
            &state[..DIGEST].try_into().expect("8"),
            &state[DIGEST..].try_into().expect("8"),
        );
    }
    super::assemble(&states, &extras, EXTRA, path.siblings.len())
}

fn public(leaf: &Digest, root: &Digest) -> Vec<F> {
    leaf.iter().chain(root).copied().collect()
}

/// Proves `leaf` is in the tree with root `path.root(leaf)`.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] if the path length is not a valid depth.
pub fn prove(leaf: &Digest, path: &MerklePath) -> Result<(Proof, Digest), ZkError> {
    let air = MerkleAir::new(path.siblings.len())?;
    let root = path.root(leaf);
    let proof = crate::prove(&air, trace(leaf, path), &public(leaf, &root))?;
    Ok((proof, root))
}

/// Verifies membership of `leaf` under `root` in a tree of `depth` levels.
///
/// # Errors
///
/// [`ZkError::Rejected`] or [`ZkError::Malformed`].
pub fn verify(proof: &Proof, leaf: &Digest, root: &Digest, depth: usize) -> Result<(), ZkError> {
    crate::verify(&MerkleAir::new(depth)?, proof, &public(leaf, root))
}
