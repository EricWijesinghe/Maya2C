//! Fuzzes [`BlockHeader::from_bytes`].
//!
//! The header is fixed-width — 112 bytes, no length prefixes — so the decoder
//! has a narrow surface, but it is reachable from `get_mining_candidate` over
//! JSON-RPC as well as from block gossip, which makes it reachable by callers
//! that never went through `Block::from_bytes`.
//!
//! The canonicality property is total here: exactly one 112-byte string maps to
//! each header, and every 112-byte string is a valid header. So `from_bytes`
//! must succeed for any input of exactly `HEADER_LEN` and fail for every other
//! length — a stronger claim than round-tripping alone, and asserted as such.
//!
//! `pow_hash` and `meets_difficulty` are never called: ArgonBlake is a 32 MiB
//! Argon2id pass, seconds per invocation.

#![no_main]

use custom_l1_node::BlockHeader;
use custom_l1_node::core::HEADER_LEN;
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
            assert_ne!(
                data.len(),
                HEADER_LEN,
                "every {HEADER_LEN}-byte string is a well-formed header, but this one was rejected"
            );
        }
    }
});
