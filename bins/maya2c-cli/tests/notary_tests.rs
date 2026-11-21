//! Notary: hashing, anchoring, tamper detection and receipts, without a node.

use custom_l1_node::core::identity_payload::AnchorAttestation;
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::rpc::types::BlockInfo;
use maya2c_cli::notary::{
    Finality, Placement, SCHEMA, anchored_hash, build_receipt, check_match, decode_block, finality,
    hash_reader, next_height,
};

/// A transaction carrying a notary anchor for `root` under `schema`.
fn anchor_tx(schema: &str, root: [u8; 32]) -> Transaction {
    let mut tx = Transaction::new(vec![], vec![], 0);
    tx.kind = TxKind::AnchorAttestation(Box::new(AnchorAttestation {
        schema: schema.to_string(),
        root,
        expires_at: 0,
    }));
    tx
}

fn placement(height: u64) -> Placement {
    Placement {
        height,
        block_hash: [7; 32],
        timestamp: 1_700_000_000,
    }
}

#[test]
fn hash_of_empty_input_matches_blake3_reference_vector() {
    // BLAKE3 reference value for the empty message.
    let expected = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";
    let got = hash_reader(&b""[..]).expect("in-memory read");
    assert_eq!(hex::encode(got), expected);
}

#[test]
fn streamed_hash_equals_one_shot_hash_across_read_chunks() {
    // Longer than one read buffer, so the loop runs more than once.
    let data: Vec<u8> = (0..(3 * 64 * 1024 + 17))
        .map(|i: usize| u8::try_from(i % 251).expect("below 251"))
        .collect();
    let streamed = hash_reader(&data[..]).expect("in-memory read");
    assert_eq!(streamed, *blake3::hash(&data).as_bytes());
}

#[test]
fn anchored_hash_reads_only_notary_stamps() {
    let root = [9; 32];
    assert_eq!(anchored_hash(&anchor_tx(SCHEMA, root)), Some(root));
    assert_eq!(anchored_hash(&anchor_tx("some-other/v1", root)), None);
    assert_eq!(anchored_hash(&Transaction::new(vec![], vec![], 0)), None);
}

#[test]
fn one_changed_byte_fails_the_match() {
    let original = b"lease agreement, page 1".to_vec();
    let mut tampered = original.clone();
    tampered[5] ^= 0x01;

    let anchored = hash_reader(&original[..]).expect("in-memory read");
    let tx = anchor_tx(SCHEMA, anchored);
    let root = anchored_hash(&tx).expect("notary stamp");

    let intact = hash_reader(&original[..]).expect("in-memory read");
    assert!(check_match(&intact, &root).is_ok());

    let changed = hash_reader(&tampered[..]).expect("in-memory read");
    assert!(check_match(&changed, &root).is_err());
}

#[test]
fn finality_needs_a_checkpoint_at_or_above_the_block() {
    assert_eq!(finality(10, None), Finality::Pending);
    assert_eq!(finality(10, Some(9)), Finality::Pending);
    assert_eq!(finality(10, Some(10)), Finality::Final);
    assert_eq!(finality(10, Some(11)), Finality::Final);
}

#[test]
fn receipt_round_trips_through_json() {
    let root = [0xAB; 32];
    let tx = anchor_tx(SCHEMA, root);
    let receipt = build_receipt(root, &tx, placement(42), Some(50)).expect("receipt");

    assert_eq!(receipt.hash.len(), 64);
    assert_eq!(receipt.txid.len(), 64);
    assert_eq!(receipt.block_hash, hex::encode([7; 32]));
    assert_eq!(receipt.height, 42);
    assert_eq!(receipt.finality, Finality::Final);

    let text = serde_json::to_string(&receipt).expect("serialize");
    let back = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(receipt, back);
}

#[test]
fn receipt_refuses_a_transaction_that_is_not_a_stamp() {
    let tx = anchor_tx("other/v1", [1; 32]);
    assert!(build_receipt([1; 32], &tx, placement(1), None).is_err());
}

fn header(timestamp: u64) -> BlockHeader {
    BlockHeader {
        version: 0,
        prev_hash: [0; 32],
        state_root: [0; 32],
        timestamp,
        nonce: 0,
        difficulty_target: [0xff; 32],
        tx_root: [0; 32],
    }
}

#[test]
fn decode_block_accepts_an_honest_block() {
    let block = Block::new(header(1), vec![anchor_tx(SCHEMA, [1; 32])]);
    let info = BlockInfo::new(5, &block);
    assert_eq!(decode_block(&info).expect("honest block"), block);
}

#[test]
fn decode_block_refuses_a_swapped_body_under_an_honest_header() {
    // The honest block's header (so its id is the real block id) over a
    // forged body, with the JSON tx_root set to the forged body's root.
    let honest = Block::new(header(1), vec![anchor_tx(SCHEMA, [1; 32])]);
    let forged = Block {
        header: honest.header.clone(),
        transactions: vec![anchor_tx(SCHEMA, [2; 32])],
    };
    let mut info = BlockInfo::new(5, &forged);
    info.header.tx_root = hex::encode(forged.tx_root());
    assert_eq!(info.header.id, hex::encode(honest.header.id()));
    let refused = format!("{:#}", decode_block(&info).expect_err("refused"));
    assert!(refused.contains("tx root"), "{refused}");
}

#[test]
fn decode_block_refuses_bytes_that_are_not_the_named_block() {
    let block = Block::new(header(1), vec![]);
    let other = Block::new(header(2), vec![]);
    let mut info = BlockInfo::new(5, &block);
    info.header.id = hex::encode(other.header.id());
    let refused = format!("{:#}", decode_block(&info).expect_err("refused"));
    assert!(
        refused.contains("not the block the node named"),
        "{refused}"
    );
}

#[test]
fn next_height_refuses_a_batch_that_does_not_advance() {
    let block = Block::new(header(1), vec![]);
    let at = |h| BlockInfo::new(h, &block);
    assert_eq!(next_height(&[at(3), at(4)], 3).expect("contiguous"), 5);
    // A node that answers every request with block 0 must not loop verify.
    assert!(next_height(&[at(0)], 3).is_err());
    assert!(next_height(&[at(3), at(3)], 3).is_err());
    assert!(next_height(&[at(4)], 3).is_err());
    assert!(next_height(&[], 3).is_err());
}
