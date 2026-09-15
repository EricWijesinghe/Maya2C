//! Fuzzes the check on a block-sync response.
//!
//! A response is bytes a remote peer chose. libp2p's CBOR codec decodes the
//! envelope under a size cap; what this crate does with the result is
//! `accept_response`, which decodes every block and refuses anything nobody
//! asked for. The properties: no input panics, and nothing is accepted that
//! was not requested.

#![no_main]

use custom_l1_node::core::Block;
use custom_l1_node::network::sync::{BlockBytes, BlockResponse, accept_response};
use libfuzzer_sys::fuzz_target;

/// Up to four length-prefixed chunks, each an attempted block.
fn chunks(mut data: &[u8]) -> Vec<BlockBytes> {
    let mut out = Vec::new();
    while data.len() >= 2 && out.len() < 4 {
        let length = usize::from(u16::from_le_bytes([data[0], data[1]])).min(data.len() - 2);
        out.push(BlockBytes(data[2..2 + length].to_vec()));
        data = &data[2 + length..];
    }
    out
}

fuzz_target!(|data: &[u8]| {
    let blocks = chunks(data);
    let requested: Vec<[u8; 32]> = blocks
        .iter()
        .filter_map(|bytes| Block::from_bytes(&bytes.0).ok())
        .map(|block| block.header.id())
        .collect();

    if let Ok(accepted) = accept_response(
        &requested,
        BlockResponse {
            blocks: blocks.clone(),
        },
    ) {
        assert!(
            accepted
                .iter()
                .all(|block| requested.contains(&block.header.id()))
        );
    }
    // Asked for nothing: only an empty answer may pass.
    if let Ok(accepted) = accept_response(&[], BlockResponse { blocks }) {
        assert!(accepted.is_empty());
    }
});
