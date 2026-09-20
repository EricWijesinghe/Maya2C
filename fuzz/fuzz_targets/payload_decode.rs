//! Fuzzes [`TxKind::decode`], the typed-payload section of a wire-version-6
//! transaction.
//!
//! This is the richest decoder in the tree and the one with the most variable
//! shapes: a `SettleBatch` is a length-prefixed vector of fixed-width closures,
//! a `DeployContract` is a length-prefixed blob capped by `MAX_CONTRACT_CODE`,
//! a `CallContract` carries a second independent blob, and a `Shielded`
//! joinsplit is a fixed 400-odd bytes of field elements and proof. Each is a
//! separate chance to get a bound wrong.
//!
//! Fuzzing it directly rather than only through `Transaction::from_bytes` gets
//! past the ~13 KB of key and signature material a version-6 frame would
//! otherwise need before the payload tag is even reached — bytes a fuzzer would
//! spend its whole budget failing to guess.
//!
//! `TxKind::Transfer` has no tag and is never produced by `decode`, so the
//! round-trip below is exact: every value `decode` can return re-encodes to a
//! non-empty section.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::TxKind;
use custom_l1_node::core::codec::ByteReader;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut reader = ByteReader::new(data);
    let Ok(kind) = TxKind::decode(&mut reader) else {
        return;
    };

    // A payload section is the tail of a transaction frame, so the decoder is
    // not itself required to consume the input exactly — `Transaction::from_bytes`
    // enforces that. Only assert canonicality when the section was the whole
    // input; otherwise the trailing bytes belong to no one and comparing them
    // would be comparing against the fuzzer's slack.
    if reader.finish().is_err() {
        return;
    }

    let mut reencoded = Vec::new();
    kind.encode_into(&mut reencoded);
    assert_eq!(
        reencoded.as_slice(),
        data,
        "non-canonical payload encoding accepted for kind `{}`",
        kind.label(),
    );

    let mut rereader = ByteReader::new(&reencoded);
    let redecoded = TxKind::decode(&mut rereader).expect("a payload's own encoding must decode back");
    rereader
        .finish()
        .expect("a payload's own encoding must be consumed exactly");
    assert_eq!(
        redecoded, kind,
        "decode is not the inverse of encode for an accepted payload"
    );
});
