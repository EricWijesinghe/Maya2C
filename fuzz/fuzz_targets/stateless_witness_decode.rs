//! Fuzzes the stateless witness decoder over both backends.
//!
//! A witness travels beside a block, outside its id and its proof of work, so
//! every byte of it is a relay's choice. The properties: no input panics or
//! allocates past the node cap, anything that decodes re-encodes to exactly the
//! bytes that produced it (a second encoding would be a second tree), and a
//! decoded tree's digest can be computed.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use maya_stateless_core::{Blake3, PartialTree, RingSis};

fn ring() -> &'static RingSis {
    static RING: OnceLock<RingSis> = OnceLock::new();
    RING.get_or_init(RingSis::new)
}

fuzz_target!(|data: &[u8]| {
    if let Ok(tree) = PartialTree::decode(&Blake3, data) {
        let mut out = Vec::new();
        tree.encode::<Blake3>(&mut out);
        assert_eq!(out, data);
        let _ = tree.digest(&Blake3);
    }
    if let Ok(tree) = PartialTree::decode(ring(), data) {
        let mut out = Vec::new();
        tree.encode::<RingSis>(&mut out);
        assert_eq!(out, data);
    }
});
