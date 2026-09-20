//! The device side: a seed, the key it expands into, and signing.

use fips204::ml_dsa_65;
use fips204::traits::{KeyGen, SerDes, Signer};
use rand_core::CryptoRngCore;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::IotError;
use crate::merkle::BatchSummary;
use crate::messages::{
    BATCH_CONTEXT, ENROLL_CONTEXT, Enrollment, TAMPER_CONTEXT, TamperEvent, TelemetryBatch,
};
use crate::types::{
    Bounds, DeviceId, PUBLIC_KEY_BYTES, SIGNATURE_BYTES, SensorClass, TamperCause, device_id,
};

/// The 32 bytes a device key is expanded from. Zeroized on drop; never
/// serialized by this crate.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct Seed([u8; 32]);

impl Seed {
    /// Wraps seed bytes, taking ownership so no copy outlives the wrapper.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The seed bytes, for a key source that must seal them.
    #[must_use]
    pub const fn expose(&self) -> &[u8; 32] {
        &self.0
    }
}

impl core::fmt::Debug for Seed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Seed(..)")
    }
}

/// A device's signing key.
pub struct DeviceKey {
    private: ml_dsa_65::PrivateKey,
    public: [u8; PUBLIC_KEY_BYTES],
    id: DeviceId,
}

impl DeviceKey {
    /// Expands `seed` into the device key. Deterministic: the same seed is the
    /// same device.
    #[must_use]
    pub fn from_seed(seed: &Seed) -> Self {
        let (public, private) = ml_dsa_65::KG::keygen_from_seed(&seed.0);
        let public = public.into_bytes();
        Self {
            private,
            id: device_id(&public),
            public,
        }
    }

    /// The device identifier.
    #[must_use]
    pub const fn id(&self) -> DeviceId {
        self.id
    }

    /// The public key.
    #[must_use]
    pub const fn public_key(&self) -> &[u8; PUBLIC_KEY_BYTES] {
        &self.public
    }

    fn sign(
        &self,
        rng: &mut impl CryptoRngCore,
        message: &[u8],
        context: &[u8],
    ) -> Result<[u8; SIGNATURE_BYTES], IotError> {
        self.private
            .try_sign_with_rng(rng, message, context)
            .map_err(|_| IotError::SigningFailed)
    }

    /// Proves possession of this key to `owner`, binding class and bounds.
    ///
    /// # Errors
    ///
    /// [`IotError::SigningFailed`] if the RNG fails.
    pub fn enroll(
        &self,
        owner: &[u8; 32],
        class: SensorClass,
        bounds: Bounds,
        rng: &mut impl CryptoRngCore,
    ) -> Result<Enrollment, IotError> {
        let mut enrollment = Enrollment {
            public_key: self.public,
            class,
            bounds,
            proof: [0; SIGNATURE_BYTES],
        };
        enrollment.proof = self.sign(rng, &enrollment.signed_bytes(owner), ENROLL_CONTEXT)?;
        Ok(enrollment)
    }

    /// Signs a batch summary as the successor of `previous`: the
    /// [`TelemetryBatch::hash`] of this device's last batch, or
    /// [`crate::rules::GENESIS_PREVIOUS`] for its first. The caller persists it.
    ///
    /// The expanded private key zeroizes itself on drop: `fips204` derives
    /// `ZeroizeOnDrop` for it, so dropping a `DeviceKey` scrubs it.
    ///
    /// # Errors
    ///
    /// [`IotError::InvalidRange`] for an inconsistent summary,
    /// [`IotError::SigningFailed`] if the RNG fails.
    pub fn sign_batch(
        &self,
        summary: &BatchSummary,
        previous: &[u8; 32],
        rng: &mut impl CryptoRngCore,
    ) -> Result<TelemetryBatch, IotError> {
        TelemetryBatch::validate(summary.first, summary.last, summary.min, summary.max)?;
        let mut batch = TelemetryBatch {
            device: self.id,
            previous: *previous,
            first_counter: summary.first,
            last_counter: summary.last,
            readings_root: summary.root,
            min: summary.min,
            max: summary.max,
            signature: [0; SIGNATURE_BYTES],
        };
        batch.signature = self.sign(rng, &batch.body(), BATCH_CONTEXT)?;
        Ok(batch)
    }

    /// Signs a tamper report. Firmware sends it and then drops this key and
    /// zeroizes its seed.
    ///
    /// # Errors
    ///
    /// [`IotError::SigningFailed`] if the RNG fails.
    pub fn sign_tamper(
        &self,
        counter: u64,
        cause: TamperCause,
        rng: &mut impl CryptoRngCore,
    ) -> Result<TamperEvent, IotError> {
        let mut event = TamperEvent {
            device: self.id,
            counter,
            cause,
            signature: [0; SIGNATURE_BYTES],
        };
        event.signature = self.sign(rng, &event.body(), TAMPER_CONTEXT)?;
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use rand_chacha::ChaCha20Rng;
    use rand_core::SeedableRng;

    use super::*;
    use crate::merkle::ReadingsAccumulator;
    use crate::messages::Equivocation;
    use crate::rules::GENESIS_PREVIOUS;

    fn key(byte: u8) -> DeviceKey {
        DeviceKey::from_seed(&Seed::new([byte; 32]))
    }

    fn summary(first: u64, values: &[i64]) -> BatchSummary {
        let mut acc = ReadingsAccumulator::new();
        for (offset, value) in (0u64..).zip(values) {
            acc.push(first + offset, *value).expect("contiguous");
        }
        acc.summary().expect("non-empty")
    }

    #[test]
    fn enrollment_binds_owner_class_and_bounds() {
        let mut rng = ChaCha20Rng::seed_from_u64(1);
        let device = key(7);
        let bounds = Bounds::new(-40_000, 8_000, 5_000).expect("bounds");
        let enrollment = device
            .enroll(
                &[1; 32],
                SensorClass::ColdChainTemperature,
                bounds,
                &mut rng,
            )
            .expect("sign");
        assert_eq!(enrollment.verify(&[1; 32]), Ok(()));
        assert_eq!(enrollment.verify(&[2; 32]), Err(IotError::BadSignature));
        let mut widened = enrollment.clone();
        widened.bounds.max = 90_000;
        assert_eq!(widened.verify(&[1; 32]), Err(IotError::BadSignature));
        assert_eq!(Enrollment::decode(&enrollment.encode()), Ok(enrollment));
    }

    #[test]
    fn a_batch_verifies_only_under_its_own_key_predecessor_and_context() {
        let mut rng = ChaCha20Rng::seed_from_u64(2);
        let (device, other) = (key(1), key(2));
        let batch = device
            .sign_batch(&summary(10, &[5, 6, 7]), &GENESIS_PREVIOUS, &mut rng)
            .expect("sign");
        assert_eq!(batch.verify(device.public_key()), Ok(()));
        assert_eq!(
            batch.verify(other.public_key()),
            Err(IotError::BadSignature)
        );
        let mut forged = batch.clone();
        forged.max = 700;
        assert_eq!(
            forged.verify(device.public_key()),
            Err(IotError::BadSignature)
        );
        let mut relinked = batch.clone();
        relinked.previous = [1; 32];
        assert_eq!(
            relinked.verify(device.public_key()),
            Err(IotError::BadSignature)
        );
        assert_eq!(TelemetryBatch::decode(&batch.encode()), Ok(batch));

        // A tamper signature cannot pass as a batch signature: separate contexts.
        let event = device
            .sign_tamper(12, TamperCause::Enclosure, &mut rng)
            .expect("sign");
        assert_eq!(event.verify(device.public_key()), Ok(()));
        assert_eq!(TamperEvent::decode(&event.encode()), Ok(event));
    }

    #[test]
    fn forks_overlaps_and_reused_counters_are_equivocation_and_a_chain_is_not() {
        let mut rng = ChaCha20Rng::seed_from_u64(3);
        let device = key(9);
        let pk = device.public_key();
        let sign = |first, values: &[i64], previous: &[u8; 32], rng: &mut ChaCha20Rng| {
            device
                .sign_batch(&summary(first, values), previous, rng)
                .expect("sign")
        };
        let honest = sign(0, &[1, 2, 3], &GENESIS_PREVIOUS, &mut rng);
        let reused = sign(2, &[99, 100], &honest.hash(), &mut rng);
        let fork = sign(500, &[5], &GENESIS_PREVIOUS, &mut rng);
        let next = sign(3, &[4], &honest.hash(), &mut rng);

        for conflicting in [reused, fork] {
            let evidence = Equivocation {
                first: honest.clone(),
                second: conflicting,
            };
            assert_eq!(evidence.check(pk), Ok(()));
            assert_eq!(Equivocation::decode(&evidence.encode()), Ok(evidence));
        }
        let chained = Equivocation {
            first: honest.clone(),
            second: next,
        };
        assert_eq!(chained.check(pk), Err(IotError::NotEquivocation));
        let same = Equivocation {
            first: honest.clone(),
            second: honest,
        };
        assert_eq!(same.check(pk), Err(IotError::NotEquivocation));
    }
}
