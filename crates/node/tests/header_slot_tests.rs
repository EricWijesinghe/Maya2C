//! The header has room for roots it does not compute yet, and the
//! transaction wire has a number for access lists (ADR-155 §4 and §6: C12,
//! B5, B6, B17).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::block::{BlockHeader, HEADER_LEN};
use custom_l1_node::core::transaction::Transaction;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;

fn header() -> BlockHeader {
    BlockHeader {
        version: 0,
        prev_hash: [1; 32],
        state_root: [2; 32],
        timestamp: 1_790_000_001,
        nonce: 7,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [3; 32],
    }
}

/// Where the reserved extension root sits: after the version byte, so no
/// earlier field moved.
const EXTENSION: std::ops::Range<usize> = 145..177;

#[test]
fn a_header_reserves_an_extension_root_inside_the_bytes_its_id_covers() {
    let bytes = header().serialize();
    assert_eq!(
        bytes.len(),
        177,
        "145 bytes of fields and a 32-byte extension root"
    );
    assert_eq!(HEADER_LEN, 177);
    assert!(
        bytes
            .get(EXTENSION)
            .is_some_and(|root| root.iter().all(|b| *b == 0)),
        "zero until activated"
    );
    // Three BLAKE3 blocks, as before: the GPU kernels' block count is unchanged.
    assert!(bytes.len() <= 3 * 64);
    assert_eq!(BlockHeader::from_bytes(&bytes).unwrap(), header());
}

#[test]
fn a_header_naming_an_extension_root_before_its_activation_is_refused() {
    let mut bytes = header().serialize().to_vec();
    bytes.resize(177, 0);
    assert!(
        BlockHeader::from_bytes(&bytes).is_ok(),
        "a zero root is the only valid one"
    );
    bytes[160] = 1;
    assert!(BlockHeader::from_bytes(&bytes).is_err());
}

#[test]
fn the_access_list_wire_version_is_reserved_and_refused_by_name() {
    let mut frame = Transaction::new(vec![], vec![], 0).to_bytes();
    frame[0] = 14;
    let refused = Transaction::from_bytes(&frame).unwrap_err().to_string();
    assert!(refused.contains("access-list"), "{refused}");
}
