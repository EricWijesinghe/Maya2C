//! TRaccoon-128's parameters: the paper's Table 2 row for kappa = 128, with
//! the modulus and sizes the authors' reference (`masksign/ec24-thrc`) uses.
//!
//! The modulus is Raccoon's, `(63·2^18 + 1)(127·2^18 + 1)`, not a prime: the
//! reference inherits it from the Raccoon NIST submission. It does **not**
//! meet the paper's own Lemma 3.2 condition at `NU_T = 37` (`q / 2^37`
//! floors to 4000 and rounds to 4001). The effect is bounded and rare --
//! ADR-014 works it out -- and the port follows the reference rather than
//! the lemma, because the reference is what the vectors come from.

/// Ring degree: `R_q = Z_q[x] / (x^N + 1)`.
pub const N: usize = 512;
/// The modulus.
pub const Q: u64 = 549_824_583_172_097;
/// Bits in `Q`.
pub const Q_BITS: u32 = 49;
/// Bits dropped from the public key.
pub const NU_T: u32 = 37;
/// Bits dropped from the commitment.
pub const NU_W: u32 = 40;
/// `Q >> NU_T`, the public key's rounded modulus.
pub const Q_T: u64 = Q >> NU_T;
/// `Q >> NU_W`, the commitment's rounded modulus.
pub const Q_W: u64 = Q >> NU_W;
/// Columns of `A` (the secret's dimension).
pub const ELL: usize = 4;
/// Rows of `A`.
pub const K: usize = 5;
/// Hamming weight of a challenge.
pub const OMEGA: usize = 19;
/// `log2(sigma_t)`: the key's Gaussian width.
pub const LG_SIGMA_T: u32 = 20;
/// `log2(sigma_w * sqrt(T))`: the per-signature width, shared across signers.
pub const LG_SIGMA_W_SQRT_T: u32 = 42;
/// The largest threshold the parameters are proven for.
pub const MAX_T: usize = 1024;

/// Pre-image resistance, bytes (`kappa / 8`).
pub const SEC: usize = 16;
/// Collision resistance, bytes.
pub const CRH: usize = 32;
/// The seed `A` expands from.
pub const A_SEED_LEN: usize = 16;
/// Key material one `random_bytes` draw supplies.
pub const KEY_LEN: usize = 32;
/// A pairwise seed.
pub const PAIR_SEED_LEN: usize = SEC;
/// A round-2 MAC.
pub const MAC_LEN: usize = SEC;
/// The message digest `mu` a caller signs.
pub const MU_LEN: usize = CRH;
/// A session id.
pub const SID_LEN: usize = CRH;

/// The two-norm bound B2 on `(z, 2^NU_W · h)`, as the reference computes it
/// (`_compute_b2_bound`): `e^(1/4) (omega sigma_t + sigma_w sqrt(T))
/// sqrt(n (k + l)) + (2^(nu_w + 1) + omega 2^nu_t) sqrt(n k)`.
///
/// A float because the reference's is, and this bound is where the
/// accept/reject decision is made. Custody is not a consensus path, and IEEE
/// arithmetic on these operands is the same on every machine.
// Float arithmetic because the reference's is (this decision is made in
// Python floats there). Every integer converted here is exact in an f64:
// powers of two up to 2^84 and counts below 2^13.
#[allow(clippy::cast_precision_loss)]
#[must_use]
pub fn b2() -> f64 {
    const EXP_QUARTER: f64 = 1.284_025_416_687_741_5;
    let sigma_t = f64::from(1u32 << LG_SIGMA_T);
    let sigma_w_sqrt_t = (1u64 << LG_SIGMA_W_SQRT_T) as f64;
    let omega = OMEGA as f64;
    EXP_QUARTER * (omega * sigma_t + sigma_w_sqrt_t) * ((N * (K + ELL)) as f64).sqrt()
        + ((1u64 << (NU_W + 1)) as f64 + omega * (1u64 << NU_T) as f64) * ((N * K) as f64).sqrt()
}
