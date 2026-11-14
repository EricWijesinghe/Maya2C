//! Fuzzes [`parse_user_identity`]: the `user_identity` a miner sends when it
//! opens a channel, `<address hex>[.<worker name>]`.
//!
//! The string arrives from whatever dialled the mining port, before any
//! account is known. Properties:
//!
//! - **No panic** on any text.
//! - **Every accepted identity is within the stated bounds:** a non-empty
//!   worker name of at most `MAX_WORKER_NAME` bytes.
//! - **Rendering an accepted identity and parsing it again gives the same
//!   key.** The key is what credits accrue to, so a rendering that changed the
//!   account would credit one miner's work to another.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use libfuzzer_sys::fuzz_target;
use maya_pool_service::model::{MAX_WORKER_NAME, parse_user_identity};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    if let Ok(key) = parse_user_identity(text) {
        assert!(!key.worker.is_empty(), "an accepted identity has a worker name");
        assert!(
            key.worker.len() <= MAX_WORKER_NAME,
            "an accepted worker name exceeds the bound"
        );
        let rendered = key.display();
        let reparsed = parse_user_identity(&rendered)
            .expect("an accepted identity renders to one that parses");
        assert_eq!(reparsed, key, "the rendered identity names a different key");
    }
});
