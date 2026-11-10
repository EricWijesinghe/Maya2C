//! Fuzzes the faucet's request parsing: the JSON body `POST /grant` accepts and
//! the recipient text inside it, on a testnet faucet (Bech32m or hex accepted)
//! and on a faucet whose chain has no text form (hex only).
//!
//! The body is written by anyone who can reach the faucet. Properties:
//!
//! - **No panic** on any body or any recipient string.
//! - **An accepted recipient has a hex form that means the same account.** The
//!   faucet keys its rate limits on the address it parsed, so a text form and
//!   its hex form landing on different ids would let one person be granted
//!   twice.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::SystemTime;

use libfuzzer_sys::fuzz_target;
use maya_faucet::Faucet;
use maya_faucet::http::GrantRequest;
use primitive_types::U256;

fuzz_target!(|data: &[u8]| {
    let testnet = Faucet::new("maya-testnet-1", U256::from(1u8), U256::from(10u8), SystemTime::UNIX_EPOCH)
        .expect("a testnet faucet builds");
    let devnet = Faucet::new("fuzz-devnet", U256::from(1u8), U256::from(10u8), SystemTime::UNIX_EPOCH)
        .expect("a faucet without a network builds");

    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(id) = testnet.parse_address(text) {
            let hex_form = hex::encode(id);
            let reparsed = testnet
                .parse_address(&hex_form)
                .expect("an accepted account has a hex form that parses");
            assert_eq!(reparsed, id, "the hex form names a different account");
        }
        let _ = devnet.parse_address(text);
    }

    if let Ok(request) = serde_json::from_slice::<GrantRequest>(data) {
        let _ = testnet.parse_address(&request.address);
        let _ = devnet.parse_address(&request.address);
    }
});
