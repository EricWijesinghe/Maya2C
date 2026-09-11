//! Fuzzes the archive reader: CAR framing, CIDs, the manifest, and zstd.
//!
//! A pruned node reads these bytes back from stores it does not trust, so the
//! property is total: any input either opens to blocks that each hash to their
//! CID, or is refused. It never panics, never allocates past
//! `MAX_ARCHIVE_BYTES`, and never returns a block whose bytes do not match its
//! CID.

#![no_main]

use libfuzzer_sys::fuzz_target;
use maya_archive::car::{RAW_CODEC, cid_of, read_car};
use maya_archive::{decompress, open_archive};

fuzz_target!(|data: &[u8]| {
    // Every section a successful read returns must verify.
    if let Ok(car) = read_car(data) {
        for (cid, bytes) in &car.sections {
            assert_eq!(
                maya_archive::car::verify(cid, bytes).ok(),
                Some(true),
                "read_car returned a section that does not hash to its CID"
            );
        }
        // Opening against the root the CAR itself names exercises the
        // manifest decoder on whatever the root section holds.
        if let Ok(blocks) = open_archive(data, &car.root) {
            for block in blocks {
                assert!(car.sections.contains_key(&cid_of(RAW_CODEC, &block.bytes)));
            }
        }
    }
    // Bounded decompression of hostile zstd.
    let _ = decompress(data);
});
