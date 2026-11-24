//! Fuzzes [`Account::decode`], the on-disk account record codec.
//!
//! Unlike the other targets this one reads storage rather than the network, so
//! the threat model is a corrupt or foreign RocksDB rather than a hostile peer.
//! It is fuzzed anyway for the same reason `ShieldedPool::settle` checks an
//! underflow the circuit already prevents: a decoder that panics on a corrupt
//! record turns a recoverable database problem into a node that cannot start.
//!
//! The record is fixed-width, and a string of that width is a valid account
//! exactly when its last byte is the one auth kind this build executes
//! (ADR-155), so the accept/reject boundary is the length and that byte.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::Account;
use custom_l1_node::state::{ACCOUNT_LEN, AUTH_KIND_KEY};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    match Account::decode(data) {
        Ok(account) => {
            assert_eq!(
                data.len(),
                ACCOUNT_LEN,
                "account decoded from {} bytes, but the record is fixed at {ACCOUNT_LEN}",
                data.len(),
            );
            assert_eq!(
                account.encode().as_slice(),
                data,
                "non-canonical account record accepted"
            );
        }
        Err(_) => {
            assert!(
                data.len() != ACCOUNT_LEN || data[ACCOUNT_LEN - 1] != AUTH_KIND_KEY,
                "every {ACCOUNT_LEN}-byte string ending in auth kind {AUTH_KIND_KEY} is a \
                 well-formed account record, but this one was rejected"
            );
        }
    }
});
