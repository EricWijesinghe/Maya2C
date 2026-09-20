//! A fuzzy extractor: a stable seed from a noisy PUF response.
//!
//! Code-offset construction with a repetition code. At manufacture a random
//! 256-bit secret is encoded by repeating each bit [`REPETITION`] times; the
//! public helper data is that codeword XOR the PUF response. In the field the
//! device XORs a fresh response with the helper and majority-decodes each group,
//! recovering the secret while fewer than half a group's bits have flipped. A
//! check value in the helper turns an uncorrectable response into an error
//! rather than a different key.
//!
//! What this is not: a PUF. Software can correct noise; unclonability is a
//! property of silicon, and a simulated PUF in tests proves the decoder, not the
//! hardware. A repetition code is chosen for auditability; it leaks through the
//! helper data if response bits are biased or correlated, so real SRAM needs
//! debiasing or a stronger code (BCH, Reed–Muller) first.

use crate::device::Seed;
use crate::error::IotError;
use crate::wire::{Reader, Writer, tagged_hash};

/// Copies of each secret bit.
pub const REPETITION: usize = 15;
/// Secret bits.
pub const SECRET_BITS: usize = 256;
/// PUF response length: one bit per codeword bit.
pub const RESPONSE_BYTES: usize = SECRET_BITS * REPETITION / 8;
/// Most flipped bits per group that still decode.
pub const CORRECTABLE_PER_GROUP: usize = REPETITION / 2;
/// Encoded helper data.
pub const HELPER_BYTES: usize = RESPONSE_BYTES + 32;

/// Public data stored beside the PUF: reveals nothing about the secret if the
/// response bits are uniform and independent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperData {
    offset: [u8; RESPONSE_BYTES],
    check: [u8; 32],
}

const fn bit(bytes: &[u8], index: usize) -> u8 {
    (bytes[index / 8] >> (index % 8)) & 1
}

fn check_value(secret: &[u8; 32]) -> [u8; 32] {
    tagged_hash(b"maya2c iot puf check v1", &[secret])
}

impl HelperData {
    /// Enrolls: binds `secret` (from a TRNG) to `response`.
    #[must_use]
    pub fn enroll(response: &[u8; RESPONSE_BYTES], secret: &[u8; 32]) -> Self {
        let mut offset = [0u8; RESPONSE_BYTES];
        for index in 0..SECRET_BITS * REPETITION {
            let code_bit = bit(secret, index / REPETITION);
            offset[index / 8] |= (bit(response, index) ^ code_bit) << (index % 8);
        }
        Self {
            offset,
            check: check_value(secret),
        }
    }

    /// Recovers the device seed from a fresh response.
    ///
    /// # Errors
    ///
    /// [`IotError::PufUncorrectable`] if any group flipped more than
    /// [`CORRECTABLE_PER_GROUP`] bits and the secret no longer matches.
    pub fn reproduce(&self, response: &[u8; RESPONSE_BYTES]) -> Result<Seed, IotError> {
        let mut secret = [0u8; 32];
        for secret_bit in 0..SECRET_BITS {
            let ones: usize = (0..REPETITION)
                .map(|copy| {
                    let index = secret_bit * REPETITION + copy;
                    usize::from(bit(response, index) ^ bit(&self.offset, index))
                })
                .sum();
            secret[secret_bit / 8] |= u8::from(ones > CORRECTABLE_PER_GROUP) << (secret_bit % 8);
        }
        let matches = check_value(&secret)
            .iter()
            .zip(&self.check)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0;
        let mut seed = tagged_hash(b"maya2c iot puf seed v1", &[&secret]);
        secret.fill(0);
        if matches {
            Ok(Seed::new(seed))
        } else {
            // Derived from a wrong secret, but still key material: scrub it.
            seed.fill(0);
            Err(IotError::PufUncorrectable)
        }
    }

    /// The stored form.
    #[must_use]
    pub fn encode(&self) -> [u8; HELPER_BYTES] {
        let mut out = [0u8; HELPER_BYTES];
        let mut writer = Writer::new(&mut out);
        writer.put(&self.offset);
        writer.put(&self.check);
        writer.finish();
        out
    }

    /// Reads helper data.
    ///
    /// # Errors
    ///
    /// Truncation or trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, IotError> {
        let mut reader = Reader::new(bytes);
        let value = Self {
            offset: reader.array()?,
            check: reader.array()?,
        };
        reader.finish()?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use rand_chacha::ChaCha20Rng;
    use rand_core::{RngCore, SeedableRng};

    use super::*;

    /// A simulated SRAM power-up pattern with `flips` bits inverted in every
    /// group — exactly controlled, so the test never depends on luck.
    fn noisy(clean: &[u8; RESPONSE_BYTES], flips: usize) -> [u8; RESPONSE_BYTES] {
        let mut out = *clean;
        for group in 0..SECRET_BITS {
            for copy in 0..flips {
                let index = group * REPETITION + copy;
                out[index / 8] ^= 1 << (index % 8);
            }
        }
        out
    }

    fn setup() -> ([u8; RESPONSE_BYTES], HelperData, Seed) {
        let mut rng = ChaCha20Rng::seed_from_u64(42);
        let mut response = [0u8; RESPONSE_BYTES];
        rng.fill_bytes(&mut response);
        let mut secret = [0u8; 32];
        rng.fill_bytes(&mut secret);
        let helper = HelperData::enroll(&response, &secret);
        let seed = helper.reproduce(&response).expect("clean response");
        (response, helper, seed)
    }

    #[test]
    fn noise_within_the_budget_reproduces_the_same_seed() {
        let (response, helper, seed) = setup();
        let worst = noisy(&response, CORRECTABLE_PER_GROUP);
        assert_eq!(helper.reproduce(&worst), Ok(seed));
        assert_eq!(HelperData::decode(&helper.encode()), Ok(helper));
    }

    #[test]
    fn noise_past_the_budget_is_an_error_not_another_key() {
        let (response, helper, _) = setup();
        let broken = noisy(&response, CORRECTABLE_PER_GROUP + 1);
        assert_eq!(helper.reproduce(&broken), Err(IotError::PufUncorrectable));
    }

    #[test]
    fn a_different_chip_does_not_reproduce_the_seed() {
        let (_, helper, _) = setup();
        let mut other = [0u8; RESPONSE_BYTES];
        ChaCha20Rng::seed_from_u64(7).fill_bytes(&mut other);
        assert_eq!(helper.reproduce(&other), Err(IotError::PufUncorrectable));
    }
}
