//! `spec/05-consensus.md` CON-5 to CON-8, and `spec/02-transactions.md` TX-4.
//!
//! 256-bit arithmetic is written out here on four little-endian `u64` limbs,
//! slowly and plainly, rather than borrowed from the node's `U256`.

/// Blocks between retargets (CON-5).
pub const RETARGET_INTERVAL: u64 = 100;
/// Intended seconds per block (CON-5).
pub const TARGET_BLOCK_TIME: u64 = 15;
/// Intended seconds per window.
pub const EXPECTED_TIMESPAN: u64 = RETARGET_INTERVAL * TARGET_BLOCK_TIME;
/// Largest factor one retarget may move the target by.
pub const MAX_ADJUSTMENT: u64 = 4;

type Limbs = [u64; 4];

/// The low 64 bits.
fn low(x: u128) -> u64 {
    u64::try_from(x & u128::from(u64::MAX)).unwrap_or(0)
}

fn from_be(bytes: &[u8; 32]) -> Limbs {
    let mut limbs = [0u64; 4];
    for (i, limb) in limbs.iter_mut().enumerate() {
        let start = 32 - 8 * (i + 1);
        *limb = u64::from_be_bytes(bytes[start..start + 8].try_into().unwrap_or([0; 8]));
    }
    limbs
}

fn to_be(limbs: &Limbs) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, limb) in limbs.iter().enumerate() {
        let start = 32 - 8 * (i + 1);
        out[start..start + 8].copy_from_slice(&limb.to_be_bytes());
    }
    out
}

/// CON-5: `previous × clamp(timespan) ÷ EXPECTED_TIMESPAN`, the timespan
/// clamped to `[EXPECTED/4, EXPECTED×4]`; a result above `pow_limit` (or too
/// wide for 256 bits) becomes `pow_limit`, and a zero result becomes 1.
#[must_use]
pub fn retarget(previous: &[u8; 32], timespan: u64, pow_limit: &[u8; 32]) -> [u8; 32] {
    let clamped = timespan.clamp(
        EXPECTED_TIMESPAN / MAX_ADJUSTMENT,
        EXPECTED_TIMESPAN * MAX_ADJUSTMENT,
    );
    // Five limbs: the product of a 256-bit and a 64-bit number.
    let mut wide = [0u64; 5];
    let mut carry = 0u128;
    for (i, limb) in from_be(previous).iter().enumerate() {
        let p = u128::from(*limb) * u128::from(clamped) + carry;
        wide[i] = low(p);
        carry = p >> 64;
    }
    wide[4] = low(carry);
    let mut rem = 0u128;
    for limb in wide.iter_mut().rev() {
        let cur = (rem << 64) | u128::from(*limb);
        // `rem` < EXPECTED_TIMESPAN, so the quotient fits 64 bits.
        *limb = low(cur / u128::from(EXPECTED_TIMESPAN));
        rem = cur % u128::from(EXPECTED_TIMESPAN);
    }
    if wide[4] != 0 {
        return *pow_limit;
    }
    let next = to_be(&[wide[0], wide[1], wide[2], wide[3]]);
    if next > *pow_limit {
        *pow_limit
    } else if next == [0; 32] {
        let mut one = [0u8; 32];
        one[31] = 1;
        one
    } else {
        next
    }
}

/// CON-6: a proof-of-work hash meets a target when, big-endian, it is not
/// above it.
#[must_use]
pub fn meets_target(hash: &[u8; 32], target: &[u8; 32]) -> bool {
    hash <= target
}

/// CON-7: the work a target stands for, `2^256 ÷ (target + 1)`, by binary
/// long division. The all-zero target's work, `2^256`, does not fit in 256
/// bits and saturates to `2^256 - 1`, as the node's does.
#[must_use]
pub fn work(target: &[u8; 32]) -> [u8; 32] {
    let mut divisor = from_be(target);
    let mut carry = true;
    for limb in &mut divisor {
        let (sum, over) = limb.overflowing_add(u64::from(carry));
        *limb = sum;
        carry = over;
    }
    if carry {
        // target + 1 = 2^256: the work is exactly 1.
        let mut one = [0u8; 32];
        one[31] = 1;
        return one;
    }
    if divisor == [1, 0, 0, 0] {
        return [0xFF; 32];
    }
    // Numerator 2^256: a one followed by 256 zero bits.
    let (mut quotient, mut remainder) = ([0u64; 4], [0u64; 4]);
    for bit in (0..=256).rev() {
        let top = remainder[3] >> 63;
        remainder = shl1(&remainder, u64::from(bit == 256));
        if top == 1 || ge(&remainder, &divisor) {
            remainder = sub(&remainder, &divisor);
            if bit < 256 {
                quotient[bit / 64] |= 1 << (bit % 64);
            }
        }
    }
    to_be(&quotient)
}

fn shl1(x: &Limbs, low: u64) -> Limbs {
    [
        x[0] << 1 | low,
        x[1] << 1 | x[0] >> 63,
        x[2] << 1 | x[1] >> 63,
        x[3] << 1 | x[2] >> 63,
    ]
}

fn ge(a: &Limbs, b: &Limbs) -> bool {
    a.iter().rev().cmp(b.iter().rev()) != std::cmp::Ordering::Less
}

fn sub(a: &Limbs, b: &Limbs) -> Limbs {
    let mut out = [0u64; 4];
    let mut borrow = false;
    for i in 0..4 {
        let (d, b1) = a[i].overflowing_sub(b[i]);
        let (d, b2) = d.overflowing_sub(u64::from(borrow));
        out[i] = d;
        borrow = b1 || b2;
    }
    out
}

/// A branch's cumulative work: the sum of [`work`] over its targets.
#[must_use]
pub fn cumulative_work(targets: &[[u8; 32]]) -> [u8; 32] {
    let mut total = [0u64; 4];
    for t in targets {
        let w = from_be(&work(t));
        let mut carry = false;
        for i in 0..4 {
            let (s, c1) = total[i].overflowing_add(w[i]);
            let (s, c2) = s.overflowing_add(u64::from(carry));
            total[i] = s;
            carry = c1 || c2;
        }
    }
    to_be(&total)
}

/// CON-8: a block at or below the prune horizon is refused.
#[must_use]
pub fn below_prune_horizon(height: u64, horizon: u64) -> bool {
    height <= horizon
}

/// TX-4: which verification path accepts a transaction of `version`.
/// `call` is `"verify"` (no height) or `"verify_at"`. Returns `Err` with the
/// reason for a refusal.
///
/// # Errors
///
/// `NeedsHeight` for a v7/v8 frame through `verify`; `BadSignature` when the
/// signature does not verify.
pub fn verification(version: u8, call: &str, signature_valid: bool) -> Result<(), &'static str> {
    let suite_frame = matches!(version, 7 | 8);
    if suite_frame && call == "verify" {
        return Err("NeedsHeight");
    }
    if !signature_valid {
        return Err("BadSignature");
    }
    Ok(())
}
