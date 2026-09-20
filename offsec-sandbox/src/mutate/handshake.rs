//! Handshake frame mutation, structure-aware.
//!
//! The post-quantum transport handshake is a fixed layout: a version byte, then
//! key material of a known length (`handshake::respond`/`initiate` read exactly
//! `RESPONDER_MESSAGE_LEN`/`INITIATOR_MESSAGE_LEN`). The mutator builds frames
//! of those shapes and perturbs the fields a peer controls — the version byte,
//! the declared length, and the body — so the reader's length and version
//! checks are exercised against frames that are *almost* right.
//!
//! The single-KEM (`/maya/mlkem/1.0.0`) and dual-KEM (`/maya/dualkem/1.0.0`)
//! message lengths both come from the node's own public constants, so a change
//! to either is reflected here rather than drifting.

use custom_l1_node::network::pq::{dual, handshake};
use rand_core::RngCore;

use super::bytes;
use crate::Rng;

/// The frame shapes the mutator knows: `(version, total_len)`.
fn shapes() -> [(u8, usize); 4] {
    [
        (handshake::VERSION, handshake::RESPONDER_MESSAGE_LEN),
        (handshake::VERSION, handshake::INITIATOR_MESSAGE_LEN),
        (dual::VERSION, dual::RESPONDER_MESSAGE_LEN),
        (dual::VERSION, dual::INITIATOR_MESSAGE_LEN),
    ]
}

/// Mutates one handshake frame.
#[must_use]
pub fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    // A quarter of the time, byte-level on whatever the seed is; the rest,
    // build a shaped frame and perturb one field.
    if seed.is_empty() || rng.next_u32().is_multiple_of(4) {
        return shaped_then_perturb(seed, rng);
    }
    bytes::mutate(seed, rng)
}

fn shaped_then_perturb(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let shapes = shapes();
    let (version, len) = shapes[(rng.next_u32() as usize) % shapes.len()];
    let mut frame = vec![0u8; len];
    if !frame.is_empty() {
        frame[0] = version;
        // Body from the seed where it reaches, RNG for the rest, so the frame
        // is well-formed but varied.
        for (i, slot) in frame.iter_mut().enumerate().skip(1) {
            *slot = seed
                .get(i)
                .copied()
                .unwrap_or_else(|| (rng.next_u32() & 0xff) as u8);
        }
    }

    match rng.next_u32() % 4 {
        // A version the reader does not speak.
        0 if !frame.is_empty() => frame[0] = frame[0].wrapping_add(1),
        // One byte short: the reader must not read past a truncated frame.
        1 if !frame.is_empty() => {
            frame.pop();
        }
        // One byte long: a trailing byte the reader must reject or ignore.
        2 => frame.push((rng.next_u32() & 0xff) as u8),
        // Untouched shaped frame: the reader's success path on random key bytes.
        _ => {}
    }
    frame
}

/// Seed frames: one well-formed frame of each shape.
#[must_use]
pub fn seeds() -> Vec<Vec<u8>> {
    shapes()
        .into_iter()
        .map(|(version, len)| {
            let mut frame = vec![0u8; len];
            if !frame.is_empty() {
                frame[0] = version;
            }
            frame
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use rand_core::SeedableRng;

    use super::*;

    #[test]
    fn seeds_carry_the_declared_version_and_length() {
        for (frame, (version, len)) in seeds().into_iter().zip(shapes()) {
            assert_eq!(frame.len(), len);
            assert_eq!(frame[0], version);
        }
    }

    #[test]
    fn mutation_is_deterministic() {
        let seed = &seeds()[0];
        let a: Vec<_> = (0..64)
            .scan(Rng::seed_from_u64(5), |r, _| Some(mutate(seed, r)))
            .collect();
        let b: Vec<_> = (0..64)
            .scan(Rng::seed_from_u64(5), |r, _| Some(mutate(seed, r)))
            .collect();
        assert_eq!(a, b);
    }
}
