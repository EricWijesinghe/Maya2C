//! Fuzzes [`Block::from_bytes`].
//!
//! This is the allocation-sensitive target. A block frame carries a `u64`
//! transaction count and then a `u64` length prefix per transaction, so a
//! hostile peer's first lever is a count that forces a huge reservation before
//! a single byte of body is read. `ByteReader::read_collection_len` is supposed
//! to close that: it caps at `MAX_COLLECTION_LEN` and cross-checks the count
//! against the bytes actually remaining. The fuzzer's job is to find the input
//! where that cross-check is wrong.
//!
//! `-rss_limit_mb` is therefore load-bearing here rather than incidental: an
//! over-reservation shows up as a libFuzzer OOM, not as a panic. See
//! `fuzz/README.md` for the flags CI runs with.
//!
//! No proof-of-work, no signature verification — decode only.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::Block;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(block) = Block::from_bytes(data) else {
        return;
    };

    let reencoded = block.to_bytes();
    assert_eq!(
        reencoded.as_slice(),
        data,
        "non-canonical block encoding accepted: {} input bytes, {} transactions, \
         re-encodes to {} bytes",
        data.len(),
        block.transactions.len(),
        reencoded.len(),
    );

    let redecoded = Block::from_bytes(&reencoded).expect("a block's own encoding must decode back");
    assert_eq!(
        redecoded, block,
        "decode is not the inverse of encode for an accepted block"
    );

    // `tx_root` is a BLAKE3 fold over txids and is cheap, unlike `pow_hash`.
    // Folding it here checks that a decoded block can be committed to at all —
    // the path every validator takes immediately after decode.
    assert_eq!(
        block.tx_root(),
        redecoded.tx_root(),
        "equal blocks committed to different transaction roots"
    );
});
