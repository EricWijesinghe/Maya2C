//! Joinsplit witness → trace and public values, with the native checks.

use p3_field::PrimeCharacteristicRing as _;
use p3_matrix::dense::RowMajorMatrix;

use crate::ZkError;
use crate::gadgets::merkle::MerklePath;
use crate::gadgets::{Domain, assemble, domain, state};
use crate::hash::{DIGEST, Digest, F, WIDTH, compress};

use super::joinsplit::{
    self, BIT, BITS, CARRY, EXTRA, INPUTS, OUTPUTS, REGISTERS, SIB, in_reg, input, out_reg, output,
    public,
};
use super::note::{
    LIMB_BITS, MAX_VALUE, NONCE, Note, VALUE_BITS, limbs, nullifier_right, value_digest,
};
use super::tree::TREE_DEPTH;

/// One input: a note, its owner's key, its path, and whether it is a pad.
#[derive(Clone, Debug)]
pub struct SpendWitness {
    /// The note.
    pub note: Note,
    /// Its owner's spending key, as field elements.
    pub sk: Digest,
    /// Its path under the anchor (ignored, but still hashed, for a dummy).
    pub path: MerklePath,
    /// A zero-value pad rather than a real note.
    pub is_dummy: bool,
}

/// Everything a joinsplit proof is made from.
#[derive(Clone, Debug)]
pub struct JoinSplitWitness {
    /// The two inputs.
    pub inputs: [SpendWitness; INPUTS],
    /// The two outputs.
    pub outputs: [Note; OUTPUTS],
    /// Tree root the real inputs are under.
    pub anchor: Digest,
    /// Value entering the pool.
    pub public_in: u64,
    /// Value leaving the pool.
    pub public_out: u64,
    /// Fee.
    pub fee: u64,
    /// Recipient of `public_out`.
    pub recipient: [u8; 32],
}

/// What a verifier sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JoinSplitPublic {
    /// Anchor.
    pub anchor: Digest,
    /// Nullifiers.
    pub nullifiers: [Digest; INPUTS],
    /// Output commitments.
    pub commitments: [Digest; OUTPUTS],
    /// Value entering the pool.
    pub public_in: u64,
    /// Value leaving the pool.
    pub public_out: u64,
    /// Fee.
    pub fee: u64,
    /// Recipient of `public_out`.
    pub recipient: [u8; 32],
}

impl JoinSplitPublic {
    /// The public-value vector, in the AIR's order.
    ///
    /// # Errors
    ///
    /// [`ZkError::Unsatisfied`] if an amount exceeds `2^54 − 1` — checked
    /// here, natively, because amounts are public.
    pub fn to_field_elements(&self) -> Result<Vec<F>, ZkError> {
        if self.public_in > MAX_VALUE || self.public_out > MAX_VALUE || self.fee > MAX_VALUE {
            return Err(ZkError::Unsatisfied("a public amount exceeds 2^54 - 1"));
        }
        let mut p = vec![F::ZERO; public::LEN];
        p[public::ANCHOR..public::ANCHOR + DIGEST].copy_from_slice(&self.anchor);
        for i in 0..INPUTS {
            let at = public::NULLIFIER + i * DIGEST;
            p[at..at + DIGEST].copy_from_slice(&self.nullifiers[i]);
        }
        for j in 0..OUTPUTS {
            let at = public::COMMITMENT + j * DIGEST;
            p[at..at + DIGEST].copy_from_slice(&self.commitments[j]);
        }
        p[public::PUBLIC_IN..public::PUBLIC_IN + 2].copy_from_slice(&limbs(self.public_in));
        p[public::PUBLIC_OUT..public::PUBLIC_OUT + 2].copy_from_slice(&limbs(self.public_out));
        p[public::FEE..public::FEE + 2].copy_from_slice(&limbs(self.fee));
        for (k, chunk) in self.recipient.chunks(3).enumerate() {
            let word = chunk
                .iter()
                .rev()
                .fold(0u32, |acc, b| (acc << 8) | u32::from(*b));
            p[public::RECIPIENT + k] = F::from_u32(word);
        }
        Ok(p)
    }
}

fn bits(value: u64) -> impl Iterator<Item = F> {
    (0..VALUE_BITS).map(move |i| F::from_bool((value >> i) & 1 == 1))
}

/// The carry `c` in `A_lo + 4·2^27 = B_lo + c·2^27`.
fn carry(w: &JoinSplitWitness) -> u64 {
    let lo = |v: u64| v & ((1 << LIMB_BITS) - 1);
    let a: u64 = w.inputs.iter().map(|s| lo(s.note.value)).sum::<u64>() + lo(w.public_in);
    let b: u64 = w.outputs.iter().map(|n| lo(n.value)).sum::<u64>() + lo(w.public_out) + lo(w.fee);
    (a + (4 << LIMB_BITS) - b) >> LIMB_BITS
}

fn registers(w: &JoinSplitWitness) -> Vec<F> {
    let mut r = vec![F::ZERO; REGISTERS];
    for (i, s) in w.inputs.iter().enumerate() {
        let at = in_reg(i);
        r[at + input::SK..at + input::SK + DIGEST].copy_from_slice(&s.sk);
        r[at + input::RHO..at + input::RHO + NONCE].copy_from_slice(&s.note.rho);
        r[at + input::RAND..at + input::RAND + NONCE].copy_from_slice(&s.note.rand);
        r[at + input::V..at + input::V + 2].copy_from_slice(&limbs(s.note.value));
        r[at + input::DUMMY] = F::from_bool(s.is_dummy);
    }
    for (j, n) in w.outputs.iter().enumerate() {
        let at = out_reg(j);
        r[at + output::ADDRESS..at + output::ADDRESS + DIGEST].copy_from_slice(&n.address.0);
        r[at + output::RHO..at + output::RHO + NONCE].copy_from_slice(&n.rho);
        r[at + output::RAND..at + output::RAND + NONCE].copy_from_slice(&n.rand);
        r[at + output::V..at + output::V + 2].copy_from_slice(&limbs(n.value));
    }
    let c = carry(w);
    for k in 0..3 {
        r[CARRY + k] = F::from_bool((c >> k) & 1 == 1);
    }
    r
}

/// Row states and per-row (bit, sibling) for one input.
fn input_rows(s: &SpendWitness) -> (Vec<[F; WIDTH]>, Vec<(F, Digest)>) {
    let key = domain(Domain::Key);
    let address = compress(&s.sk, &key);
    let mut states = vec![
        state(&s.sk, &key),
        state(&value_digest(s.note.value), &address),
        state(
            &compress(&value_digest(s.note.value), &address),
            &s.note.nonce_digest(),
        ),
    ];
    let mut extra = vec![(F::ZERO, [F::ZERO; DIGEST]); 3];
    let mut cur = compress(
        &compress(&value_digest(s.note.value), &address),
        &s.note.nonce_digest(),
    );
    for (level, sib) in s.path.siblings.iter().enumerate() {
        let right = (s.path.index >> level) & 1 == 1;
        let st = if right {
            state(sib, &cur)
        } else {
            state(&cur, sib)
        };
        states.push(st);
        extra.push((F::from_bool(right), *sib));
        cur = crate::hash::permute(st)[..DIGEST].try_into().expect("8");
    }
    states.push(state(&s.sk, &nullifier_right(&s.note.rho)));
    extra.push((F::ZERO, [F::ZERO; DIGEST]));
    (states, extra)
}

/// The trace, without any check — the negative tests build bad traces here.
#[must_use]
pub fn trace(w: &JoinSplitWitness) -> RowMajorMatrix<F> {
    let mut states = Vec::with_capacity(joinsplit::ROWS);
    let mut per_row = Vec::with_capacity(joinsplit::ROWS);
    for s in &w.inputs {
        let (st, ex) = input_rows(s);
        states.extend(st);
        per_row.extend(ex);
    }
    for n in &w.outputs {
        states.push(state(&value_digest(n.value), &n.address.0));
        states.push(state(&n.value_layer(), &n.nonce_digest()));
    }
    let regs = registers(w);
    let extras: Vec<Vec<F>> = (0..joinsplit::ROWS)
        .map(|row| {
            let mut x = vec![F::ZERO; EXTRA];
            x[..REGISTERS].copy_from_slice(&regs);
            if let Some((bit, sib)) = per_row.get(row) {
                x[BIT] = *bit;
                x[SIB..SIB + DIGEST].copy_from_slice(sib);
            }
            if row == 0 {
                let values = w
                    .inputs
                    .iter()
                    .map(|s| s.note.value)
                    .chain(w.outputs.iter().map(|n| n.value));
                for (k, value) in values.enumerate() {
                    for (b, bit) in bits(value).enumerate() {
                        x[BITS + k * VALUE_BITS + b] = bit;
                    }
                }
            }
            x
        })
        .collect();
    assemble(&states, &extras, EXTRA, joinsplit::ROWS)
}

/// The public statement the witness proves.
#[must_use]
pub fn public_of(w: &JoinSplitWitness) -> JoinSplitPublic {
    JoinSplitPublic {
        anchor: w.anchor,
        nullifiers: core::array::from_fn(|i| w.inputs[i].note.nullifier(&w.inputs[i].sk)),
        commitments: core::array::from_fn(|j| w.outputs[j].commitment()),
        public_in: w.public_in,
        public_out: w.public_out,
        fee: w.fee,
        recipient: w.recipient,
    }
}

/// The native checks every honest witness passes.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`], naming the first check that failed.
pub fn check(w: &JoinSplitWitness) -> Result<(), ZkError> {
    for s in &w.inputs {
        if s.path.siblings.len() != TREE_DEPTH {
            return Err(ZkError::Unsatisfied("path is not the tree's depth"));
        }
        if s.is_dummy && s.note.value != 0 {
            return Err(ZkError::Unsatisfied("a dummy input must carry no value"));
        }
        if !s.is_dummy {
            if compress(&s.sk, &domain(Domain::Key)) != s.note.address.0 {
                return Err(ZkError::Unsatisfied(
                    "the spending key does not own an input note",
                ));
            }
            if s.path.root(&s.note.commitment()) != w.anchor {
                return Err(ZkError::Unsatisfied(
                    "an input note is not under the anchor",
                ));
            }
        }
    }
    let values = w
        .inputs
        .iter()
        .map(|s| s.note.value)
        .chain(w.outputs.iter().map(|n| n.value));
    if values
        .chain([w.public_in, w.public_out, w.fee])
        .any(|v| v > MAX_VALUE)
    {
        return Err(ZkError::Unsatisfied("a value exceeds 2^54 - 1"));
    }
    let a = w
        .inputs
        .iter()
        .map(|s| u128::from(s.note.value))
        .sum::<u128>()
        + u128::from(w.public_in);
    let b = w.outputs.iter().map(|n| u128::from(n.value)).sum::<u128>()
        + u128::from(w.public_out)
        + u128::from(w.fee);
    if a != b {
        return Err(ZkError::Unsatisfied("inputs do not equal outputs plus fee"));
    }
    if w.inputs[0].note.nullifier(&w.inputs[0].sk) == w.inputs[1].note.nullifier(&w.inputs[1].sk) {
        return Err(ZkError::Unsatisfied("both inputs are the same note"));
    }
    Ok(())
}
