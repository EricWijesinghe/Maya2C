//! Fuzzes [`BlockHeader::from_bytes`].
//!
//! The header is fixed-width — 177 bytes, no length prefixes — so the decoder
//! has a narrow surface, but it is reachable from `get_mining_candidate` over
//! JSON-RPC as well as from block gossip, which makes it reachable by callers
//! that never went through `Block::from_bytes`.
//!
//! The canonicality property is total here: exactly one 177-byte string maps to
//! each header, and every 177-byte string whose ADR-063 version byte is 0 and
//! whose reserved extension root (ADR-155) is all zeros is a valid header. So
//! `from_bytes`
//! must succeed for exactly those inputs and fail for every other
//! input — a stronger claim than round-tripping alone, and asserted as such.
//!
//! `pow_hash` and `meets_difficulty` are never called: ArgonBlake is a 32 MiB
//! Argon2id pass, seconds per invocation.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::BlockHeader;
use custom_l1_node::core::HEADER_LEN;
use custom_l1_node::core::block::{EXTENSION_ROOT_RANGE, VERSION_OFFSET};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    match BlockHeader::from_bytes(data) {
        Ok(header) => {
            assert_eq!(
                data.len(),
                HEADER_LEN,
                "header decoded from {} bytes, but the layout is fixed at {HEADER_LEN}",
                data.len(),
            );
            assert_eq!(
                header.serialize().as_slice(),
                data,
                "non-canonical header encoding accepted"
            );
        }
        Err(_) => {
            // Only version 0 exists, and the extension root is reserved zeros.
            assert!(
                data.len() != HEADER_LEN
                    || data[VERSION_OFFSET] != 0
                    || data[EXTENSION_ROOT_RANGE].iter().any(|b| *b != 0),
                "every {HEADER_LEN}-byte version-0 string with a zero extension root is a well-formed header, but this one was rejected"
            );
        }
    }
});
