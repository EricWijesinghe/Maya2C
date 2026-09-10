//! Fuzzes [`Transaction::from_bytes`], the widest untrusted-input surface on
//! the node: every gossiped transaction and every transaction nested inside a
//! gossiped block enters here.
//!
//! Two properties are asserted, both of which the codec claims:
//!
//! 1. **No panic.** `ByteReader` range-checks every access, so a malformed
//!    frame must surface as `NodeError::Decode`, never as an index panic, an
//!    arithmetic overflow, or a capacity overflow.
//! 2. **Canonicality.** `ByteReader::finish` rejects trailing bytes and every
//!    field is fixed-width or length-prefixed, so a frame that decodes must
//!    re-encode to *itself*. Two distinct byte strings decoding to one
//!    transaction would mean one transaction has two txids.
//!
//! Deliberately absent: `Transaction::verify`. That runs ML-DSA-65 and
//! SLH-DSA-SHA2-128s over the frame, ~105 ms per call, which would cap the
//! fuzzer at roughly ten executions a second. Signature checking is not a
//! decode concern and is covered by `tests/hybrid_tests.rs`.

#![no_main]

use custom_l1_node::Transaction;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(transaction) = Transaction::from_bytes(data) else {
        return;
    };

    let reencoded = transaction.to_bytes();
    assert_eq!(
        reencoded.as_slice(),
        data,
        "non-canonical encoding accepted: {} input bytes decoded to a transaction \
         that re-encodes to {} bytes",
        data.len(),
        reencoded.len(),
    );

    let redecoded = Transaction::from_bytes(&reencoded)
        .expect("a transaction's own encoding must decode back");
    assert_eq!(
        redecoded, transaction,
        "decode is not the inverse of encode for an accepted frame"
    );
});
