//! A pruned node keeps one commitment; an untrusted archive serves old
//! transactions; the node accepts exactly the ones the chain committed to.

#![allow(clippy::cast_possible_truncation)]

use maya_history_compactor::{
    ArchiveMmr, BlockLeaf, Compactor, DecodeError, InclusionProof, VerifyError, tx_proof, tx_root,
    verify_transaction,
};

fn block_txs(height: u64) -> Vec<Vec<u8>> {
    let n = (height % 7 + 1) as usize;
    (0..n)
        .map(|i| format!("tx {i} of block {height}").into_bytes())
        .collect()
}

fn leaf_for(height: u64, txs: &[Vec<u8>]) -> BlockLeaf {
    BlockLeaf {
        height,
        block_id: *blake3::hash(&height.to_be_bytes()).as_bytes(),
        tx_root: tx_root(txs),
    }
}

const BLOCKS: u64 = 10_000;

fn chain() -> (Compactor, ArchiveMmr) {
    let mut pruned = Compactor::new();
    let mut archive = ArchiveMmr::new();
    for h in 0..BLOCKS {
        let leaf = leaf_for(h, &block_txs(h));
        pruned.append(&leaf);
        archive.append(&leaf);
    }
    (pruned, archive)
}

#[test]
fn a_pruned_node_verifies_old_transactions_from_an_archive() {
    let (pruned, archive) = chain();
    let commitment = pruned.commitment();
    assert!(pruned.peak_count() <= 64);
    for h in (0..BLOCKS).step_by(997) {
        let txs = block_txs(h);
        let leaf = leaf_for(h, &txs);
        let block_proof = archive.prove(h).expect("archive holds it");
        let last = txs.len() - 1;
        let proof = tx_proof(&txs, last).expect("in range");
        assert_eq!(
            verify_transaction(&commitment, &leaf, &block_proof, &txs[last], &proof),
            Ok(())
        );
        // Proof bytes survive the wire.
        let wire = InclusionProof::decode(&block_proof.encode()).expect("decodes");
        assert_eq!(wire, block_proof);
    }
}

#[test]
fn a_lying_archive_is_caught() {
    let (pruned, archive) = chain();
    let commitment = pruned.commitment();
    let h = 4_242;
    let txs = block_txs(h);
    let leaf = leaf_for(h, &txs);
    let block_proof = archive.prove(h).expect("held");
    let proof = tx_proof(&txs, 0).expect("in range");

    // A transaction that was never in the block.
    assert_eq!(
        verify_transaction(&commitment, &leaf, &block_proof, b"mint 1e9 to mallory", &proof),
        Err(VerifyError::TxNotInBlock)
    );
    // A block that was never in the chain, with its own consistent tx root.
    let forged_txs = vec![b"mint 1e9 to mallory".to_vec()];
    let forged = leaf_for(h, &forged_txs);
    let forged_proof = tx_proof(&forged_txs, 0).expect("in range");
    assert_eq!(
        verify_transaction(&commitment, &forged, &block_proof, &forged_txs[0], &forged_proof),
        Err(VerifyError::BlockNotCommitted)
    );
    // The right block presented at another height.
    let moved = BlockLeaf { height: h + 1, ..leaf };
    assert_eq!(
        verify_transaction(&commitment, &moved, &block_proof, &txs[0], &proof),
        Err(VerifyError::BlockNotCommitted)
    );
    // One flipped bit anywhere in the proof.
    let mut bytes = block_proof.encode();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    let tampered = InclusionProof::decode(&bytes).expect("shape is fine");
    assert_eq!(
        verify_transaction(&commitment, &leaf, &tampered, &txs[0], &proof),
        Err(VerifyError::BlockNotCommitted)
    );
}

#[test]
fn the_decoder_refuses_malformed_bytes_without_panicking() {
    assert_eq!(InclusionProof::decode(&[]), Err(DecodeError::Truncated));
    let mut header = vec![0u8; 18];
    header[16] = 65;
    assert_eq!(InclusionProof::decode(&header), Err(DecodeError::TooManyHashes));
    header[16] = 1;
    assert_eq!(InclusionProof::decode(&header), Err(DecodeError::Truncated));
    header.extend_from_slice(&[0u8; 33]);
    assert_eq!(InclusionProof::decode(&header), Err(DecodeError::TrailingBytes));
    // Every prefix of a real proof, and a cheap deterministic byte stream.
    let (_, archive) = chain();
    let real = archive.prove(1234).expect("held").encode();
    for cut in 0..real.len() {
        let _ = InclusionProof::decode(&real[..cut]);
    }
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..10_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let len = (x % 600) as usize;
        let bytes: Vec<u8> = (0..len).map(|i| (x >> (i % 56)) as u8).collect();
        let _ = InclusionProof::decode(&bytes);
    }
}
