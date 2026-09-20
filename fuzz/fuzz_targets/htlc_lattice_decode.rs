//! Fuzzes the lattice HTLC decoders: openings, commitments, lock records.
//!
//! A claim's opening and a lock's commitment are written by whoever submitted
//! the transaction. The properties: no input panics, and anything that decodes
//! re-encodes to exactly the bytes that produced it — a second encoding of an
//! opening would be a second transaction id for one claim.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use libfuzzer_sys::fuzz_target;
use maya_htlc_lattice::{
    COMMITMENT_BYTES, Commitment, ETA, LockRecord, OPENING_BYTES, Opening,
};

fuzz_target!(|data: &[u8]| {
    if let Ok(opening) = Opening::decode(data) {
        assert_eq!(opening.encode(), data);
        assert!(
            opening
                .s()
                .iter()
                .chain(opening.e())
                .all(|c| i32::from(*c).abs() <= ETA)
        );
    }
    if data.len() >= OPENING_BYTES {
        let _ = Opening::decode(&data[..OPENING_BYTES]);
    }
    if let Ok(commitment) = Commitment::decode(data) {
        assert_eq!(commitment.encode(), data);
    }
    if data.len() >= COMMITMENT_BYTES {
        if let Ok(commitment) = Commitment::decode(&data[..COMMITMENT_BYTES]) {
            assert_eq!(commitment.encode(), &data[..COMMITMENT_BYTES]);
        }
    }
    if let Ok(record) = LockRecord::decode(data) {
        assert_eq!(record.encode(), data);
    }
});
