//! Fuzzes the HTLC decoders: openings, commitments, tagged locks and unlocks
//! (hash and lattice), lock records.
//!
//! A claim's opening and a lock's commitment are written by whoever submitted
//! the transaction. The properties: no input panics, and anything that decodes
//! re-encodes to exactly the bytes that produced it — a second encoding of an
//! opening would be a second transaction id for one claim.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use libfuzzer_sys::fuzz_target;
use maya_htlc_lattice::{
    COMMITMENT_BYTES, Commitment, ETA, Lock, LockRecord, OPENING_BYTES, Opening, Unlock,
};

fuzz_target!(|data: &[u8]| {
    // Tagged forms decode a prefix: what they consumed must re-encode to it.
    if let Ok((lock, used)) = Lock::decode(data) {
        let mut again = Vec::new();
        lock.encode_into(&mut again);
        assert_eq!(again, &data[..used]);
    }
    if let Ok((unlock, used)) = Unlock::decode(data) {
        let mut again = Vec::new();
        unlock.encode_into(&mut again);
        assert_eq!(again, &data[..used]);
    }
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
