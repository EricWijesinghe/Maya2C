//! Offline signing, end to end, over a real signed transaction.
//!
//! `airgap.rs`'s unit tests frame synthetic bytes. These frame the real thing:
//! a transaction signed by a key derived from a real mnemonic, at the real
//! size a hybrid signature makes it. That is what confirms the chunking
//! constants were chosen against the actual payload and not against an
//! estimate of it.
//!
//! # This is the whole air-gapped flow
//!
//! ```text
//! offline device   mnemonic -> key -> sign -> split -> N QR frames
//! online device    scan frames -> assemble -> verify -> submit
//! ```
//!
//! Nothing here touches the network, which is the point: an offline signer
//! that needed a node would not be offline.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::ChainTag;
use maya_wallet_core::airgap::{self, Assembler, Frame};
use maya_wallet_core::hd::{DerivationPath, seed_from_mnemonic, signing_key_at};
use maya_wallet_core::payment::SignedTransfer;
use maya_wallet_core::wallet::{SignTerms, Wallet};

/// The chain every test signature commits to (ADR-036).
const CHAIN: ChainTag = ChainTag::from_genesis([0x5A; 32]);

/// A fixed phrase, so a failure is reproducible.
const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
                      abandon abandon abandon abandon abandon abandon abandon abandon \
                      abandon abandon abandon abandon abandon abandon abandon art";

fn signed_blob() -> Vec<u8> {
    let seed = seed_from_mnemonic(PHRASE, "").expect("a valid phrase yields a seed");
    let key = signing_key_at(seed.as_slice(), &DerivationPath::account(0, 0)).expect("derive");

    let signed = sign_transfer(
        &key,
        &hex::encode([9u8; 32]),
        primitive_types::U256::from(1_000),
        primitive_types::U256::from(0),
        10,
        &CHAIN,
    )
    .expect("signing needs no network");

    hex::decode(&signed.raw_hex).expect("the wallet emits valid hex")
}

/// Signs through the wallet's confirm-hash gate, as the GUI does: the hash is
/// the confirmation of these same terms. `key` must be account 0 of `PHRASE`.
fn bound_sign(
    key: &custom_l1_node::crypto::hybrid::HybridSigningKey,
    recipient: &str,
    amount: primitive_types::U256,
    terms: SignTerms<'_>,
) -> maya_wallet_core::error::Result<SignedTransfer> {
    let mut wallet = Wallet::from_mnemonic(PHRASE, "").expect("wallet");
    let account = wallet.account(0).expect("account 0");
    assert_eq!(account.address, hex::encode(key.address()), "test key is account 0");
    let confirmed = wallet.confirm(0, recipient, amount, &terms)?.signing_hash;
    wallet.sign(0, recipient, amount, &terms, &confirmed)
}

fn sign_transfer(
    key: &custom_l1_node::crypto::hybrid::HybridSigningKey,
    recipient: &str,
    amount: primitive_types::U256,
    fee: primitive_types::U256,
    nonce: u64,
    chain: &ChainTag,
) -> maya_wallet_core::error::Result<SignedTransfer> {
    bound_sign(key, recipient, amount, SignTerms { fee, fee_to: None, nonce, chain })
}

#[test]
fn a_real_signed_transaction_needs_many_frames() {
    // The measurement the module's constants were chosen against, taken from a
    // real transaction rather than a synthetic one. If a future change made a
    // transaction fit in one QR code, this fails and the chunking can be
    // reconsidered — which is the only circumstance in which it should be.
    const QR_ABSOLUTE_MAX: usize = 2_953;

    let blob = signed_blob();
    assert!(
        blob.len() > QR_ABSOLUTE_MAX,
        "a signed transfer is {} bytes; QR's absolute ceiling is {QR_ABSOLUTE_MAX}",
        blob.len()
    );

    let frames = airgap::split(&blob).expect("frames");
    assert!(
        frames.len() >= 5,
        "a {} byte payload should need several frames, got {}",
        blob.len(),
        frames.len()
    );
    assert_eq!(frames.len(), airgap::frame_count(blob.len()));
}

#[test]
fn the_offline_flow_round_trips_a_real_transaction() {
    // Sign offline, split, encode each frame as a scanner would read it,
    // decode, assemble, and get the same bytes a node would accept.
    let blob = signed_blob();

    let encoded: Vec<Vec<u8>> = airgap::split(&blob)
        .expect("frames")
        .iter()
        .map(Frame::encode)
        .collect();

    let decoded: Vec<Frame> = encoded
        .iter()
        .map(|bytes| Frame::decode(bytes).expect("scanned frame decodes"))
        .collect();

    let mut assembler = Assembler::new(&decoded[0]);
    for frame in &decoded {
        assembler.add(frame).expect("same payload");
    }

    assert_eq!(assembler.finish().expect("complete"), blob);
}

#[test]
fn a_scan_that_misses_the_last_frame_reports_it() {
    // The realistic failure: a user stops the animation one frame early.
    let blob = signed_blob();
    let frames = airgap::split(&blob).expect("frames");
    let last = frames.len() - 1;

    let mut assembler = Assembler::new(&frames[0]);
    for frame in &frames[..last] {
        assembler.add(frame).expect("same payload");
    }

    assert!(!assembler.is_complete());
    assert_eq!(assembler.missing(), vec![last as u16]);
    assert!(assembler.finish().is_err());
}

#[test]
fn signing_is_deterministic_so_re_signing_is_safe_to_rescan() {
    // A property, not a coincidence. `crypto::keys` signs with
    // `try_sign_with_seed(&DETERMINISTIC_SEED, ..)`, and its own comment calls
    // that load-bearing for consensus: a txid hashes the signature, so a hedged
    // signature would give one transaction two identities.
    //
    // For an air-gapped flow the consequence is good news. A user who restarts
    // the offline signer and re-signs the same transfer gets byte-identical
    // frames, so a half-finished scan can be completed from the second run
    // rather than restarted.
    //
    // An earlier draft of this test asserted the opposite — that ML-DSA signing
    // is randomised — and failed. Worth recording, because "sign it again and
    // rescan" is exactly what a user does when a scan stalls.
    let first = signed_blob();
    let second = signed_blob();
    assert_eq!(
        first, second,
        "signing is deterministic; re-signing must reproduce the same bytes"
    );

    let first_frames = airgap::split(&first).expect("frames");
    let second_frames = airgap::split(&second).expect("frames");

    // Frames from the two runs interleave freely, because they are the same
    // frames.
    let mut assembler = Assembler::new(&first_frames[0]);
    for (index, frame) in first_frames.iter().enumerate() {
        let source = if index % 2 == 0 {
            frame
        } else {
            &second_frames[index]
        };
        assembler
            .add(source)
            .expect("identical payloads interleave");
    }
    assert_eq!(assembler.finish().expect("complete"), first);
}

#[test]
fn frames_from_two_different_transfers_cannot_be_mixed() {
    // The guard that matters. Two transfers differing only in nonce produce
    // different blobs, and mixing their frames would assemble a signature
    // matching neither — a transaction the user never approved.
    let seed = seed_from_mnemonic(PHRASE, "").expect("seed");
    let key = signing_key_at(seed.as_slice(), &DerivationPath::account(0, 0)).expect("derive");

    let first = hex::decode(
        &sign_transfer(
            &key,
            &hex::encode([9u8; 32]),
            primitive_types::U256::from(1_000),
            primitive_types::U256::from(0),
            10,
            &CHAIN,
        )
        .expect("sign")
        .raw_hex,
    )
    .expect("hex");
    let second = hex::decode(
        &sign_transfer(
            &key,
            &hex::encode([9u8; 32]),
            primitive_types::U256::from(1_000),
            primitive_types::U256::from(0),
            11,
            &CHAIN,
        )
        .expect("sign")
        .raw_hex,
    )
    .expect("hex");

    assert_ne!(
        first, second,
        "a different nonce is a different transaction"
    );

    let first_frames = airgap::split(&first).expect("frames");
    let second_frames = airgap::split(&second).expect("frames");

    let mut assembler = Assembler::new(&first_frames[0]);
    assembler.add(&first_frames[0]).expect("same payload");

    let error = assembler
        .add(&second_frames[1])
        .expect_err("a frame from another transfer must be refused");
    assert!(format!("{error}").contains("different payload"));
}
