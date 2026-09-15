//! WASM module mutation, structure-aware.
//!
//! Two engines, chosen by the seed:
//!
//! - **`wasm-smith`** turns the seed bytes into a *valid* module, so the deploy
//!   validator and the fuel meter see well-formed WASM within a shape close to
//!   what a contract is, not bytes that fail at the magic number.
//! - **`wasm-mutate`** perturbs a valid module — snip a body, rename an export,
//!   reorder sections — to probe the validator's edges from a working start.
//!
//! Arbitrary-byte robustness of the deploy decoder is already the
//! `payload_decode` libFuzzer target's job; this aims at the layer that only a
//! module past the magic number reaches.

use arbitrary::Unstructured;
use rand_core::RngCore;
use wasm_smith::{Config, Module};

use super::bytes;
use crate::Rng;

/// Largest module the mutator emits. Below the VM's `MAX_CONTRACT_CODE` (512 KiB)
/// so a module rejected purely for size is a separate, cheaper case.
const MAX_MODULE_BYTES: usize = 128 * 1024;

/// Mutates one WASM input.
#[must_use]
pub fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    match rng.next_u32() % 3 {
        // Generate a fresh valid module from seed-derived entropy.
        0 => smith(seed, rng),
        // Perturb the seed if it is already valid WASM; else generate.
        1 => match perturb(seed, rng) {
            Some(module) if module.len() <= MAX_MODULE_BYTES => module,
            _ => smith(seed, rng),
        },
        // Byte-level, so a truncated or corrupt module still exercises the
        // decoder's own bounds.
        _ => bytes::mutate(seed, rng),
    }
}

fn smith(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    // Mix the seed with RNG bytes so generation is varied yet reproducible.
    let mut entropy = seed.to_vec();
    let mut tail = [0u8; 256];
    rng.fill_bytes(&mut tail);
    entropy.extend_from_slice(&tail);

    let mut config = Config::default();
    config.max_memories = 1;
    config.max_imports = 16;
    config.max_funcs = 32;
    config.bulk_memory_enabled = false;
    config.reference_types_enabled = false;
    config.simd_enabled = false;

    let mut u = Unstructured::new(&entropy);
    match Module::new(config, &mut u) {
        Ok(module) => {
            let bytes = module.to_bytes();
            if bytes.len() <= MAX_MODULE_BYTES {
                bytes
            } else {
                bytes[..MAX_MODULE_BYTES].to_vec()
            }
        }
        // Not enough entropy to build a module: fall back to byte mutation.
        Err(_) => bytes::mutate(seed, rng),
    }
}

fn perturb(seed: &[u8], rng: &mut Rng) -> Option<Vec<u8>> {
    let mut mutator = wasm_mutate::WasmMutate::default();
    mutator.seed(rng.next_u64());
    mutator.fuel(1_000);
    // `run` fails if the seed is not valid WASM; the caller then generates one.
    let mut iterator = mutator.run(seed).ok()?;
    iterator.next()?.ok()
}

/// Seed modules: a handful of tiny valid modules from fixed entropy.
#[must_use]
pub fn seeds() -> Vec<Vec<u8>> {
    // The bare valid module: the four-byte magic and the version. Always valid,
    // and the smallest thing the decoder must accept.
    let empty_module = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    let mut seeds = vec![empty_module];
    for salt in 0u8..4 {
        let entropy = [salt; 1024];
        let mut u = Unstructured::new(&entropy);
        if let Ok(module) = Module::new(Config::default(), &mut u) {
            seeds.push(module.to_bytes());
        }
    }
    seeds
}

#[cfg(test)]
mod tests {
    use rand_core::SeedableRng;

    use super::*;

    #[test]
    fn seeds_are_valid_wasm() {
        for seed in seeds() {
            assert_eq!(&seed[..4], b"\0asm", "not a wasm module");
        }
    }

    #[test]
    fn mutation_is_deterministic_and_bounded() {
        let seed = &seeds()[0];
        let a: Vec<_> = (0..40)
            .scan(Rng::seed_from_u64(7), |r, _| Some(mutate(seed, r)))
            .collect();
        let b: Vec<_> = (0..40)
            .scan(Rng::seed_from_u64(7), |r, _| Some(mutate(seed, r)))
            .collect();
        assert_eq!(a, b);
        assert!(a.iter().all(|m| m.len() <= MAX_MODULE_BYTES));
    }

    #[test]
    fn generation_never_panics_on_hostile_entropy() {
        let mut r = Rng::seed_from_u64(9);
        for _ in 0..500 {
            let _ = mutate(&[], &mut r);
            let _ = mutate(&[0xff; 64], &mut r);
        }
    }
}
