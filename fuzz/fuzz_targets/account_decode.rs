//! Fuzzes [`Account::decode`], the on-disk account record codec.
//!
//! Unlike the other targets this one reads storage rather than the network, so
//! the threat model is a corrupt or foreign RocksDB rather than a hostile peer.
//! It is fuzzed anyway for the same reason `ShieldedPool::settle` checks an
//! underflow the circuit already prevents: a decoder that panics on a corrupt
//! record turns a recoverable database problem into a node that cannot start.
//!
//! Like the header, the record is fixed-width and every 16-byte string is a
//! valid account, so the accept/reject boundary is exactly the length check.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::Account;
use custom_l1_node::state::ACCOUNT_LEN;
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
            assert_ne!(
                data.len(),
                ACCOUNT_LEN,
                "every {ACCOUNT_LEN}-byte string is a well-formed account record, \
                 but this one was rejected"
            );
        }
    }
});
