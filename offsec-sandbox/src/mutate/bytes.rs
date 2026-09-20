//! Byte-level mutations, the fallback when a seed does not decode.
//!
//! Deterministic given the RNG. These are the classic libFuzzer moves —
//! bit flips, boundary-value splats, chunk splices, length games — kept here so
//! the typed mutators can reach for them when they have nothing structured to
//! work with.

use rand_core::RngCore;

use crate::Rng;

/// A `u64` value chosen from a set biased toward the boundaries that break
/// arithmetic: 0, 1, max, and one either side of the halfway split.
#[must_use]
pub fn boundary_u64(rng: &mut Rng) -> u64 {
    const EDGES: [u64; 8] = [
        0,
        1,
        u64::MAX,
        u64::MAX - 1,
        i64::MAX as u64,
        i64::MAX as u64 + 1,
        1 << 32,
        (1 << 32) - 1,
    ];
    // Half the time a boundary, half the time a full-range value, so both the
    // edge cases and the ordinary interior are covered.
    if rng.next_u32() & 1 == 0 {
        EDGES[(rng.next_u32() as usize) % EDGES.len()]
    } else {
        rng.next_u64()
    }
}

/// Mutates `seed` with one randomly chosen byte-level operation.
#[must_use]
pub fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut out = seed.to_vec();
    match rng.next_u32() % 6 {
        0 => flip_bits(&mut out, rng),
        1 => splat_boundary(&mut out, rng),
        2 => truncate(&mut out, rng),
        3 => extend(&mut out, rng),
        4 => splice(&mut out, rng),
        _ => {
            // A fresh random buffer, so an empty or tiny corpus still explores.
            out.clear();
            let len = (rng.next_u32() % 256) as usize;
            out.resize(len, 0);
            rng.fill_bytes(&mut out);
        }
    }
    out
}

fn flip_bits(out: &mut [u8], rng: &mut Rng) {
    if out.is_empty() {
        return;
    }
    let flips = 1 + rng.next_u32() % 8;
    for _ in 0..flips {
        let index = (rng.next_u32() as usize) % out.len();
        out[index] ^= 1 << (rng.next_u32() % 8);
    }
}

fn splat_boundary(out: &mut [u8], rng: &mut Rng) {
    if out.is_empty() {
        return;
    }
    let byte = *[0x00u8, 0xff, 0x7f, 0x80, 0x01]
        .get((rng.next_u32() as usize) % 5)
        .unwrap();
    let at = (rng.next_u32() as usize) % out.len();
    // At most `out.len() - at` bytes remain from `at`, one of them `at` itself.
    let width = 1 + (rng.next_u32() as usize % 8).min(out.len() - at - 1);
    out[at..at + width].fill(byte);
}

fn truncate(out: &mut Vec<u8>, rng: &mut Rng) {
    if out.is_empty() {
        return;
    }
    let keep = (rng.next_u32() as usize) % out.len();
    out.truncate(keep);
}

fn extend(out: &mut Vec<u8>, rng: &mut Rng) {
    let extra = (rng.next_u32() % 64) as usize;
    let start = out.len();
    out.resize(start + extra, 0);
    rng.fill_bytes(&mut out[start..]);
}

fn splice(out: &mut Vec<u8>, rng: &mut Rng) {
    if out.len() < 2 {
        return;
    }
    let a = (rng.next_u32() as usize) % out.len();
    let b = (rng.next_u32() as usize) % out.len();
    let (lo, hi) = (a.min(b), a.max(b));
    let chunk: Vec<u8> = out[lo..hi].to_vec();
    let at = (rng.next_u32() as usize) % out.len();
    out.splice(at..at, chunk);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use rand_core::SeedableRng;

    use super::*;

    fn rng() -> Rng {
        Rng::seed_from_u64(1)
    }

    #[test]
    fn mutation_is_deterministic_for_a_seed() {
        let a: Vec<Vec<u8>> = (0..50)
            .scan(rng(), |r, _| Some(mutate(b"hello world", r)))
            .collect();
        let b: Vec<Vec<u8>> = (0..50)
            .scan(rng(), |r, _| Some(mutate(b"hello world", r)))
            .collect();
        assert_eq!(a, b);
    }

    #[test]
    fn every_operation_terminates_on_empty_and_tiny_input() {
        let mut r = rng();
        for _ in 0..1000 {
            let _ = mutate(&[], &mut r);
            let _ = mutate(&[0x41], &mut r);
        }
    }

    #[test]
    fn boundary_u64_reaches_the_edges() {
        let mut r = rng();
        let seen: std::collections::HashSet<u64> =
            (0..10_000).map(|_| boundary_u64(&mut r)).collect();
        for edge in [0, 1, u64::MAX] {
            assert!(seen.contains(&edge));
        }
    }
}
