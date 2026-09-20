//! The stored record: one per device, overwritten by each batch.

use crate::error::IotError;
use crate::rules::DeviceStatus;
use crate::types::{BOUNDS_BYTES, Bounds, DeviceId, PUBLIC_KEY_BYTES, SensorClass, device_id};
use crate::wire::{Reader, Writer};

/// Encoded progress.
pub const PROGRESS_BYTES: usize = 1 + 8 + 8 + 32 + 32 + 8 + 8 + 8 + 8 + 8;

/// Encoded device record.
pub const DEVICE_RECORD_BYTES: usize =
    32 + PUBLIC_KEY_BYTES + 1 + BOUNDS_BYTES + 1 + 8 + PROGRESS_BYTES;

/// The latest recorded batch and running counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Whether any batch has been recorded; every other field is zero if not.
    pub has_batch: bool,
    /// Latest batch's first counter.
    pub first: u64,
    /// Latest batch's last counter.
    pub last: u64,
    /// Latest batch's body hash.
    pub hash: [u8; 32],
    /// The predecessor the latest batch named. A second batch naming it is a
    /// fork of the device's hash chain.
    pub previous: [u8; 32],
    /// Latest batch's lowest reading.
    pub min: i64,
    /// Latest batch's highest reading.
    pub max: i64,
    /// Height that recorded it.
    pub height: u64,
    /// Batches recorded.
    pub batches: u64,
    /// Of those, anomalous.
    pub anomalies: u64,
}

/// What the chain holds about one device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRecord {
    /// The address that enrolled it and may revoke it.
    pub owner: [u8; 32],
    /// Its ML-DSA-65 key.
    pub public_key: [u8; PUBLIC_KEY_BYTES],
    /// What it measures.
    pub class: SensorClass,
    /// What it declared plausible.
    pub bounds: Bounds,
    /// Lifecycle.
    pub status: DeviceStatus,
    /// Height that enrolled it.
    pub enrolled_height: u64,
    /// Latest batch.
    pub progress: Progress,
}

impl DeviceRecord {
    /// The device's identifier.
    #[must_use]
    pub fn device(&self) -> DeviceId {
        device_id(&self.public_key)
    }

    /// The stored form. Boxed-size on a host; never built on a device.
    #[must_use]
    pub fn encode(&self) -> [u8; DEVICE_RECORD_BYTES] {
        let mut out = [0u8; DEVICE_RECORD_BYTES];
        let mut writer = Writer::new(&mut out);
        writer.put(&self.owner);
        writer.put(&self.public_key);
        writer.u8(self.class.tag());
        self.bounds.write(&mut writer);
        writer.u8(self.status.tag());
        writer.u64(self.enrolled_height);
        let p = &self.progress;
        writer.u8(u8::from(p.has_batch));
        writer.u64(p.first);
        writer.u64(p.last);
        writer.put(&p.hash);
        writer.put(&p.previous);
        writer.i64(p.min);
        writer.i64(p.max);
        writer.u64(p.height);
        writer.u64(p.batches);
        writer.u64(p.anomalies);
        writer.finish();
        out
    }

    /// Reads a stored record, refusing any no transaction sequence produces.
    ///
    /// # Errors
    ///
    /// Truncation, trailing bytes, unknown tags, or
    /// [`IotError::NonCanonicalRecord`].
    pub fn decode(bytes: &[u8]) -> Result<Self, IotError> {
        let mut reader = Reader::new(bytes);
        let owner = reader.array()?;
        let public_key = reader.array()?;
        let class = SensorClass::from_tag(reader.u8()?)?;
        let bounds = Bounds::read(&mut reader)?;
        let status_tag = reader.u8()?;
        let status = DeviceStatus::from_tag(status_tag).ok_or(IotError::UnknownTag(status_tag))?;
        let enrolled_height = reader.u64()?;
        let has_batch = match reader.u8()? {
            0 => false,
            1 => true,
            _ => return Err(IotError::NonCanonicalRecord),
        };
        let progress = Progress {
            has_batch,
            first: reader.u64()?,
            last: reader.u64()?,
            hash: reader.array()?,
            previous: reader.array()?,
            min: reader.i64()?,
            max: reader.i64()?,
            height: reader.u64()?,
            batches: reader.u64()?,
            anomalies: reader.u64()?,
        };
        reader.finish()?;
        let consistent = if has_batch {
            progress.first <= progress.last
                && progress.min <= progress.max
                && progress.batches > 0
                && progress.anomalies <= progress.batches
                && progress.height >= enrolled_height
        } else {
            progress == Progress::default()
        };
        if !consistent {
            return Err(IotError::NonCanonicalRecord);
        }
        Ok(Self {
            owner,
            public_key,
            class,
            bounds,
            status,
            enrolled_height,
            progress,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{BatchView, record_batch};

    fn record() -> DeviceRecord {
        DeviceRecord {
            owner: [3; 32],
            public_key: [4; PUBLIC_KEY_BYTES],
            class: SensorClass::EnergyMeter,
            bounds: Bounds::new(0, 1_000_000, 10_000).expect("bounds"),
            status: DeviceStatus::Active,
            enrolled_height: 7,
            progress: Progress::default(),
        }
    }

    #[test]
    fn a_record_round_trips_and_an_impossible_one_does_not_decode() {
        let mut stored = record();
        assert_eq!(DeviceRecord::decode(&stored.encode()), Ok(stored.clone()));
        let view = BatchView {
            first: 1,
            last: 5,
            hash: [9; 32],
            previous: [0; 32],
            min: 10,
            max: 20,
        };
        stored.progress = record_batch(&stored.progress, &view, true, 8);
        assert_eq!(DeviceRecord::decode(&stored.encode()), Ok(stored.clone()));

        let mut bytes = stored.encode();
        let has_batch_at = 32 + PUBLIC_KEY_BYTES + 1 + BOUNDS_BYTES + 1 + 8;
        bytes[has_batch_at] = 0;
        assert_eq!(
            DeviceRecord::decode(&bytes),
            Err(IotError::NonCanonicalRecord)
        );
        assert_eq!(
            DeviceRecord::decode(&bytes[..bytes.len() - 1]),
            Err(IotError::Truncated)
        );
    }
}
