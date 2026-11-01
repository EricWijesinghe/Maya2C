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

/// CON-11: blocks the median is taken over.
pub const MEDIAN_TIME_SPAN: usize = 11;

/// CON-10: the only header version defined is 0.
#[must_use]
pub const fn header_version_ok(version: u8) -> bool {
    version == 0
}

/// CON-12: the largest block, in wire bytes (ADR-072, provisional). Mirrors
/// `maya_fee_market::limits::MAX_BLOCK_BYTES`; the conformance runner checks
/// that the node's constant is this one.
pub const MAX_BLOCK_BYTES: u64 = 5 * 1024 * 1024;

/// CON-12: the header (145 bytes) and the transaction count (8).
const BLOCK_OVERHEAD: u64 = 145 + 8;

/// CON-12: wire bytes of a block carrying transactions of these encoded
/// lengths: the overhead plus an 8-byte length prefix per transaction.
#[must_use]
pub fn block_wire_bytes(tx_lens: &[u64]) -> u64 {
    tx_lens.iter().fold(BLOCK_OVERHEAD, |sum, len| {
        sum.saturating_add(8).saturating_add(*len)
    })
}

/// CON-12: the indices first-fit admits under `cap`, if each transaction
/// executes. A transaction that would take the block over the cap is passed
/// over, and the ones after it are still considered.
#[must_use]
pub fn first_fit(tx_lens: &[u64], cap: u64) -> Vec<usize> {
    let mut total = BLOCK_OVERHEAD;
    let mut admitted = Vec::new();
    for (index, len) in tx_lens.iter().enumerate() {
        let next = total.saturating_add(8).saturating_add(*len);
        if next <= cap {
            total = next;
            admitted.push(index);
        }
    }
    admitted
}

/// CON-13: how many leading payloads the builder decodes under `budget`
/// bytes. `payload_lens` are the distinct payloads in commit order. The walk
/// stops at the first one that would take the total over; it does not pass
/// it over.
#[must_use]
pub fn decode_cut(payload_lens: &[u64], budget: u64) -> usize {
    let mut total = 0_u64;
    for (count, len) in payload_lens.iter().enumerate() {
        total = total.saturating_add(*len);
        if total > budget {
            return count;
        }
    }
    payload_lens.len()
}

/// CON-11: the median of the most recent `MEDIAN_TIME_SPAN` timestamps,
/// `recent` ordered oldest first. The upper middle for an even count.
#[must_use]
pub fn median_time_past(recent: &[u64]) -> u64 {
    let start = recent.len().saturating_sub(MEDIAN_TIME_SPAN);
    let mut window = recent[start..].to_vec();
    window.sort_unstable();
    window.get(window.len() / 2).copied().unwrap_or(0)
}

/// CON-11: a timestamp must be later than the median.
#[must_use]
pub const fn timestamp_ok(timestamp: u64, median: u64) -> bool {
    timestamp > median
}

/// CON-9: under DAG-BFT a block's difficulty target is its parent's, so every
/// block carries the genesis target. There is no retarget and no activation
/// pin.
#[must_use]
pub const fn dag_bft_target(parent: &[u8; 32]) -> [u8; 32] {
    *parent
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

/// CON-13: deficit round-robin over authors. `entries` are `(author, length,
/// key)` in commit order; a repeat has the same key. Each turn adds
/// `budget / committee` to every waiting author, in ascending author order.
/// An author admits from the front of its queue while the next payload fits
/// its deficit and the budget left, and finishes when its queue is empty or
/// its next payload is larger than the budget left. Returns the admitted
/// indices in commit order, and the bytes charged.
#[must_use]
pub fn decode_fair(entries: &[(u16, u64, u64)], committee: u64, budget: u64) -> (Vec<usize>, u64) {
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    let mut queues: BTreeMap<u16, VecDeque<usize>> = BTreeMap::new();
    for (index, (author, _, _)) in entries.iter().enumerate() {
        queues.entry(*author).or_default().push_back(index);
    }
    let quantum = (budget / committee.max(1)).max(1);
    let mut deficit: BTreeMap<u16, u64> = BTreeMap::new();
    let mut remaining = budget;
    let mut seen: BTreeSet<u64> = BTreeSet::new();
    let mut admitted: Vec<usize> = Vec::new();
    while !queues.is_empty() {
        let mut finished = Vec::new();
        for (author, queue) in &mut queues {
            let d = deficit.entry(*author).or_insert(0);
            *d = d.saturating_add(quantum);
            loop {
                let Some(&index) = queue.front() else {
                    finished.push(*author);
                    break;
                };
                let (_, len, key) = entries[index];
                if seen.contains(&key) {
                    queue.pop_front();
                    continue;
                }
                if len > remaining {
                    finished.push(*author);
                    break;
                }
                if len > *d {
                    break;
                }
                *d -= len;
                remaining -= len;
                seen.insert(key);
                admitted.push(index);
                queue.pop_front();
            }
        }
        for author in finished {
            queues.remove(&author);
        }
    }
    admitted.sort_unstable();
    (admitted, budget - remaining)
}
