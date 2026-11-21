//! A pruned node keeps one commitment; an untrusted archive serves old
//! transactions; the node accepts exactly the ones the chain committed to.

#![allow(clippy::cast_possible_truncation)]

use maya_history_compactor::{
    ArchiveMmr, BlockLeaf, Commitment, Compactor, DecodeError, InclusionProof, VerifyError,
    tx_proof, tx_root, verify_transaction,
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
        verify_transaction(
            &commitment,
            &leaf,
            &block_proof,
            b"mint 1e9 to mallory",
            &proof
        ),
        Err(VerifyError::TxNotInBlock)
    );
    // A block that was never in the chain, with its own consistent tx root.
    let forged_txs = vec![b"mint 1e9 to mallory".to_vec()];
    let forged = leaf_for(h, &forged_txs);
    let forged_proof = tx_proof(&forged_txs, 0).expect("in range");
    assert_eq!(
        verify_transaction(
            &commitment,
            &forged,
            &block_proof,
            &forged_txs[0],
            &forged_proof
        ),
        Err(VerifyError::BlockNotCommitted)
    );
    // The right block presented at another height.
    let moved = BlockLeaf {
        height: h + 1,
        ..leaf
    };
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
    assert_eq!(
        InclusionProof::decode(&header),
        Err(DecodeError::TooManyHashes)
    );
    header[16] = 1;
    assert_eq!(InclusionProof::decode(&header), Err(DecodeError::Truncated));
    header.extend_from_slice(&[0u8; 33]);
    assert_eq!(
        InclusionProof::decode(&header),
        Err(DecodeError::TrailingBytes)
    );
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

// ---------------------------------------------------------------------------
// red team: the right hashes in the wrong shape, and hostile sizes
// ---------------------------------------------------------------------------

fn small_archive(blocks: u64) -> (ArchiveMmr, Vec<BlockLeaf>) {
    let mut archive = ArchiveMmr::new();
    let leaves: Vec<BlockLeaf> = (0..blocks).map(|h| leaf_for(h, &block_txs(h))).collect();
    for leaf in &leaves {
        archive.append(leaf);
    }
    (archive, leaves)
}

#[test]
fn a_proof_moved_to_another_index_or_leaf_count_is_refused() {
    // 11 = 8 + 2 + 1: three peaks, so a leaf in every tree is exercised.
    let (archive, leaves) = small_archive(11);
    let c = archive.commitment();
    let wrong_count = Commitment {
        leaves: 12,
        root: c.root,
    };
    for i in 0..11 {
        let leaf = &leaves[i as usize];
        let proof = archive.prove(i).expect("held");
        assert!(proof.verify(&c, leaf), "leaf {i}");
        for j in (0..11).filter(|&j| j != i) {
            let moved = InclusionProof {
                leaf_index: j,
                ..proof.clone()
            };
            assert!(!moved.verify(&c, leaf), "leaf {i} claimed at {j}");
        }
        let recounted = InclusionProof {
            leaves: 12,
            ..proof.clone()
        };
        assert!(!recounted.verify(&c, leaf), "leaf {i} under 12 leaves");
        assert!(
            !proof.verify(&wrong_count, leaf),
            "the count is bound into the root"
        );
    }
}

#[test]
fn a_sibling_or_peak_added_dropped_or_reordered_is_refused() {
    let (archive, leaves) = small_archive(11);
    let c = archive.commitment();
    // Leaf 5 sits in the 8-tree: three siblings, and the 2- and 1-tree peaks.
    let proof = archive.prove(5).expect("held");
    assert_eq!((proof.siblings.len(), proof.other_peaks.len()), (3, 2));
    let leaf = &leaves[5];

    let mut extra = proof.clone();
    extra.siblings.push([7; 32]);
    assert!(!extra.verify(&c, leaf), "an extra sibling");
    let mut short = proof.clone();
    short.siblings.pop();
    assert!(!short.verify(&c, leaf), "a missing sibling");
    let mut swapped = proof.clone();
    swapped.siblings.swap(0, 1);
    assert!(!swapped.verify(&c, leaf), "siblings out of order");
    let mut reversed = proof.clone();
    reversed.other_peaks.reverse();
    assert!(!reversed.verify(&c, leaf), "peaks out of order");
    let mut more = proof.clone();
    more.other_peaks.push([7; 32]);
    assert!(!more.verify(&c, leaf), "an extra peak");
    let mut fewer = proof;
    fewer.other_peaks.pop();
    assert!(!fewer.verify(&c, leaf), "a missing peak");
}

#[test]
fn the_same_blocks_in_another_order_give_another_commitment() {
    let (archive, leaves) = small_archive(8);
    let mut swapped = leaves.clone();
    swapped.swap(2, 3);
    let mut reordered = ArchiveMmr::new();
    for leaf in &swapped {
        reordered.append(leaf);
    }
    assert_eq!(reordered.len(), archive.len());
    assert_ne!(reordered.commitment(), archive.commitment());
    // Block 2's proof from the reordered history does not place it in the real one.
    let proof = reordered.prove(3).expect("held");
    assert!(proof.verify(&reordered.commitment(), &leaves[2]));
    assert!(!proof.verify(&archive.commitment(), &leaves[2]));
}

#[test]
fn an_empty_history_commits_the_same_everywhere_and_proves_nothing() {
    let pruned = Compactor::new();
    let archive = ArchiveMmr::new();
    assert!(pruned.is_empty() && archive.is_empty());
    assert_eq!(pruned.commitment(), archive.commitment());
    assert!(archive.prove(0).is_none());
}

#[test]
fn a_transaction_from_another_block_is_refused_under_this_block() {
    let (pruned, archive) = chain();
    let commitment = pruned.commitment();
    let (a, b) = (100, 101);
    let (txs_a, txs_b) = (block_txs(a), block_txs(b));
    assert!(txs_a.len() > 1, "index 1 must exist for the move below");
    let leaf_a = leaf_for(a, &txs_a);
    let block_proof = archive.prove(a).expect("held");
    // A real transaction with a real proof, but of block b.
    let proof_b = tx_proof(&txs_b, 0).expect("in range");
    assert_eq!(
        verify_transaction(&commitment, &leaf_a, &block_proof, &txs_b[0], &proof_b),
        Err(VerifyError::TxNotInBlock)
    );
    // A real transaction of block a, its proof claiming another index.
    let mut moved = tx_proof(&txs_a, 0).expect("in range");
    moved.index = 1;
    assert_eq!(
        verify_transaction(&commitment, &leaf_a, &block_proof, &txs_a[0], &moved),
        Err(VerifyError::TxNotInBlock)
    );
}

#[test]
fn a_hostile_proof_at_the_largest_leaf_count_is_refused_without_overflow() {
    // u64::MAX leaves is 64 peaks, the largest shape the wire admits. The
    // shape arithmetic must not overflow: a debug build would panic.
    let c = Commitment {
        leaves: u64::MAX,
        root: [9; 32],
    };
    let leaf = leaf_for(0, &block_txs(0));
    for leaf_index in [0, 1 << 62, u64::MAX - 1] {
        for siblings in [0usize, 1, 63] {
            let proof = InclusionProof {
                leaf_index,
                leaves: u64::MAX,
                siblings: vec![[1; 32]; siblings],
                other_peaks: vec![[2; 32]; 63],
            };
            assert!(!proof.verify(&c, &leaf));
            let wire = InclusionProof::decode(&proof.encode()).expect("within bounds");
            assert_eq!(wire, proof);
        }
    }
}
