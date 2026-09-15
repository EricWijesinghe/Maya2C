//! Transaction mutation, structure-aware.
//!
//! A seed that decodes is mutated as a `Transaction` — amounts to `u64`
//! boundaries, nonce, output and input counts, and the signature toggled
//! between present-and-stale and absent — then re-encoded. This is what reaches
//! `Transaction::from_bytes`'s success path, the mempool, and the
//! signature-check path, rather than dying at offset 0. A seed that does not
//! decode falls back to [`super::bytes`].

use custom_l1_node::core::{Transaction, TxInput, TxOutput};
use custom_l1_node::crypto::hybrid::generate_signing_key;
use rand_core::RngCore;

use super::bytes::{self, boundary_u64};
use crate::Rng;

/// Mutates one transaction input.
#[must_use]
pub fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    match Transaction::from_bytes(seed) {
        Ok(tx) => mutate_typed(tx, rng),
        // The seed is not a transaction yet: hammer the decoder instead.
        Err(_) => bytes::mutate(seed, rng),
    }
}

fn mutate_typed(mut tx: Transaction, rng: &mut Rng) -> Vec<u8> {
    let signed_before = tx.signature.is_some();
    match rng.next_u32() % 6 {
        0 => {
            if let Some(output) = pick_mut(&mut tx.outputs, rng) {
                output.amount = boundary_u64(rng);
            } else {
                tx.outputs.push(TxOutput {
                    amount: boundary_u64(rng),
                    recipient: [rng_byte(rng); 32],
                });
            }
        }
        1 => tx.nonce = boundary_u64(rng),
        2 => {
            // Many outputs: the block-size and per-tx bounds, and the loop that
            // sums them.
            let count = (rng.next_u32() % 4096) as usize;
            tx.outputs = (0..count)
                .map(|i| TxOutput {
                    amount: boundary_u64(rng),
                    recipient: [i as u8; 32],
                })
                .collect();
        }
        3 => {
            let count = (rng.next_u32() % 4096) as usize;
            tx.inputs = (0..count)
                .map(|i| TxInput {
                    prev_tx: [i as u8; 32],
                    index: rng.next_u32(),
                })
                .collect();
        }
        4 => tx.signature = None,
        _ => {
            // Re-sign under a fresh, unrelated key: the signature verifies
            // against the wrong public key, exactly the malleability case.
            if let Ok(key) = generate_signing_key() {
                let _ = tx.sign(&key);
            }
        }
    }

    // Half the mutations that left a signature in place invalidate it by
    // changing signed bytes without re-signing — a stale signature is a
    // distinct path from an absent one.
    if signed_before && tx.signature.is_some() && rng.next_u32() & 1 == 0 {
        tx.nonce = tx.nonce.wrapping_add(1);
    }
    tx.to_bytes()
}

fn pick_mut<'a, T>(items: &'a mut [T], rng: &mut Rng) -> Option<&'a mut T> {
    if items.is_empty() {
        return None;
    }
    let index = (rng.next_u32() as usize) % items.len();
    items.get_mut(index)
}

fn rng_byte(rng: &mut Rng) -> u8 {
    (rng.next_u32() & 0xff) as u8
}

/// Seed transactions: a signed transfer and an unsigned one, encoded.
#[must_use]
pub fn seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    let unsigned = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: 1_000,
            recipient: [7u8; 32],
        }],
        0,
    );
    seeds.push(unsigned.to_bytes());
    if let Ok(key) = generate_signing_key() {
        let mut signed = Transaction::new(
            vec![],
            vec![TxOutput {
                amount: 500,
                recipient: [9u8; 32],
            }],
            1,
        );
        if signed.sign(&key).is_ok() {
            seeds.push(signed.to_bytes());
        }
    }
    seeds
}

#[cfg(test)]
mod tests {
    use rand_core::SeedableRng;

    use super::*;

    #[test]
    fn seeds_decode_and_mutations_are_deterministic() {
        for seed in seeds() {
            assert!(Transaction::from_bytes(&seed).is_ok());
        }
        let seed = &seeds()[0];
        let a: Vec<_> = (0..64)
            .scan(Rng::seed_from_u64(3), |r, _| Some(mutate(seed, r)))
            .collect();
        let b: Vec<_> = (0..64)
            .scan(Rng::seed_from_u64(3), |r, _| Some(mutate(seed, r)))
            .collect();
        assert_eq!(a, b);
    }

    #[test]
    fn mutating_a_seed_never_panics() {
        let seed = &seeds()[0];
        let mut r = Rng::seed_from_u64(11);
        for _ in 0..5_000 {
            let out = mutate(seed, &mut r);
            // Whatever comes out, decoding it must not panic either.
            let _ = Transaction::from_bytes(&out);
        }
    }
}
