//! The step that leaves the CPU: `w'_approx = NTT^-1(A_hat o NTT(z) - NTT(c) o NTT(t1 * 2^d))`
//! (FIPS 204 Algorithm 8, line 9), with its intermediates.
//!
//! Also the buffer layout both sides agree on, so the stale-buffer check can be
//! tested on a machine with no GPU.

use crate::arith::{Poly, add_mod, inv_ntt, mul_mod, ntt, sub_mod};
use crate::error::VerifyError;
use crate::params::{K, L, N};

/// Items per dispatch. Small on purpose (threat model G-8, ADR-137 S-12): one
/// dispatch must finish well inside an operating system's GPU watchdog, which
/// resets the whole card, miner included. Not tuned toward a throughput.
pub const MAX_DISPATCH_ITEMS: usize = 64;

/// Input words per item: `z`, `c`, `t1 * 2^d`, then `A_hat` row-major.
pub const WORDS_IN: usize = (L + 1 + K + K * L) * N;
/// Output words per item: a two-word header (batch nonce, item index), then
/// `z_hat`, `c_hat`, `t1_hat`, `w_approx`.
pub const WORDS_OUT: usize = HEADER_WORDS + (L + 1 + 2 * K) * N;
/// Header words at the start of each item's output.
pub const HEADER_WORDS: usize = 2;

/// Everything the lattice step reads, canonical in `0..Q`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatticeInput {
    /// The response `z`, reduced modulo `Q`.
    pub z: [Poly; L],
    /// The challenge `c` (coefficients `-1, 0, 1` as `Q-1, 0, 1`).
    pub c: Poly,
    /// `t1 * 2^d`.
    pub t1: [Poly; K],
    /// `ExpandA(rho)`, already in the NTT domain.
    pub a_hat: [[Poly; L]; K],
}

/// Everything the lattice step writes. Compared word for word, CPU against GPU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatticeOutput {
    /// `NTT(z)`.
    pub z_hat: [Poly; L],
    /// `NTT(c)`.
    pub c_hat: Poly,
    /// `NTT(t1 * 2^d)`.
    pub t1_hat: [Poly; K],
    /// `w'_approx`, the input to `UseHint`.
    pub w_approx: [Poly; K],
}

/// Which intermediate differed first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// `NTT(z)`.
    ZHat,
    /// `NTT(c)`.
    CHat,
    /// `NTT(t1 * 2^d)`.
    T1Hat,
    /// `w'_approx`.
    WApprox,
}

impl LatticeOutput {
    /// The first intermediate where `self` and `other` differ, if any.
    #[must_use]
    pub fn first_difference(&self, other: &Self) -> Option<Stage> {
        if self.z_hat != other.z_hat {
            Some(Stage::ZHat)
        } else if self.c_hat != other.c_hat {
            Some(Stage::CHat)
        } else if self.t1_hat != other.t1_hat {
            Some(Stage::T1Hat)
        } else if self.w_approx != other.w_approx {
            Some(Stage::WApprox)
        } else {
            None
        }
    }
}

/// Something that computes the lattice step for a batch.
pub trait LatticeBackend {
    /// For reports: adapter name and backend, or "cpu".
    fn name(&self) -> String;

    /// One output per input, in order. Called with at most
    /// [`MAX_DISPATCH_ITEMS`] inputs.
    ///
    /// # Errors
    ///
    /// Any failure: no device, a lost device, a timeout, a stale buffer. The
    /// caller reads every error as "unavailable", never as a verdict.
    fn run(&mut self, inputs: &[LatticeInput]) -> Result<Vec<LatticeOutput>, VerifyError>;
}

/// The CPU reference backend.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuLattice;

impl LatticeBackend for CpuLattice {
    fn name(&self) -> String {
        "cpu reference".to_owned()
    }

    fn run(&mut self, inputs: &[LatticeInput]) -> Result<Vec<LatticeOutput>, VerifyError> {
        Ok(inputs.iter().map(lattice_on_cpu).collect())
    }
}

/// The lattice step on the CPU.
#[must_use]
pub fn lattice_on_cpu(input: &LatticeInput) -> LatticeOutput {
    let z_hat: [Poly; L] = core::array::from_fn(|l| ntt(&input.z[l]));
    let c_hat = ntt(&input.c);
    let t1_hat: [Poly; K] = core::array::from_fn(|k| ntt(&input.t1[k]));
    let w_approx = core::array::from_fn(|k| {
        let mut acc = [0u32; N];
        for (n, slot) in acc.iter_mut().enumerate() {
            let sum = input.a_hat[k]
                .iter()
                .zip(&z_hat)
                .fold(0, |s, (a, z)| add_mod(s, mul_mod(a[n], z[n])));
            *slot = sub_mod(sum, mul_mod(c_hat[n], t1_hat[k][n]));
        }
        inv_ntt(&acc)
    });
    LatticeOutput {
        z_hat,
        c_hat,
        t1_hat,
        w_approx,
    }
}

/// Flattens a batch in the order the shader reads it.
#[must_use]
pub fn encode_inputs(inputs: &[LatticeInput]) -> Vec<u32> {
    let mut words = Vec::with_capacity(inputs.len() * WORDS_IN);
    for input in inputs {
        input.z.iter().for_each(|p| words.extend_from_slice(p));
        words.extend_from_slice(&input.c);
        input.t1.iter().for_each(|p| words.extend_from_slice(p));
        input
            .a_hat
            .iter()
            .flatten()
            .for_each(|p| words.extend_from_slice(p));
    }
    words
}

/// Reads a batch back, refusing it unless every item carries this batch's
/// `nonce` and its own index (threat model G-9).
///
/// # Errors
///
/// [`VerifyError::StaleBuffer`] on a short buffer, a wrong nonce or a wrong
/// index.
pub fn decode_outputs(
    words: &[u32],
    nonce: u32,
    count: usize,
) -> Result<Vec<LatticeOutput>, VerifyError> {
    if words.len() < count * WORDS_OUT {
        return Err(VerifyError::StaleBuffer("output shorter than the batch"));
    }
    let mut outputs = Vec::with_capacity(count);
    for (index, item) in words
        .as_chunks::<WORDS_OUT>()
        .0
        .iter()
        .take(count)
        .enumerate()
    {
        if item[0] != nonce {
            return Err(VerifyError::StaleBuffer("nonce is not this batch's"));
        }
        if usize::try_from(item[1]).ok() != Some(index) {
            return Err(VerifyError::StaleBuffer("item index out of place"));
        }
        let mut polys = item[HEADER_WORDS..].as_chunks::<N>().0.iter().copied();
        let mut next = || polys.next().unwrap_or([0u32; N]);
        let z_hat = core::array::from_fn(|_| next());
        let c_hat = next();
        let t1_hat = core::array::from_fn(|_| next());
        let w_approx = core::array::from_fn(|_| next());
        outputs.push(LatticeOutput {
            z_hat,
            c_hat,
            t1_hat,
            w_approx,
        });
    }
    Ok(outputs)
}

/// The inverse of [`decode_outputs`], for tests and fake backends.
#[must_use]
pub fn encode_outputs(outputs: &[LatticeOutput], nonce: u32) -> Vec<u32> {
    let mut words = Vec::with_capacity(outputs.len() * WORDS_OUT);
    for (index, out) in outputs.iter().enumerate() {
        words.push(nonce);
        words.push(u32::try_from(index).unwrap_or(u32::MAX));
        out.z_hat.iter().for_each(|p| words.extend_from_slice(p));
        words.extend_from_slice(&out.c_hat);
        out.t1_hat.iter().for_each(|p| words.extend_from_slice(p));
        out.w_approx.iter().for_each(|p| words.extend_from_slice(p));
    }
    words
}
