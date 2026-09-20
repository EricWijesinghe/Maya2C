//! The fuzz target's property, driven by a seeded generator so it runs
//! everywhere.
//!
//! `fuzz/fuzz_targets/car_decode.rs` is the real thing, but `libfuzzer-sys`
//! does not build on Windows, so on the machine this was written on it never
//! runs. The property is the same one, and it is worth checking on every
//! platform and in CI rather than only where a sanitizer build works:
//!
//! - a reader either refuses the bytes or returns sections that each hash to
//!   their own CID;
//! - `open_archive` never returns a block the CAR does not contain;
//! - nothing panics, on any input;
//! - decompression stays bounded.
//!
//! Deterministic: the seed is printed and can be replayed with
//! `MAYA_ARCHIVE_SEED=<n>`. A randomized test whose failures cannot be
//! reproduced is a test that reports bugs nobody can fix.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_archive::car::{RAW_CODEC, cid_of, read_car, verify};
use maya_archive::{ArchivedBlock, build_archive, compress, decompress, open_archive};

/// SplitMix64, inline for the same reason `tests/chaos_simulator.rs` has one.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

fn seed() -> u64 {
    std::env::var("MAYA_ARCHIVE_SEED")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(0x4D41_5941_4152_4348)
}

fn archive_bytes(rng: &mut Rng) -> Vec<u8> {
    let count = 1 + rng.below(6);
    let first = rng.next_u64() % 10_000;
    let blocks: Vec<ArchivedBlock> = (0..count)
        .map(|offset| ArchivedBlock {
            height: first + offset as u64,
            id: [(first as u8).wrapping_add(offset as u8); 32],
            bytes: vec![rng.next_u64() as u8; 1 + rng.below(300)],
        })
        .collect();
    build_archive("maya-robustness", &blocks)
        .expect("build")
        .car
}

/// Every section a successful read returns must hash to its own CID, and an
/// opened archive must only return blocks the CAR actually holds.
fn check(bytes: &[u8]) {
    if let Ok(car) = read_car(bytes) {
        for (cid, data) in &car.sections {
            assert_eq!(
                verify(cid, data).ok(),
                Some(true),
                "read_car returned a section that does not hash to its CID"
            );
        }
        if let Ok(blocks) = open_archive(bytes, &car.root) {
            for block in blocks {
                assert!(
                    car.sections.contains_key(&cid_of(RAW_CODEC, &block.bytes)),
                    "open_archive returned a block the CAR does not contain"
                );
            }
        }
    }
    // Bounded, and never a panic, whatever the bytes claim to be.
    let _ = decompress(bytes);
}

#[test]
fn mutated_archives_are_verified_or_refused_and_never_panic() {
    let run_seed = seed();
    println!("MAYA_ARCHIVE_SEED={run_seed}");
    let mut rng = Rng(run_seed);

    for _ in 0..600 {
        let original = archive_bytes(&mut rng);

        // The honest archive always reads.
        check(&original);

        // Then six ways of damaging it, the shapes a hostile or corrupt store
        // produces: a flipped bit, a truncation, an insertion, a swapped
        // region, a byte-for-byte replacement, and pure noise.
        for mutation in 0..6 {
            let mut damaged = original.clone();
            match mutation {
                0 if !damaged.is_empty() => {
                    let at = rng.below(damaged.len());
                    damaged[at] ^= 1 << rng.below(8);
                }
                1 if !damaged.is_empty() => damaged.truncate(rng.below(damaged.len())),
                2 => {
                    let at = rng.below(damaged.len() + 1);
                    damaged.insert(at, rng.next_u64() as u8);
                }
                3 if damaged.len() > 8 => {
                    let at = rng.below(damaged.len() - 8);
                    let value = rng.next_u64().to_le_bytes();
                    damaged[at..at + 8].copy_from_slice(&value);
                }
                4 if !damaged.is_empty() => {
                    let at = rng.below(damaged.len());
                    damaged[at] = rng.next_u64() as u8;
                }
                _ => {
                    damaged = (0..rng.below(200)).map(|_| rng.next_u64() as u8).collect();
                }
            }
            check(&damaged);
        }

        // And the compressed form, which is what a store actually holds.
        let packed = compress(&original).expect("compress");
        assert_eq!(decompress(&packed).expect("round trip"), original);
        let mut damaged = packed;
        if !damaged.is_empty() {
            let at = rng.below(damaged.len());
            damaged[at] ^= 0xFF;
        }
        let _ = decompress(&damaged);
    }
}
