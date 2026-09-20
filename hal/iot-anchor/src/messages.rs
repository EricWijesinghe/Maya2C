//! What a device signs and a gateway submits.
//!
//! Each kind signs under its own ML-DSA context string, so a signature over one
//! kind can never be presented as another.

use crate::error::IotError;
use crate::rules::BatchView;
use crate::types::{
    BOUNDS_BYTES, Bounds, DeviceId, MAX_BATCH_READINGS, PUBLIC_KEY_BYTES, SIGNATURE_BYTES,
    SensorClass, TamperCause, device_id,
};
use crate::wire::{Reader, Writer, tagged_hash};

/// ML-DSA context for enrollment proofs.
pub const ENROLL_CONTEXT: &[u8] = b"maya2c-iot-enroll-v1";
/// ML-DSA context for telemetry batches.
pub const BATCH_CONTEXT: &[u8] = b"maya2c-iot-batch-v1";
/// ML-DSA context for tamper events.
pub const TAMPER_CONTEXT: &[u8] = b"maya2c-iot-tamper-v1";

/// Signed body of an enrollment: owner, device, class, bounds.
pub const ENROLLMENT_BODY_BYTES: usize = 32 + 32 + 1 + BOUNDS_BYTES;
/// Encoded enrollment: key, class, bounds, proof. The owner is the sender.
pub const ENROLLMENT_BYTES: usize = PUBLIC_KEY_BYTES + 1 + BOUNDS_BYTES + SIGNATURE_BYTES;
/// Signed body of a batch: device, predecessor, counters, root, extremes.
pub const BATCH_BODY_BYTES: usize = 32 + 32 + 8 + 8 + 32 + 8 + 8;
/// Encoded batch.
pub const BATCH_BYTES: usize = BATCH_BODY_BYTES + SIGNATURE_BYTES;
/// Signed body of a tamper event.
pub const TAMPER_BODY_BYTES: usize = 32 + 8 + 1;
/// Encoded tamper event.
pub const TAMPER_BYTES: usize = TAMPER_BODY_BYTES + SIGNATURE_BYTES;
/// Encoded equivocation evidence.
pub const EQUIVOCATION_BYTES: usize = 2 * BATCH_BYTES;

/// Verifies an ML-DSA-65 signature. `false` for a malformed key.
#[must_use]
pub fn verify_signature(
    public_key: &[u8; PUBLIC_KEY_BYTES],
    message: &[u8],
    context: &[u8],
    signature: &[u8; SIGNATURE_BYTES],
) -> bool {
    use fips204::traits::{SerDes, Verifier};
    fips204::ml_dsa_65::PublicKey::try_from_bytes(*public_key)
        .is_ok_and(|key| key.verify(message, signature, context))
}

/// An owner's claim to a device, with the device's proof of possession.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enrollment {
    /// The device's ML-DSA-65 public key.
    pub public_key: [u8; PUBLIC_KEY_BYTES],
    /// What it measures.
    pub class: SensorClass,
    /// What it declares plausible.
    pub bounds: Bounds,
    /// The device's signature over [`Enrollment::signed_bytes`].
    pub proof: [u8; SIGNATURE_BYTES],
}

impl Enrollment {
    /// The device this enrolls.
    #[must_use]
    pub fn device(&self) -> DeviceId {
        device_id(&self.public_key)
    }

    /// The bytes the device signs: binding it to `owner`, its class and bounds,
    /// so none can be changed without the device.
    #[must_use]
    pub fn signed_bytes(&self, owner: &[u8; 32]) -> [u8; ENROLLMENT_BODY_BYTES] {
        let mut body = [0u8; ENROLLMENT_BODY_BYTES];
        let mut writer = Writer::new(&mut body);
        writer.put(owner);
        writer.put(&self.device());
        writer.u8(self.class.tag());
        self.bounds.write(&mut writer);
        writer.finish();
        body
    }

    /// Checks the proof of possession for `owner`.
    ///
    /// # Errors
    ///
    /// [`IotError::BadSignature`].
    pub fn verify(&self, owner: &[u8; 32]) -> Result<(), IotError> {
        let body = self.signed_bytes(owner);
        if verify_signature(&self.public_key, &body, ENROLL_CONTEXT, &self.proof) {
            Ok(())
        } else {
            Err(IotError::BadSignature)
        }
    }

    /// The wire form.
    #[must_use]
    pub fn encode(&self) -> [u8; ENROLLMENT_BYTES] {
        let mut out = [0u8; ENROLLMENT_BYTES];
        let mut writer = Writer::new(&mut out);
        writer.put(&self.public_key);
        writer.u8(self.class.tag());
        self.bounds.write(&mut writer);
        writer.put(&self.proof);
        writer.finish();
        out
    }

    /// Reads exactly one enrollment.
    ///
    /// # Errors
    ///
    /// Truncation, trailing bytes, an unknown class or invalid bounds.
    pub fn decode(bytes: &[u8]) -> Result<Self, IotError> {
        let mut reader = Reader::new(bytes);
        let value = Self {
            public_key: reader.array()?,
            class: SensorClass::from_tag(reader.u8()?)?,
            bounds: Bounds::read(&mut reader)?,
            proof: reader.array()?,
        };
        reader.finish()?;
        Ok(value)
    }
}

/// A signed commitment to a contiguous run of readings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TelemetryBatch {
    /// The signing device.
    pub device: DeviceId,
    /// Hash of the device's previous batch ([`TelemetryBatch::hash`]), or
    /// [`crate::rules::GENESIS_PREVIOUS`] for its first. Firmware persists it
    /// across reboots: a device that restarts its chain forks it, and a fork is
    /// what a copied key looks like.
    pub previous: [u8; 32],
    /// Counter of the first reading.
    pub first_counter: u64,
    /// Counter of the last reading.
    pub last_counter: u64,
    /// Merkle root over the readings ([`crate::merkle`]).
    pub readings_root: [u8; 32],
    /// Lowest reading.
    pub min: i64,
    /// Highest reading.
    pub max: i64,
    /// The device's signature over [`TelemetryBatch::body`].
    pub signature: [u8; SIGNATURE_BYTES],
}

impl TelemetryBatch {
    /// Checks the internal consistency every batch must have.
    ///
    /// # Errors
    ///
    /// [`IotError::InvalidRange`].
    pub const fn validate(
        first_counter: u64,
        last_counter: u64,
        min: i64,
        max: i64,
    ) -> Result<(), IotError> {
        if first_counter > last_counter
            || last_counter - first_counter >= MAX_BATCH_READINGS
            || min > max
        {
            return Err(IotError::InvalidRange);
        }
        Ok(())
    }

    /// The signed bytes.
    #[must_use]
    pub fn body(&self) -> [u8; BATCH_BODY_BYTES] {
        let mut body = [0u8; BATCH_BODY_BYTES];
        let mut writer = Writer::new(&mut body);
        writer.put(&self.device);
        writer.put(&self.previous);
        writer.u64(self.first_counter);
        writer.u64(self.last_counter);
        writer.put(&self.readings_root);
        writer.i64(self.min);
        writer.i64(self.max);
        writer.finish();
        body
    }

    /// The hash the device's next batch names as its predecessor.
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        tagged_hash(b"maya2c iot batch hash v1", &[&self.body()])
    }

    /// What the rules judge: ranges, extremes, the body hash and its predecessor.
    #[must_use]
    pub fn view(&self) -> BatchView {
        BatchView {
            first: self.first_counter,
            last: self.last_counter,
            hash: self.hash(),
            previous: self.previous,
            min: self.min,
            max: self.max,
        }
    }

    /// Checks the signature against `public_key`, which must be this batch's
    /// device.
    ///
    /// # Errors
    ///
    /// [`IotError::BadSignature`] for a wrong key or signature.
    pub fn verify(&self, public_key: &[u8; PUBLIC_KEY_BYTES]) -> Result<(), IotError> {
        if device_id(public_key) == self.device
            && verify_signature(public_key, &self.body(), BATCH_CONTEXT, &self.signature)
        {
            Ok(())
        } else {
            Err(IotError::BadSignature)
        }
    }

    /// The wire form.
    #[must_use]
    pub fn encode(&self) -> [u8; BATCH_BYTES] {
        let mut out = [0u8; BATCH_BYTES];
        let mut writer = Writer::new(&mut out);
        writer.put(&self.body());
        writer.put(&self.signature);
        writer.finish();
        out
    }

    /// Reads exactly one batch.
    ///
    /// # Errors
    ///
    /// Truncation, trailing bytes, or an inconsistent range.
    pub fn decode(bytes: &[u8]) -> Result<Self, IotError> {
        let mut reader = Reader::new(bytes);
        let value = Self::read(&mut reader)?;
        reader.finish()?;
        Ok(value)
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, IotError> {
        let value = Self {
            device: reader.array()?,
            previous: reader.array()?,
            first_counter: reader.u64()?,
            last_counter: reader.u64()?,
            readings_root: reader.array()?,
            min: reader.i64()?,
            max: reader.i64()?,
            signature: reader.array()?,
        };
        Self::validate(
            value.first_counter,
            value.last_counter,
            value.min,
            value.max,
        )?;
        Ok(value)
    }
}

/// A device's signed report that it detected tampering, sent before it
/// zeroizes its seed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TamperEvent {
    /// The signing device.
    pub device: DeviceId,
    /// The device's counter when it fired.
    pub counter: u64,
    /// What fired.
    pub cause: TamperCause,
    /// The device's signature over [`TamperEvent::body`].
    pub signature: [u8; SIGNATURE_BYTES],
}

impl TamperEvent {
    /// The signed bytes.
    #[must_use]
    pub fn body(&self) -> [u8; TAMPER_BODY_BYTES] {
        let mut body = [0u8; TAMPER_BODY_BYTES];
        let mut writer = Writer::new(&mut body);
        writer.put(&self.device);
        writer.u64(self.counter);
        writer.u8(self.cause.tag());
        writer.finish();
        body
    }

    /// Checks the signature against `public_key`.
    ///
    /// # Errors
    ///
    /// [`IotError::BadSignature`].
    pub fn verify(&self, public_key: &[u8; PUBLIC_KEY_BYTES]) -> Result<(), IotError> {
        if device_id(public_key) == self.device
            && verify_signature(public_key, &self.body(), TAMPER_CONTEXT, &self.signature)
        {
            Ok(())
        } else {
            Err(IotError::BadSignature)
        }
    }

    /// The wire form.
    #[must_use]
    pub fn encode(&self) -> [u8; TAMPER_BYTES] {
        let mut out = [0u8; TAMPER_BYTES];
        let mut writer = Writer::new(&mut out);
        writer.put(&self.body());
        writer.put(&self.signature);
        writer.finish();
        out
    }

    /// Reads exactly one tamper event.
    ///
    /// # Errors
    ///
    /// Truncation, trailing bytes, or an unknown cause.
    pub fn decode(bytes: &[u8]) -> Result<Self, IotError> {
        let mut reader = Reader::new(bytes);
        let value = Self {
            device: reader.array()?,
            counter: reader.u64()?,
            cause: TamperCause::from_tag(reader.u8()?)?,
            signature: reader.array()?,
        };
        reader.finish()?;
        Ok(value)
    }
}

/// Two batches one device signed over overlapping counter ranges with different
/// content. Only a copied key — or firmware that ignores its own counter —
/// produces this, so anyone may submit it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Equivocation {
    /// One batch.
    pub first: TelemetryBatch,
    /// A conflicting batch.
    pub second: TelemetryBatch,
}

impl Equivocation {
    /// Checks the evidence against the device's key.
    ///
    /// # Errors
    ///
    /// [`IotError::BadSignature`] if either batch fails to verify,
    /// [`IotError::NotEquivocation`] if they do not conflict.
    pub fn check(&self, public_key: &[u8; PUBLIC_KEY_BYTES]) -> Result<(), IotError> {
        self.first.verify(public_key)?;
        self.second.verify(public_key)?;
        let (a, b) = (self.first.view(), self.second.view());
        let overlap = a.first <= b.last && b.first <= a.last;
        // Two successors of one predecessor: a fork of the hash chain.
        let fork = a.previous == b.previous;
        // One extends the other but speaks again for counters already used.
        let reuse = (b.previous == a.hash && b.first <= a.last)
            || (a.previous == b.hash && a.first <= b.last);
        if a.hash != b.hash && (overlap || fork || reuse) {
            Ok(())
        } else {
            Err(IotError::NotEquivocation)
        }
    }

    /// The wire form.
    #[must_use]
    pub fn encode(&self) -> [u8; EQUIVOCATION_BYTES] {
        let mut out = [0u8; EQUIVOCATION_BYTES];
        let mut writer = Writer::new(&mut out);
        writer.put(&self.first.encode());
        writer.put(&self.second.encode());
        writer.finish();
        out
    }

    /// Reads exactly one pair.
    ///
    /// # Errors
    ///
    /// As [`TelemetryBatch::decode`].
    pub fn decode(bytes: &[u8]) -> Result<Self, IotError> {
        let mut reader = Reader::new(bytes);
        let value = Self {
            first: TelemetryBatch::read(&mut reader)?,
            second: TelemetryBatch::read(&mut reader)?,
        };
        reader.finish()?;
        Ok(value)
    }
}
