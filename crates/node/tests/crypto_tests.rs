//! Integration tests for ML-DSA-65 transaction authorization and ArgonBlake
//! proof-of-work evaluation.
//!
//! ## Cost discipline
//!
//! Each ArgonBlake digest deliberately costs 32 MiB of memory-hard work, so
//! these tests treat Argon2 calls as a scarce resource: target evaluation is
//! exercised as a pure function over synthetic digests, and only a handful of
//! real hashes are computed. The one test that actually mines is `#[ignore]`d.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::{Block, BlockHeader, Transaction, TxInput, TxOutput};
use custom_l1_node::crypto::argon_blake::{HASH_LEN, argon_blake_hash};
use custom_l1_node::crypto::hybrid::{HybridPublicKey, generate_signing_key};
use custom_l1_node::crypto::pow::{leading_zero_bits, meets_target, target_from_leading_zero_bits};
use custom_l1_node::error::NodeError;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn sample_transaction() -> Transaction {
    Transaction::new(
        vec![TxInput {
            prev_tx: [1u8; 32],
            index: 0,
        }],
        vec![TxOutput {
            amount: 1_000,
            recipient: [2u8; 32],
        }],
        7,
    )
}

fn sample_header(difficulty_target: [u8; HASH_LEN]) -> BlockHeader {
    BlockHeader {
        prev_hash: [3u8; 32],
        state_root: [4u8; 32],
        timestamp: 1_756_252_800,
        nonce: 0,
        difficulty_target,
        tx_root: [5u8; 32],
    }
}

// ---------------------------------------------------------------------------
// signature validity
// ---------------------------------------------------------------------------

#[test]
fn signed_transaction_verifies() {
    let key = generate_signing_key().expect("keygen");
    let mut tx = sample_transaction();
    tx.sign(&key).expect("sign");
    assert!(tx.signature.is_some(), "sign must populate the signature");
    assert_eq!(
        *tx.public_key,
        key.public_key(),
        "sign must adopt the signer's public key"
    );
    assert_eq!(
        tx.sender(),
        key.address(),
        "the sender address must be the hash of that key"
    );
    assert_eq!(tx.verify(), Ok(()));
}

#[test]
fn unsigned_transaction_reports_missing_signature() {
    let tx = sample_transaction();
    assert_eq!(tx.verify(), Err(NodeError::MissingSignature));
}

#[test]
fn tampered_output_amount_fails_verification() {
    let key = generate_signing_key().expect("keygen");
    let mut tx = sample_transaction();
    tx.sign(&key).expect("sign");
    tx.outputs[0].amount += 1;

    assert_eq!(tx.verify(), Err(NodeError::SignatureVerification));
}

#[test]
fn tampered_nonce_fails_verification() {
    let key = generate_signing_key().expect("keygen");
    let mut tx = sample_transaction();
    tx.sign(&key).expect("sign");
    tx.nonce = tx.nonce.wrapping_add(1);

    assert_eq!(tx.verify(), Err(NodeError::SignatureVerification));
}

#[test]
fn added_input_fails_verification() {
    let key = generate_signing_key().expect("keygen");
    let mut tx = sample_transaction();
    tx.sign(&key).expect("sign");
    tx.inputs.push(TxInput {
        prev_tx: [9u8; 32],
        index: 3,
    });

    assert_eq!(tx.verify(), Err(NodeError::SignatureVerification));
}

#[test]
fn substituted_public_key_fails_verification() {
    let signer = generate_signing_key().expect("keygen");
    let impostor = generate_signing_key().expect("keygen");
    let mut tx = sample_transaction();
    tx.sign(&signer).expect("sign");
    // The signature commits to the public key, so swapping it must fail rather
    // than letting another identity claim an existing signature.
    *tx.public_key = impostor.public_key();

    assert_eq!(tx.verify(), Err(NodeError::SignatureVerification));
}

#[test]
fn signature_from_another_transaction_fails_verification() {
    let key = generate_signing_key().expect("keygen");

    let mut signed = sample_transaction();
    signed.sign(&key).expect("sign");
    let mut other = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: 999_999,
            recipient: [8u8; 32],
        }],
        42,
    );
    other.public_key = signed.public_key;
    other.signature = signed.signature;

    assert_eq!(other.verify(), Err(NodeError::SignatureVerification));
}

#[test]
fn malformed_public_key_is_rejected() {
    let key = generate_signing_key().expect("keygen");
    let mut tx = sample_transaction();
    tx.sign(&key).expect("sign");
    // An ML-DSA-65 public key is rho followed by packed t1 coefficients, and
    // every 1952-byte string unpacks to *some* coefficient vector — there is no
    // point decompression to fail, the way there was for a compressed Edwards
    // y-coordinate. So a garbage key is rejected at the signature, not at the
    // decode, and the test asserts the outcome that actually matters: it does
    // not verify.
    *tx.public_key = HybridPublicKey {
        lattice: [0xE0u8; custom_l1_node::crypto::keys::PUBLIC_KEY_LEN],
        hash_based: [0xE0u8; custom_l1_node::crypto::hybrid::SLH_DSA_PUBLIC_KEY_LEN],
    };

    assert!(
        matches!(
            tx.verify(),
            Err(NodeError::SignatureVerification | NodeError::MalformedPublicKey)
        ),
        "a substituted key must never verify"
    );
}

#[test]
fn distinct_transactions_have_distinct_ids() {
    let key = generate_signing_key().expect("keygen");

    let mut first = sample_transaction();
    first.sign(&key).expect("sign");
    let mut second = sample_transaction();
    second.nonce = 8;
    second.sign(&key).expect("sign");
    assert_ne!(first.txid(), second.txid());
}

#[test]
fn block_verifies_all_transaction_signatures() {
    let key = generate_signing_key().expect("keygen");

    let mut good = sample_transaction();
    good.sign(&key).expect("sign");
    let mut tampered = sample_transaction();
    tampered.sign(&key).expect("sign");
    tampered.outputs[0].amount = 0;

    let target = target_from_leading_zero_bits(0);

    let valid_block = Block::new(sample_header(target), vec![good.clone()]);
    assert_eq!(valid_block.verify_transactions(), Ok(()));

    let invalid_block = Block::new(sample_header(target), vec![good, tampered]);
    assert_eq!(
        invalid_block.verify_transactions(),
        Err(NodeError::SignatureVerification)
    );
}

// ---------------------------------------------------------------------------
// PoW target evaluation (pure — no Argon2)
// ---------------------------------------------------------------------------

#[test]
fn zero_digest_meets_every_target() {
    let zero = [0u8; HASH_LEN];
    assert!(meets_target(&zero, &target_from_leading_zero_bits(0)));
    assert!(meets_target(&zero, &target_from_leading_zero_bits(128)));
    assert!(meets_target(&zero, &target_from_leading_zero_bits(256)));
}

#[test]
fn max_digest_meets_only_the_easiest_target() {
    let max = [0xFFu8; HASH_LEN];
    assert!(meets_target(&max, &target_from_leading_zero_bits(0)));
    assert!(!meets_target(&max, &target_from_leading_zero_bits(1)));
}

#[test]
fn digest_equal_to_target_meets_it() {
    // The comparison is inclusive: hash <= target.
    let target = target_from_leading_zero_bits(16);
    assert!(meets_target(&target, &target));
}

#[test]
fn digest_one_above_target_fails() {
    let target = target_from_leading_zero_bits(16);
    let mut just_over = target;
    // target for 16 bits is 00 00 FF FF ... ; bumping the first non-zero byte
    // region upward crosses the threshold.
    just_over[1] = 0x01;
    assert!(!meets_target(&just_over, &target));
}

#[test]
fn target_layout_is_correct_for_byte_and_sub_byte_boundaries() {
    let none = target_from_leading_zero_bits(0);
    assert_eq!(none, [0xFFu8; HASH_LEN]);

    let nibble = target_from_leading_zero_bits(4);
    assert_eq!(nibble[0], 0x0F);
    assert_eq!(nibble[1], 0xFF);

    let one_byte = target_from_leading_zero_bits(8);
    assert_eq!(one_byte[0], 0x00);
    assert_eq!(one_byte[1], 0xFF);

    let byte_and_a_half = target_from_leading_zero_bits(12);
    assert_eq!(byte_and_a_half[0], 0x00);
    assert_eq!(byte_and_a_half[1], 0x0F);
    assert_eq!(byte_and_a_half[2], 0xFF);

    let saturated = target_from_leading_zero_bits(256);
    assert_eq!(saturated, [0u8; HASH_LEN]);

    // Beyond the digest width the target stays saturated rather than wrapping.
    let beyond = target_from_leading_zero_bits(1_000);
    assert_eq!(beyond, [0u8; HASH_LEN]);
}

#[test]
fn target_has_exactly_the_requested_leading_zeros() {
    for bits in [0u32, 1, 4, 7, 8, 9, 16, 31, 32, 100, 255] {
        let target = target_from_leading_zero_bits(bits);
        assert_eq!(
            leading_zero_bits(&target),
            bits,
            "target for {bits} bits has the wrong leading-zero count"
        );
    }
}

#[test]
fn meets_target_agrees_with_leading_zero_bits() {
    // A digest satisfies an n-bit target exactly when it has >= n leading zeros.
    let mut digest = [0u8; HASH_LEN];
    digest[2] = 0x3F; // 16 + 2 = 18 leading zero bits
    assert_eq!(leading_zero_bits(&digest), 18);

    for bits in 0..=18 {
        assert!(
            meets_target(&digest, &target_from_leading_zero_bits(bits)),
            "digest with 18 leading zeros should satisfy a {bits}-bit target"
        );
    }
    for bits in 19..=32 {
        assert!(
            !meets_target(&digest, &target_from_leading_zero_bits(bits)),
            "digest with 18 leading zeros should not satisfy a {bits}-bit target"
        );
    }
}

#[test]
fn leading_zero_bits_counts_full_zero_digest() {
    assert_eq!(leading_zero_bits(&[0u8; HASH_LEN]), 256);
    assert_eq!(leading_zero_bits(&[0xFFu8; HASH_LEN]), 0);
}

// ---------------------------------------------------------------------------
// ArgonBlake (real hashing — kept to a small, fixed number of calls)
// ---------------------------------------------------------------------------

/// Frozen known-answer vector for the ArgonBlake digest.
///
/// This is a **consensus gate, not a regression test.** The digest is what
/// proof-of-work is measured against, so any change to it — a different Argon2
/// implementation, altered parameters, a reordered stage, even a compiler flag
/// that changed arithmetic — invalidates every mined block, every stored
/// header, and every `genesis.json` in existence.
///
/// If this test fails, the change under review is a hard fork. It is never
/// correct to simply update the expected value.
const KAT_DIGEST: &str = "ebdd87f0608df19740abeeeebe33b1f315ab0861d4649890fe425627bf13bcce";

/// Input length [`KAT_DIGEST`] was frozen over: the header length before
/// `tx_root` was added.
///
/// Literal rather than [`custom_l1_node::core::HEADER_LEN`] on purpose. This
/// vector freezes the *function*. When the header grew to 144 bytes the function
/// did not change, and this vector still passing on its original input is the
/// evidence. The 144-byte header has its own vector below.
const KAT_INPUT_LEN: usize = 112;

/// ArgonBlake over a full-length 144-byte header of the same pattern.
///
/// Added 2026-09-11 when `tx_root` grew the header, and mirrored in
/// `hal/cuda-miner/tests/parity.rs`, whose split hasher only accepts
/// full-length headers. It was frozen from the node's output; the GPU
/// split was checked against it independently. The same rule applies: it is
/// never correct to update it to make a failure go away.
const KAT_DIGEST_HEADER: &str = "5b3992c971393e4127ccd78fb00c2d82217ecdc41b3a0c3caab0629531a78e44";

#[test]
fn argon_blake_matches_the_frozen_known_answer_on_a_full_header() {
    let header = [0x5Au8; custom_l1_node::core::HEADER_LEN];
    let digest = argon_blake_hash(&header).expect("hash must succeed");

    assert_eq!(
        hex::encode(digest),
        KAT_DIGEST_HEADER,
        "ArgonBlake over a full header changed — consensus-breaking, not a test to update"
    );
}

#[test]
fn argon_blake_matches_the_frozen_known_answer() {
    // A fixed, non-trivial byte pattern, at the length the vector was frozen at.
    let header = [0x5Au8; KAT_INPUT_LEN];
    let digest = argon_blake_hash(&header).expect("hash must succeed");

    assert_eq!(
        hex::encode(digest),
        KAT_DIGEST,
        "ArgonBlake output changed — this is consensus-breaking, not a test to update"
    );
}

#[test]
fn argon_blake_is_deterministic() {
    let header = sample_header(target_from_leading_zero_bits(0)).serialize();

    let first = argon_blake_hash(&header).expect("hash must succeed");
    let second = argon_blake_hash(&header).expect("hash must succeed");

    assert_eq!(
        first, second,
        "PoW is unverifiable unless hashing is deterministic"
    );
    assert_ne!(first, [0u8; HASH_LEN], "digest must not be all zeros");
}

#[test]
fn argon_blake_changes_with_a_single_bit_flip() {
    let base = sample_header(target_from_leading_zero_bits(0));
    let mut flipped = base.clone();
    flipped.nonce = 1;

    let a = argon_blake_hash(&base.serialize()).expect("hash must succeed");
    let b = argon_blake_hash(&flipped.serialize()).expect("hash must succeed");

    assert_ne!(a, b, "changing the nonce must change the digest");
}

#[test]
fn header_difficulty_check_is_wired_to_the_pow_hash() {
    // Evaluated against fixed extreme targets so no mining is required: the
    // easiest target is always satisfied, the hardest essentially never is.
    let always = sample_header(target_from_leading_zero_bits(0));
    assert_eq!(always.meets_difficulty(), Ok(true));

    let never = sample_header(target_from_leading_zero_bits(256));
    assert_eq!(never.meets_difficulty(), Ok(false));
}

#[test]
fn header_serialization_is_fixed_width_and_positional() {
    let header = sample_header(target_from_leading_zero_bits(32));
    let bytes = header.serialize();

    assert_eq!(bytes.len(), custom_l1_node::core::HEADER_LEN);
    assert_eq!(&bytes[0..32], &header.prev_hash);
    assert_eq!(&bytes[32..64], &header.state_root);
    assert_eq!(&bytes[64..72], &header.timestamp.to_le_bytes());
    assert_eq!(&bytes[72..80], &header.nonce.to_le_bytes());
    assert_eq!(&bytes[80..112], &header.difficulty_target);
    assert_eq!(&bytes[custom_l1_node::core::TX_ROOT_RANGE], &header.tx_root);
    assert_eq!(
        BlockHeader::from_bytes(&bytes).expect("round trip"),
        header,
        "decoding must invert the layout, tx_root included"
    );
}

// ---------------------------------------------------------------------------
// the header commits to the transactions
// ---------------------------------------------------------------------------

fn signed_transfers(count: u64) -> Vec<Transaction> {
    let key = generate_signing_key().expect("keygen");
    (0..count)
        .map(|nonce| {
            let mut tx = Transaction::new(
                vec![],
                vec![TxOutput {
                    amount: 10 + nonce,
                    recipient: [7u8; 32],
                }],
                nonce,
            );
            tx.sign(&key).expect("sign");
            tx
        })
        .collect()
}

#[test]
fn block_identity_and_proof_of_work_cover_the_transaction_root() {
    // The substance of the fix. Before `tx_root` existed, both of these hashed
    // the header alone, so two blocks with different transactions had one id
    // and one proof of work.
    let header = sample_header(target_from_leading_zero_bits(0));
    let mut other = header.clone();
    other.tx_root[0] ^= 1;

    assert_ne!(header.id(), other.id(), "the id must cover tx_root");
    assert_ne!(
        header.pow_seed(),
        other.pow_seed(),
        "the DAG seed must cover tx_root"
    );
    assert_ne!(
        header.pow_hash().expect("hash"),
        other.pow_hash().expect("hash"),
        "the ArgonBlake digest must cover tx_root"
    );
}

#[test]
fn block_new_commits_the_header_to_its_transactions() {
    let transactions = signed_transfers(3);
    let block = Block::new(
        sample_header(target_from_leading_zero_bits(0)),
        transactions,
    );

    assert_eq!(block.header.tx_root, block.tx_root());
    assert!(block.check_tx_root().is_ok());

    let empty = Block::new(sample_header(target_from_leading_zero_bits(0)), Vec::new());
    assert_eq!(
        empty.header.tx_root, [0u8; 32],
        "no transactions roots to zero"
    );
}

#[test]
fn a_decoded_block_whose_body_disagrees_with_its_header_is_caught() {
    // The wire keeps the header it was sent, so a relay that swaps the body
    // produces a block that decodes cleanly and fails `check_tx_root`.
    let block = Block::new(
        sample_header(target_from_leading_zero_bits(0)),
        signed_transfers(2),
    );
    let replacement = signed_transfers(1);

    let mut forged = Block::from_bytes(&block.to_bytes()).expect("decode");
    assert_eq!(forged, block, "a round trip must be exact");
    forged.transactions = replacement;

    let reencoded = Block::from_bytes(&forged.to_bytes()).expect("the forgery decodes");
    assert!(matches!(
        reencoded.check_tx_root(),
        Err(NodeError::TxRootMismatch { .. })
    ));
}

#[test]
fn transaction_order_is_committed() {
    // Order is consensus: a batch settles in block order, so a reordered block
    // is a different block, and it has to have a different root.
    let transactions = signed_transfers(2);
    let forward = Block::new(
        sample_header(target_from_leading_zero_bits(0)),
        transactions.clone(),
    );
    let reversed = Block::new(
        sample_header(target_from_leading_zero_bits(0)),
        transactions.into_iter().rev().collect(),
    );
    assert_ne!(forward.header.tx_root, reversed.header.tx_root);
}

#[test]
fn every_transaction_proves_its_inclusion_against_the_header() {
    use custom_l1_node::core::transaction_leaf;
    use custom_l1_node::state::merkle::verify_path;

    let block = Block::new(
        sample_header(target_from_leading_zero_bits(0)),
        signed_transfers(5),
    );
    for (index, tx) in block.transactions.iter().enumerate() {
        let path = block.tx_inclusion_path(index).expect("in range");
        assert_eq!(
            verify_path(&transaction_leaf(&tx.txid()), &path),
            block.header.tx_root,
            "transaction {index} did not prove"
        );
    }
    assert!(block.tx_inclusion_path(5).is_none());
}

/// Opt-in: actually mines a header.
///
/// Every attempt costs a full 32 MiB Argon2id pass (tens of milliseconds), so
/// even this low difficulty takes a while. Run with:
/// `cargo test --release -- --ignored --nocapture`
#[test]
#[ignore = "memory-hard mining loop; run explicitly with --ignored"]
fn mining_finds_a_header_meeting_a_low_target() {
    const BITS: u32 = 8;
    const MAX_ATTEMPTS: u64 = 20_000;

    let mut header = sample_header(target_from_leading_zero_bits(BITS));
    let mut found = false;

    for nonce in 0..MAX_ATTEMPTS {
        header.nonce = nonce;
        if header.meets_difficulty().expect("hash must succeed") {
            println!("solved at nonce {nonce}");
            found = true;
            break;
        }
    }

    assert!(found, "no solution within {MAX_ATTEMPTS} attempts");
    assert!(leading_zero_bits(&header.pow_hash().expect("hash must succeed")) >= BITS);
}
