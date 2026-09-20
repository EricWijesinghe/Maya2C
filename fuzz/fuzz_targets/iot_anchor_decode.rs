//! Fuzzes the IoT anchor decoders: enrollments, telemetry batches, tamper
//! events, equivocation pairs, device records and PUF helper data.
//!
//! All of them arrive from strangers: a gateway relays device bytes, and anyone
//! may submit equivocation evidence. The properties: no input panics, and
//! anything that decodes re-encodes to exactly the bytes that produced it.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use libfuzzer_sys::fuzz_target;
use maya_iot_anchor::puf::HelperData;
use maya_iot_anchor::{DeviceRecord, Enrollment, Equivocation, TamperEvent, TelemetryBatch};

fuzz_target!(|data: &[u8]| {
    if let Ok(value) = Enrollment::decode(data) {
        assert_eq!(value.encode().as_slice(), data);
    }
    if let Ok(value) = TelemetryBatch::decode(data) {
        assert_eq!(value.encode().as_slice(), data);
        assert!(value.first_counter <= value.last_counter && value.min <= value.max);
    }
    if let Ok(value) = TamperEvent::decode(data) {
        assert_eq!(value.encode().as_slice(), data);
    }
    if let Ok(value) = Equivocation::decode(data) {
        assert_eq!(value.encode().as_slice(), data);
    }
    if let Ok(value) = DeviceRecord::decode(data) {
        assert_eq!(value.encode().as_slice(), data);
    }
    if let Ok(value) = HelperData::decode(data) {
        assert_eq!(value.encode().as_slice(), data);
    }
});
