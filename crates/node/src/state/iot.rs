//! `IoT` anchor records: one per device, under `v:`, one layer of the state root
//! (invariant 25).
//!
//! - `v:dev:<device id>` — [`DeviceRecord::encode`].
//!
//! Each batch overwrites its device's record, so state grows with the number
//! of devices, never with the number of readings.

use maya_iot_anchor::{DeviceId, DeviceRecord};

use crate::error::{NodeError, Result};
use crate::state::db::{Overlay, StateDB};

/// Prefix of every `IoT` anchor record.
pub const IOT_PREFIX: &[u8] = b"v:";

/// Prefix of device records.
pub(crate) const DEVICE_PREFIX: &[u8] = b"v:dev:";

const _: () = assert!(DEVICE_PREFIX[0] == IOT_PREFIX[0] && DEVICE_PREFIX[1] == IOT_PREFIX[1]);

/// Storage key of a device record.
#[must_use]
pub fn device_key(device: &DeviceId) -> Vec<u8> {
    [DEVICE_PREFIX, device.as_slice()].concat()
}

impl StateDB {
    /// A device record through the overlay.
    pub(crate) fn iot_device(
        &self,
        overlay: &Overlay,
        device: &DeviceId,
    ) -> Result<Option<DeviceRecord>> {
        self.record(overlay, &device_key(device))?
            .map(|bytes| decode(&bytes))
            .transpose()
    }

    /// A committed device record.
    ///
    /// # Errors
    ///
    /// A read failure, or a stored record that does not decode.
    pub fn stored_iot_device(&self, device: &DeviceId) -> Result<Option<DeviceRecord>> {
        self.raw_get(&device_key(device))?
            .map(|bytes| decode(&bytes))
            .transpose()
    }

    pub(crate) fn put_iot_device(overlay: &mut Overlay, record: &DeviceRecord) {
        StateDB::put_record(
            overlay,
            device_key(&record.device()),
            record.encode().to_vec(),
        );
    }
}

fn decode(bytes: &[u8]) -> Result<DeviceRecord> {
    DeviceRecord::decode(bytes).map_err(|e| NodeError::Decode(format!("iot device record: {e}")))
}
