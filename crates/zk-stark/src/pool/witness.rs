//! Spend witness → trace, with the native checks first.

use p3_field::PrimeCharacteristicRing as _;
use p3_matrix::dense::RowMajorMatrix;

use crate::gadgets::merkle::MerklePath;
use crate::gadgets::range::{LIMB_BITS, MAX_BITS, limbs};
use crate::gadgets::{Domain, SecretDigest, assemble, domain, state};
use crate::hash::{DIGEST, Digest, F, WIDTH, compress};
use crate::{Proof, ZkError};

use super::note::{NONCE, Note, nullifier_right, value_digest};
use super::spend::{
    self, BIT, BITS_IN, BITS_OUT, CARRY, EXTRA, Mode, PK_OUT, R_IN, R_OUT, REGISTERS, RHO_IN,
    RHO_OUT, SIB, SK, SpendAir, VIN, VOUT,
};
use super::tree::DEPTH;
use super::{MAX_FEE, MAX_VALUE, SpendStatement};

/// Everything the trace is built from, already checked.
pub(crate) struct SpendWitness<'a> {
    pub mode: Mode,
    pub sk: Digest,
    pub input: &'a Note,
    pub path: &'a MerklePath,
    pub output: Option<&'a Note>,
    pub vout: u64,
    pub fee: u64,
}

fn bits(value: u64) -> Vec<F> {
    (0..MAX_BITS)
        .map(|i| F::from_bool((value >> i) & 1 == 1))
        .collect()
}

/// The register block, identical on every row.
fn registers(w: &SpendWitness<'_>, carry: bool) -> Vec<F> {
    let mut r = vec![F::ZERO; REGISTERS];
    r[SK..SK + DIGEST].copy_from_slice(&w.sk);
    r[RHO_IN..RHO_IN + NONCE].copy_from_slice(&w.input.rho);
    r[R_IN..R_IN + NONCE].copy_from_slice(&w.input.blind);
    r[VIN..VIN + 2].copy_from_slice(&limbs(w.input.value));
    if let Some(out) = w.output {
        r[PK_OUT..PK_OUT + DIGEST].copy_from_slice(&out.owner);
        r[RHO_OUT..RHO_OUT + NONCE].copy_from_slice(&out.rho);
        r[R_OUT..R_OUT + NONCE].copy_from_slice(&out.blind);
    }
    r[VOUT..VOUT + 2].copy_from_slice(&limbs(w.vout));
    r[CARRY] = F::from_bool(carry);
    r
}

/// The borrow from the high limb: needed exactly when the low limbs cannot pay.
fn carry(vin: u64, vout: u64, fee: u64) -> bool {
    let mask = (1u64 << LIMB_BITS) - 1;
    (vin & mask) < (vout & mask) + fee
}

/// Builds the spend trace without checking anything — the negative tests
/// build bad traces through this.
#[must_use]
pub(crate) fn trace(w: &SpendWitness<'_>) -> RowMajorMatrix<F> {
    let key = domain(Domain::Key);
    let pk = compress(&w.sk, &key);
    let mut states: Vec<[F; WIDTH]> = vec![
        state(&w.sk, &key),
        state(&value_digest(w.input.value), &pk),
        state(&w.input.value_layer(), &w.input.nonce_digest()),
    ];
    let mut cur = w.input.commitment();
    let mut rows_extra: Vec<(F, Digest)> = vec![(F::ZERO, [F::ZERO; DIGEST]); 3];
    for (level, sib) in w.path.siblings.iter().enumerate() {
        let right = (w.path.index >> level) & 1 == 1;
        let s = if right {
            state(sib, &cur)
        } else {
            state(&cur, sib)
        };
        states.push(s);
        rows_extra.push((F::from_bool(right), *sib));
        cur = compress(
            &s[..DIGEST].try_into().expect("8"),
            &s[DIGEST..].try_into().expect("8"),
        );
    }
    states.push(state(&w.sk, &nullifier_right(&w.input.rho)));
    if let Some(out) = w.output {
        states.push(state(&value_digest(out.value), &out.owner));
        states.push(state(&out.value_layer(), &out.nonce_digest()));
    }
    let regs = registers(w, carry(w.input.value, w.vout, w.fee));
    let extras: Vec<Vec<F>> = (0..spend::ROWS)
        .map(|row| {
            let mut x = vec![F::ZERO; EXTRA];
            x[..REGISTERS].copy_from_slice(&regs);
            if let Some((bit, sib)) = rows_extra.get(row) {
                x[BIT] = *bit;
                x[SIB..SIB + DIGEST].copy_from_slice(sib);
            }
            if row == 0 {
                x[BITS_IN..BITS_IN + MAX_BITS].copy_from_slice(&bits(w.input.value));
                x[BITS_OUT..BITS_OUT + MAX_BITS].copy_from_slice(&bits(w.vout));
            }
            x
        })
        .collect();
    assemble(&states, &extras, EXTRA, spend::ROWS)
}

/// The public statement the witness proves.
pub(crate) fn statement(w: &SpendWitness<'_>) -> SpendStatement {
    SpendStatement {
        mode: w.mode,
        root: w.path.root(&w.input.commitment()),
        nullifier: w.input.nullifier(&w.sk),
        out: match w.output {
            Some(out) => out.commitment(),
            None => value_digest(w.vout),
        },
        fee: w.fee,
    }
}

fn check(w: &SpendWitness<'_>) -> Result<(), ZkError> {
    if w.path.siblings.len() != DEPTH {
        return Err(ZkError::Unsatisfied("path is not the tree's depth"));
    }
    if compress(&w.sk, &domain(Domain::Key)) != w.input.owner {
        return Err(ZkError::Unsatisfied(
            "the spending key does not own the input note",
        ));
    }
    if w.input.value > MAX_VALUE || w.vout > MAX_VALUE || w.fee > MAX_FEE {
        return Err(ZkError::Unsatisfied("a value or the fee is out of range"));
    }
    if w.vout.checked_add(w.fee) != Some(w.input.value) {
        return Err(ZkError::Unsatisfied(
            "input value does not equal output plus fee",
        ));
    }
    Ok(())
}

pub(crate) fn prove(
    mode: Mode,
    sk: &SecretDigest,
    input: &Note,
    path: &MerklePath,
    output: Option<&Note>,
    vout: u64,
    fee: u64,
) -> Result<(Proof, SpendStatement), ZkError> {
    let w = SpendWitness {
        mode,
        sk: sk.to_field(),
        input,
        path,
        output,
        vout,
        fee,
    };
    check(&w)?;
    let statement = statement(&w);
    let proof = crate::prove(&SpendAir::new(mode), trace(&w), &statement.public())?;
    Ok((proof, statement))
}
