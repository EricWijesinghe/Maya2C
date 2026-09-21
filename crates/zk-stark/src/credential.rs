//! Selective disclosure — ADR-008.
//!
//! > I know a claim value `v`, a subject, a schema and a blinding factor such
//! > that `leaf = compress(compress(v, schema, 0.. ‖ subject), blind)` is in
//! > the issuer's tree under `issuer_root`; `v` satisfies the public
//! > predicate; and the position of that leaf in the revocation tree under
//! > `revocation_root` holds the *unrevoked* leaf.
//!
//! Public: the two roots and the predicate. The value, subject, schema,
//! blinding and index stay private. The issuer's signature is not in here —
//! the chain verifies it natively when the root is anchored, which is why
//! only membership under an anchored root has to be proved.
//!
//! Values are below `2^29` so that `v = bound + d` in the field is the
//! integer equation (`bound + d < 2^30 < p`). The predicate is coarse on
//! purpose — `AtLeast(18)`, not a birth date — because a fine predicate
//! leaks through the public input however private the value is.
//!
//! Unlike the Groth16 presentation this replaces, it is post-quantum and has
//! no setup.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess as _};
use p3_field::{Field as _, PrimeCharacteristicRing as _};
use std::borrow::Cow;

use crate::dual::{self, S_LAST, S_ROW0, S_ROW1, STATEMENT};
use crate::gadgets::merkle::MerklePath;
use crate::gadgets::poseidon::PermAir;
use crate::gadgets::range::recompose;
use crate::gadgets::{poseidon, state};
use crate::hash::{DIGEST, Digest, F, WIDTH, compress};
use crate::{Proof, ZkError};

/// Bits a claim value may use.
pub const VALUE_BITS: usize = 29;
/// Largest claim value.
pub const MAX_VALUE: u32 = (1 << VALUE_BITS) - 1;

const SUBJECT: usize = STATEMENT;
const BLIND: usize = SUBJECT + DIGEST;
const VALUE: usize = BLIND + DIGEST;
const SCHEMA: usize = VALUE + 1;
const DIFF: usize = SCHEMA + 1;
const VBITS: usize = DIFF + 1;
const DBITS: usize = VBITS + VALUE_BITS;
const END: usize = DBITS + VALUE_BITS;
const REVOCATION_TAG: u32 = 0x7265_76;

/// What is disclosed about the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Predicate {
    /// `v >= bound`.
    AtLeast(u32),
    /// `v <= bound`.
    AtMost(u32),
    /// `v == bound`.
    EqualTo(u32),
}

impl Predicate {
    fn parts(self) -> (u32, u32) {
        match self {
            Self::AtLeast(b) => (0, b),
            Self::AtMost(b) => (1, b),
            Self::EqualTo(b) => (2, b),
        }
    }

    /// Whether `value` satisfies it.
    #[must_use]
    pub const fn holds(self, value: u32) -> bool {
        match self {
            Self::AtLeast(b) => value >= b,
            Self::AtMost(b) => value <= b,
            Self::EqualTo(b) => value == b,
        }
    }

    /// The non-negative difference the AIR range-checks.
    const fn diff(self, value: u32) -> u32 {
        match self {
            Self::AtLeast(b) => value.wrapping_sub(b),
            Self::AtMost(b) => b.wrapping_sub(value),
            Self::EqualTo(_) => 0,
        }
    }
}

/// Everything a verifier sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisclosurePublic {
    /// Root of the issuer's credential tree (anchored on chain).
    pub issuer_root: Digest,
    /// Root of the issuer's revocation tree.
    pub revocation_root: Digest,
    /// The predicate.
    pub predicate: Predicate,
}

impl DisclosurePublic {
    fn values(&self) -> Result<Vec<F>, ZkError> {
        let (tag, bound) = self.predicate.parts();
        if bound > MAX_VALUE {
            return Err(ZkError::Unsatisfied("predicate bound exceeds 2^29 - 1"));
        }
        let mut v = self.issuer_root.to_vec();
        v.extend_from_slice(&self.revocation_root);
        v.push(F::from_u32(tag));
        v.push(F::from_u32(bound));
        Ok(v)
    }
}

/// The holder's private inputs.
#[derive(Clone, Debug)]
pub struct DisclosureWitness {
    /// Who the credential is about.
    pub subject: Digest,
    /// What kind of claim it is.
    pub schema: u32,
    /// The claim value.
    pub value: u32,
    /// Blinding.
    pub blinding: Digest,
    /// Path of the credential leaf in the issuer tree.
    pub credential_path: MerklePath,
    /// Path of the same index in the revocation tree.
    pub revocation_path: MerklePath,
}

fn value_digest(value: u32, schema: u32) -> Digest {
    let mut d = [F::ZERO; DIGEST];
    d[0] = F::from_u32(value);
    d[1] = F::from_u32(schema);
    d
}

fn first_state(value: u32, schema: u32, subject: &Digest) -> [F; WIDTH] {
    state(&value_digest(value, schema), subject)
}

fn revocation_first(revoked: bool) -> [F; WIDTH] {
    let mut left = [F::ZERO; DIGEST];
    left[0] = F::from_u32(REVOCATION_TAG);
    left[1] = F::from_bool(revoked);
    state(&left, &[F::ZERO; DIGEST])
}

/// A credential leaf, as the issuer builds it.
#[must_use]
pub fn credential_leaf(subject: &Digest, schema: u32, value: u32, blinding: &Digest) -> Digest {
    compress(&compress(&value_digest(value, schema), subject), blinding)
}

/// A revocation-tree leaf.
#[must_use]
pub fn revocation_leaf(revoked: bool) -> Digest {
    let s = revocation_first(revoked);
    let d0 = compress(
        &s[..DIGEST].try_into().expect("8"),
        &s[DIGEST..].try_into().expect("8"),
    );
    compress(&d0, &[F::ZERO; DIGEST])
}

/// The disclosure AIR.
pub struct DisclosureAir {
    perm: PermAir,
    selectors: Vec<Vec<F>>,
}

impl Default for DisclosureAir {
    fn default() -> Self {
        Self {
            perm: poseidon::perm_air(),
            selectors: dual::selectors(),
        }
    }
}

impl BaseAir<F> for DisclosureAir {
    fn width(&self) -> usize {
        END
    }
    fn num_public_values(&self) -> usize {
        2 * DIGEST + 2
    }
    fn num_periodic_columns(&self) -> usize {
        dual::SELECTORS
    }
    fn periodic_columns(&self) -> Cow<'_, [Vec<F>]> {
        dual::periodic(&self.selectors)
    }
}

type E<AB> = <AB as AirBuilder>::Expr;

impl<AB: AirBuilder<F = F>> Air<AB> for DisclosureAir {
    fn eval(&self, b: &mut AB) {
        let main = b.main();
        let (row, next) = (main.current_slice().to_vec(), main.next_slice().to_vec());
        let sel: Vec<E<AB>> = b.periodic_values().iter().map(|&p| p.into()).collect();
        let public: Vec<E<AB>> = b.public_values().iter().map(|&p| p.into()).collect();
        dual::eval_paths(&self.perm, b, &row, &next, &sel);
        let (a, r) = (
            dual::path_row(&row, dual::PERM_A),
            dual::path_row(&row, dual::PERM_B),
        );
        let v = |c: usize| -> E<AB> { row[c].into() };

        let mut t = b.when_transition();
        for c in STATEMENT..END {
            t.assert_eq(next[c], row[c]);
        }
        // Row 0: the credential's first hash and the revocation leaf's.
        let rev = revocation_first(false);
        let s0 = sel[S_ROW0].clone();
        b.assert_zero(s0.clone() * (E::<AB>::from(a.inputs[0]) - v(VALUE)));
        b.assert_zero(s0.clone() * (E::<AB>::from(a.inputs[1]) - v(SCHEMA)));
        for k in 0..WIDTH {
            if (2..DIGEST).contains(&k) {
                b.assert_zero(s0.clone() * E::<AB>::from(a.inputs[k]));
            }
            if k >= DIGEST {
                b.assert_zero(s0.clone() * (E::<AB>::from(a.inputs[k]) - v(SUBJECT + k - DIGEST)));
            }
            b.assert_zero(s0.clone() * (E::<AB>::from(r.inputs[k]) - E::<AB>::from(rev[k])));
        }
        // Row 1: blinding on the right; the revocation leaf's second half is zero.
        for k in 0..DIGEST {
            b.assert_zero(
                sel[S_ROW1].clone() * (E::<AB>::from(a.inputs[DIGEST + k]) - v(BLIND + k)),
            );
            b.assert_zero(sel[S_ROW1].clone() * E::<AB>::from(r.inputs[DIGEST + k]));
        }
        // Last level: both roots, and the same index in both trees.
        for k in 0..DIGEST {
            b.assert_zero(sel[S_LAST].clone() * (E::<AB>::from(a.digest[k]) - public[k].clone()));
            b.assert_zero(
                sel[S_LAST].clone() * (E::<AB>::from(r.digest[k]) - public[DIGEST + k].clone()),
            );
        }
        b.assert_zero(sel[S_LAST].clone() * (v(dual::ACC_A) - v(dual::ACC_B)));
        eval_predicate(b, &row, &public);
    }
}

fn eval_predicate<AB: AirBuilder<F = F>>(b: &mut AB, row: &[AB::Var], public: &[E<AB>]) {
    let mut first = b.when_first_row();
    for bit in &row[VBITS..END] {
        first.assert_bool(*bit);
    }
    first.assert_eq(row[VALUE], recompose::<AB>(&row[VBITS..DBITS]));
    first.assert_eq(row[DIFF], recompose::<AB>(&row[DBITS..END]));
    // Lagrange selectors on the public tag. They partition unity only on
    // {0, 1, 2}, so the tag is pinned there rather than trusted to arrive
    // from `Predicate::parts`.
    let tag = public[2 * DIGEST].clone();
    first.assert_zero(
        tag.clone() * (tag.clone() - E::<AB>::ONE) * (tag.clone() - E::<AB>::from(F::TWO)),
    );
    let bound = public[2 * DIGEST + 1].clone();
    let (one, two) = (E::<AB>::ONE, E::<AB>::from(F::TWO));
    let half = E::<AB>::from(F::TWO.inverse());
    let at_least = (tag.clone() - one.clone()) * (tag.clone() - two.clone()) * half.clone();
    let at_most = E::<AB>::ZERO - tag.clone() * (tag.clone() - two);
    let equal = tag.clone() * (tag - one) * half;
    let (value, diff): (E<AB>, E<AB>) = (row[VALUE].into(), row[DIFF].into());
    first.assert_zero(at_least * (value.clone() - bound.clone() - diff.clone()));
    first.assert_zero(at_most * (bound.clone() - value.clone() - diff));
    first.assert_zero(equal * (value - bound));
}

/// The trace, without checks.
#[must_use]
pub fn trace(w: &DisclosureWitness, predicate: Predicate) -> p3_matrix::dense::RowMajorMatrix<F> {
    let mut statement = w.subject.to_vec();
    statement.extend_from_slice(&w.blinding);
    let diff = predicate.diff(w.value);
    statement.push(F::from_u32(w.value));
    statement.push(F::from_u32(w.schema));
    statement.push(F::from_u32(diff));
    statement.extend((0..VALUE_BITS).map(|i| F::from_bool((w.value >> i) & 1 == 1)));
    statement.extend((0..VALUE_BITS).map(|i| F::from_bool((diff >> i) & 1 == 1)));
    dual::trace(
        (
            first_state(w.value, w.schema, &w.subject),
            w.blinding,
            &w.credential_path,
        ),
        (
            revocation_first(false),
            [F::ZERO; DIGEST],
            &w.revocation_path,
        ),
        &statement,
    )
}

/// Proves a disclosure.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] if the predicate does not hold, the value is out
/// of range, the paths disagree on the index, or a path is the wrong depth.
pub fn prove(w: &DisclosureWitness, public: &DisclosurePublic) -> Result<Proof, ZkError> {
    if w.value > MAX_VALUE || !public.predicate.holds(w.value) {
        return Err(ZkError::Unsatisfied("the predicate does not hold"));
    }
    if w.credential_path.index != w.revocation_path.index
        || w.credential_path.siblings.len() != dual::DEPTH
        || w.revocation_path.siblings.len() != dual::DEPTH
    {
        return Err(ZkError::Unsatisfied(
            "paths must share an index at depth 16",
        ));
    }
    let leaf = credential_leaf(&w.subject, w.schema, w.value, &w.blinding);
    if w.credential_path.root(&leaf) != public.issuer_root
        || w.revocation_path.root(&revocation_leaf(false)) != public.revocation_root
    {
        return Err(ZkError::Unsatisfied(
            "not an unrevoked credential under these roots",
        ));
    }
    crate::prove(
        &DisclosureAir::default(),
        trace(w, public.predicate),
        &public.values()?,
    )
}

/// Verifies a disclosure.
///
/// # Errors
///
/// [`ZkError::Rejected`] or [`ZkError::Malformed`].
pub fn verify(proof: &Proof, public: &DisclosurePublic) -> Result<(), ZkError> {
    crate::verify(&DisclosureAir::default(), proof, &public.values()?)
}

/// 32 bytes as a digest: eight little-endian words, each reduced below p.
/// For identifiers and subjects that arrive as bytes; not injective above p,
/// which for a hash-derived identifier is a 2^-4 per-word nudge, not a
/// collision an adversary can steer.
#[must_use]
pub fn digest_from_bytes(bytes: &[u8; 32]) -> Digest {
    core::array::from_fn(|i| {
        let w = u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().expect("4 bytes"));
        F::from_u32(w % crate::hash::MODULUS)
    })
}

/// The witness for credential `index`, from the issuer's two leaf lists.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] for an index outside either list.
pub fn witness_for(
    subject: Digest,
    schema: u32,
    value: u32,
    blinding: Digest,
    index: u64,
    credentials: &[Digest],
    revocations: &[Digest],
) -> Result<DisclosureWitness, ZkError> {
    let path = |leaves: &[Digest]| -> Result<MerklePath, ZkError> {
        let full = crate::pool::tree::merkle_path(leaves, index)?;
        Ok(MerklePath {
            siblings: full.siblings[..dual::DEPTH].to_vec(),
            index,
        })
    };
    Ok(DisclosureWitness {
        subject,
        schema,
        value,
        blinding,
        credential_path: path(credentials)?,
        revocation_path: path(revocations)?,
    })
}

/// The root of a depth-16 tree over `leaves` (issuer and revocation trees).
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] for an empty list.
pub fn tree_root(leaves: &[Digest]) -> Result<Digest, ZkError> {
    let first = *leaves.first().ok_or(ZkError::Unsatisfied("empty tree"))?;
    let full = crate::pool::tree::merkle_path(leaves, 0)?;
    Ok(MerklePath {
        siblings: full.siblings[..dual::DEPTH].to_vec(),
        index: 0,
    }
    .root(&first))
}
