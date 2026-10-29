//! Transaction payload wire format and canonicality.
//!
//! The signing bytes are what every signature in existence commits to, so this
//! encoding is consensus. These tests pin the properties that keep it safe.
//!
//! 1. A transfer's signing bytes are **exactly** what this file documents, so a
//!    change to the encoding fails here rather than in the field.
//! 2. No payload encoding can collide with any other, so one signature can
//!    never authorize two different actions.
//! 3. The single-signature wire versions are refused by name rather than
//!    misparsed.
//!
//! Point 1 used to read "byte-identical to the pre-payload format, so no
//! existing signature is invalidated". That guarantee is gone, deliberately,
//! and has now been given up twice: the move from ed25519 to ML-DSA-65 replaced
//! a 32-byte public key with a 1952-byte one inside the signed bytes, and the
//! move to hybrid signing appended a 32-byte SLH-DSA key beside it. Every
//! signature predating either fork is invalid by construction. What survives is
//! the *discipline* — the encoding is still frozen against a literal, so it
//! cannot drift silently from here.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::dex_payload::{
    AssetRegistration, AssetTransfer, LiquidityDeposit, LiquidityWithdrawal, MAX_ROUTE_LEGS,
    OrderPlacement, PoolCreation, ROUTE_LEG_SIZE, RouteLeg, SwapRequest, SwapRoute,
};
use custom_l1_node::core::governance_payload::{
    Ballot, MAX_CHANGES, ProposalSubmission, StakeLock, StakeUnlock, WorkClaim,
};
use custom_l1_node::core::oracle_payload::{
    BeaconSubmission, FeedCreation, FeedObservation, FeedSubmission, MAX_OBSERVATIONS,
    RegistryRotation,
};
use custom_l1_node::core::payload::{
    CLOSURE_SIZE, ChannelClosure, ChannelOpen, MAX_BATCH_CLOSURES, RevocationProof,
    channel_state_signing_bytes, derive_channel_id, revocation_commitment,
};
use custom_l1_node::core::{ChainTag, Transaction, TxKind, TxOutput};
use custom_l1_node::crypto::hybrid::{
    HybridPublicKey, HybridSignature, SLH_DSA_PUBLIC_KEY_LEN, SLH_DSA_SIGNATURE_LENGTH,
    generate_signing_key,
};
use custom_l1_node::crypto::keys::{
    PUBLIC_KEY_LEN as ML_DSA_PUBLIC_KEY_LEN, SIGNATURE_LENGTH as ML_DSA_SIGNATURE_LENGTH,
};
use primitive_types::U256;

mod common;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn closure(seq: u64, a: u64, b: u64) -> ChannelClosure {
    ChannelClosure {
        channel_id: [7u8; 32],
        seq,
        balance_a: primitive_types::U256::from(a),
        balance_b: primitive_types::U256::from(b),
        revocation_commitment: [9u8; 32],
        // Arbitrary bytes: these tests exercise the wire encoding, not the
        // signature schemes, so nothing here has to verify. Each half gets a
        // distinct filler so a round trip that swapped or truncated one of the
        // four sections shows up as a value mismatch rather than passing.
        pubkey_a: Box::new(filled_public_key(1)),
        pubkey_b: Box::new(filled_public_key(2)),
        sig_a: Box::new(filled_signature(3)),
        sig_b: Box::new(filled_signature(4)),
    }
}

/// A public key pair whose two halves are distinguishable in a hex dump.
fn filled_public_key(seed: u8) -> HybridPublicKey {
    HybridPublicKey {
        lattice: [seed; ML_DSA_PUBLIC_KEY_LEN],
        hash_based: [seed.wrapping_add(0x80); SLH_DSA_PUBLIC_KEY_LEN],
    }
}

/// A signature pair whose two halves are distinguishable in a hex dump.
fn filled_signature(seed: u8) -> HybridSignature {
    HybridSignature {
        lattice: [seed; ML_DSA_SIGNATURE_LENGTH],
        hash_based: [seed.wrapping_add(0x80); SLH_DSA_SIGNATURE_LENGTH],
    }
}

fn transfer_tx() -> Transaction {
    Transaction::new(
        vec![],
        vec![TxOutput {
            amount: primitive_types::U256::from(500),
            recipient: [3u8; 32],
        }],
        7,
    )
}

// ---------------------------------------------------------------------------
// backward compatibility
// ---------------------------------------------------------------------------

/// The prefix of a transfer's signing bytes: domain, the chain tag (ADR-036),
/// input count, output count, and the single output's amount and recipient.
///
/// Only the prefix is a literal. The 1984-byte public key pair that follows
/// would make a full literal 3968 hex characters, so the complete encoding is
/// pinned by [`TRANSFER_SIGNING_DIGEST`] instead.
const TRANSFER_SIGNING_PREFIX: &str = "\
637573746f6d2d6c312d6e6f64652e74782e7634\
2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\
0000000000000000\
0100000000000000\
f401000000000000000000000000000000000000000000000000000000000000\
0303030303030303030303030303030303030303030303030303030303030303";

/// BLAKE3 of an unsigned transfer's complete signing bytes.
///
/// Frozen rather than recomputed, so a change to the encoding fails here instead
/// of silently agreeing with itself. Any change to this value is a consensus
/// change: it invalidates every signature in existence.
///
/// Re-pinned 2026-10-13 for ADR-046: the output amount became 32 bytes. That
/// is the consensus change this constant exists to make deliberate; every
/// signature over the old 8-byte form is invalid on a re-genesised chain.
const TRANSFER_SIGNING_DIGEST: &str =
    "002988bfd60e9ec080eb541072d63117ea489dddddfe3c8939e3a1b96da90e5a";

/// Length of those signing bytes: domain, chain tag, two counts, one output,
/// both public keys, and the nonce.
const TRANSFER_SIGNING_LEN: usize =
    20 + 32 + 8 + 8 + 32 + 32 + ML_DSA_PUBLIC_KEY_LEN + SLH_DSA_PUBLIC_KEY_LEN + 8;

/// The genesis id the documented bytes are signed for: fixed here rather than
/// taken from a test helper, so the literal above cannot drift with one.
const DOCUMENTED_CHAIN: ChainTag = ChainTag::from_genesis([0x2a; 32]);

#[test]
fn a_transfer_signs_exactly_the_documented_bytes() {
    let tx = transfer_tx();
    let bytes = tx.signing_bytes(&DOCUMENTED_CHAIN);

    assert_eq!(bytes.len(), TRANSFER_SIGNING_LEN);
    assert!(
        hex::encode(&bytes).starts_with(TRANSFER_SIGNING_PREFIX),
        "the domain, the counts, or the output encoding changed"
    );
    assert_eq!(
        hex::encode(blake3::hash(&bytes).as_bytes()),
        TRANSFER_SIGNING_DIGEST,
        "transfer signing bytes changed — every existing signature is now invalid"
    );
}

#[test]
fn a_transfer_encodes_at_the_transfer_wire_version() {
    let tx = transfer_tx();
    let encoded = tx.to_bytes();

    // 5, not 3. Versions 3 and 4 carried one ML-DSA key and one signature
    // where this format carries a key pair and a signature pair, so they share
    // no decoder.
    assert_eq!(encoded[0], 5, "transfers use wire version 5");
    assert!(!tx.kind.has_payload());

    // Round trip preserves the kind.
    let decoded = Transaction::from_bytes(&encoded).expect("decode");
    assert_eq!(decoded, tx);
    assert_eq!(decoded.kind, TxKind::Transfer);
}

#[test]
fn a_payload_transaction_encodes_at_the_payload_wire_version() {
    let tx = Transaction::with_kind(TxKind::CooperativeClose(closure(3, 60, 40)), 1);
    let encoded = tx.to_bytes();

    assert_eq!(encoded[0], 6, "payload transactions use wire version 6");

    let decoded = Transaction::from_bytes(&encoded).expect("decode");
    assert_eq!(decoded, tx);
}

#[test]
fn the_single_signature_wire_versions_are_refused_by_name() {
    // Decoding a v1-v4 frame as if it were current would read fields out of
    // whatever followed the shorter key and signature it actually holds.
    // Refusing by version means the error explains itself.
    //
    // v3 and v4 are the interesting ones: unlike the ed25519 frames they carry
    // a perfectly good ML-DSA signature. It is refused anyway, because one
    // valid proof is not the rule.
    for version in 1u8..=4 {
        let mut encoded = transfer_tx().to_bytes();
        encoded[0] = version;

        let error = Transaction::from_bytes(&encoded)
            .expect_err("a single-signature frame must be refused");
        assert!(
            error.to_string().contains("predates hybrid signing"),
            "unhelpful error for version {version}: {error}"
        );
    }
}

#[test]
fn a_signed_transfer_still_verifies_across_a_round_trip() {
    let key = generate_signing_key().expect("keygen");
    let mut tx = transfer_tx();
    tx.sign(&key, &common::test_chain()).expect("sign");
    let decoded = Transaction::from_bytes(&tx.to_bytes()).expect("decode");
    assert_eq!(decoded.verify(&common::test_chain()), Ok(()));
    assert_eq!(decoded.txid(), tx.txid());
}

// ---------------------------------------------------------------------------
// canonicality: no two actions share signing bytes
// ---------------------------------------------------------------------------

#[test]
fn every_payload_kind_has_distinct_signing_bytes() {
    let kinds = vec![
        TxKind::Transfer,
        TxKind::OpenChannel(ChannelOpen {
            counterparty: [4u8; 32],
            funding: primitive_types::U256::from(1_000),
            dispute_window: 100,
        }),
        TxKind::CooperativeClose(closure(1, 50, 50)),
        // Same closure, different action: the tag must separate them.
        TxKind::DisputeClose(closure(1, 50, 50)),
        TxKind::PenaltyClaim(RevocationProof {
            channel_id: [7u8; 32],
            revoked_seq: 1,
            secret: [8u8; 32],
        }),
        TxKind::SettleBatch(vec![closure(1, 50, 50)]),
        // Trading. Several of these have identical field layouts to each other
        // — a deposit and a withdrawal are both a pair identifier and three
        // `u64`s — so the tag is the only thing separating them, and a
        // signature authorizing one must never authorize the other.
        TxKind::RegisterAsset(AssetRegistration {
            symbol: *b"AAA\0\0\0\0\0",
            total_supply: primitive_types::U256::from(1_000),
        }),
        TxKind::TransferAsset(AssetTransfer {
            asset: [1u8; 32],
            recipient: [2u8; 32],
            amount: primitive_types::U256::from(1_000),
        }),
        TxKind::CreatePool(PoolCreation {
            asset_a: [1u8; 32],
            asset_b: [2u8; 32],
            lp_fee_bps: 30,
            amount_a: primitive_types::U256::from(1_000),
            amount_b: primitive_types::U256::from(2_000),
        }),
        TxKind::AddLiquidity(LiquidityDeposit {
            pair: [3u8; 32],
            base_desired: primitive_types::U256::from(1_000),
            quote_desired: primitive_types::U256::from(2_000),
            min_shares: primitive_types::U256::from(1),
        }),
        TxKind::RemoveLiquidity(LiquidityWithdrawal {
            pair: [3u8; 32],
            shares: primitive_types::U256::from(1_000),
            min_base: primitive_types::U256::from(2_000),
            min_quote: primitive_types::U256::from(1),
        }),
        TxKind::Swap(SwapRequest {
            pair: [3u8; 32],
            direction: 0,
            amount_in: primitive_types::U256::from(1_000),
            min_out: primitive_types::U256::from(1),
            deadline: 0,
        }),
        // Same numbers, other direction: a signature for a buy must not
        // authorize a sell.
        TxKind::Swap(SwapRequest {
            pair: [3u8; 32],
            direction: 1,
            amount_in: primitive_types::U256::from(1_000),
            min_out: primitive_types::U256::from(1),
            deadline: 0,
        }),
        TxKind::SwapRoute(SwapRoute {
            legs: vec![RouteLeg {
                pair: [3u8; 32],
                direction: 0,
            }],
            amount_in: primitive_types::U256::from(1_000),
            min_out: primitive_types::U256::from(1),
            deadline: 0,
        }),
        TxKind::PlaceOrder(OrderPlacement {
            pair: [3u8; 32],
            side: 0,
            price: primitive_types::U256::from(1_000),
            amount: primitive_types::U256::from(1),
            expiry: 0,
        }),
        TxKind::CancelOrder([4u8; 32]),
        // Oracle. `CreateFeed` and `CancelOrder` both carry thirty-two opaque
        // bytes and nothing else, so the tag is all that separates a feed name
        // from an order identifier.
        TxKind::CreateFeed(FeedCreation {
            name: *b"MAYA/USD\0\0\0\0\0\0\0\0",
        }),
        TxKind::SubmitFeed(Box::new(FeedSubmission {
            feed_id: [5u8; 32],
            round: 1,
            observed_height: 2,
            observations: vec![FeedObservation {
                value: U256::from(100),
                public_key: Box::new(filled_public_key(5)),
                signature: Box::new(filled_signature(6)),
            }],
        })),
        TxKind::RotateAuthorities(Box::new(RegistryRotation {
            epoch: 1,
            authorities: vec![([6u8; 32], [7u8; 33])],
            quorum: 2,
            approvals: vec![(
                Box::new(filled_public_key(7)),
                Box::new(filled_signature(8)),
            )],
        })),
        TxKind::SubmitBeacon(BeaconSubmission {
            height: 9,
            proof: [10u8; 80],
        }),
        // Governance. `ClaimWork` and `CancelProposal` both carry thirty-two
        // opaque bytes and nothing else, so the tag is all that separates a
        // beneficiary address from a proposal identifier.
        TxKind::ClaimWork(WorkClaim {
            beneficiary: [11u8; 32],
        }),
        TxKind::LockStake(StakeLock {
            amount: primitive_types::U256::from(1_000),
            unlock_height: 5_000,
        }),
        TxKind::UnlockStake(StakeUnlock {
            amount: primitive_types::U256::from(1_000),
        }),
        TxKind::Propose(Box::new(ProposalSubmission {
            voting_blocks: 480,
            timelock_blocks: 960,
            changes: vec![(1, maya_dex::Amount::from_u64(25))],
        })),
        TxKind::CastVote(Ballot {
            proposal: [12u8; 32],
            choice: 1,
        }),
        // Same numbers, other choice: a signature for a yes must not authorize
        // a no.
        TxKind::CastVote(Ballot {
            proposal: [12u8; 32],
            choice: 2,
        }),
        TxKind::CancelProposal([13u8; 32]),
    ];

    let mut seen = std::collections::HashSet::new();
    for kind in kinds {
        let tx = Transaction::with_kind(kind.clone(), 0);
        assert!(
            seen.insert(tx.signing_bytes(&common::test_chain())),
            "{} shares signing bytes with another kind",
            kind.label()
        );
    }
}

#[test]
fn a_payload_can_never_collide_with_a_transfer() {
    let transfer = Transaction::new(vec![], vec![], 0);
    let with_payload = Transaction::with_kind(TxKind::CooperativeClose(closure(0, 0, 0)), 0);

    let plain = transfer.signing_bytes(&common::test_chain());
    let payload = with_payload.signing_bytes(&common::test_chain());

    // The payload section is appended after fixed-width fields, so a payload'd
    // transaction is strictly longer and shares the transfer as a prefix.
    assert!(payload.len() > plain.len());
    assert_eq!(&payload[..plain.len()], &plain[..]);
}

#[test]
fn changing_any_closure_field_changes_the_signing_bytes() {
    let base = Transaction::with_kind(TxKind::CooperativeClose(closure(1, 50, 50)), 0);

    let variants = [
        closure(2, 50, 50), // seq
        closure(1, 51, 50), // balance_a
        closure(1, 50, 51), // balance_b
    ];

    for variant in variants {
        let other = Transaction::with_kind(TxKind::CooperativeClose(variant), 0);
        assert_ne!(
            base.signing_bytes(&common::test_chain()),
            other.signing_bytes(&common::test_chain())
        );
    }
}

// ---------------------------------------------------------------------------
// decoder hardening
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_payload_tag_is_rejected() {
    let mut encoded =
        Transaction::with_kind(TxKind::CooperativeClose(closure(1, 50, 50)), 0).to_bytes();

    // The payload tag sits immediately after the signature-presence byte.
    let tag_index = encoded.len() - 1 - CLOSURE_SIZE;
    encoded[tag_index] = 99;

    assert!(Transaction::from_bytes(&encoded).is_err());
}

#[test]
fn a_version_two_frame_without_a_payload_is_rejected() {
    // Would otherwise be a second encoding of a plain transfer.
    let mut encoded = transfer_tx().to_bytes();
    encoded[0] = 2;
    assert!(Transaction::from_bytes(&encoded).is_err());
}

#[test]
fn a_truncated_payload_is_rejected() {
    let encoded =
        Transaction::with_kind(TxKind::CooperativeClose(closure(1, 50, 50)), 0).to_bytes();

    for cut in 1..=CLOSURE_SIZE.min(40) {
        let truncated = &encoded[..encoded.len() - cut];
        assert!(
            Transaction::from_bytes(truncated).is_err(),
            "a frame short by {cut} bytes must not decode"
        );
    }
}

#[test]
fn an_oversized_batch_count_is_rejected() {
    let mut encoded =
        Transaction::with_kind(TxKind::SettleBatch(vec![closure(1, 50, 50)]), 0).to_bytes();

    // Overwrite the batch length with something implausible. The decoder must
    // refuse before attempting an allocation of that size.
    let count_index = encoded.len() - CLOSURE_SIZE - 8;
    encoded[count_index..count_index + 8]
        .copy_from_slice(&(MAX_BATCH_CLOSURES as u64 + 1).to_le_bytes());

    assert!(Transaction::from_bytes(&encoded).is_err());
}

#[test]
fn a_maximum_batch_round_trips() {
    // At the limit, not below it: the interesting failure is an off-by-one that
    // makes the largest legal batch undecodable.
    //
    // The limit fell from 4096 to 128 with ML-DSA. A closure grew from 216 bytes
    // to about 10.5 KB, so the old ceiling described a 43 MB transaction no
    // gossip layer would carry, and each closure now costs two ~200 µs
    // verifications rather than two ~56 µs ones.
    let closures: Vec<ChannelClosure> = (0..MAX_BATCH_CLOSURES as u64)
        .map(|i| closure(i, 1_000 - i, i))
        .collect();
    let tx = Transaction::with_kind(TxKind::SettleBatch(closures.clone()), 0);

    let encoded = tx.to_bytes();
    assert!(
        encoded.len() > MAX_BATCH_CLOSURES * CLOSURE_SIZE,
        "a full batch should be dominated by its closures"
    );

    let decoded = Transaction::from_bytes(&encoded).expect("decode");
    assert_eq!(decoded.kind, TxKind::SettleBatch(closures));
}

#[test]
fn a_maximum_batch_survives_being_carried_in_a_block() {
    // Audit 2026-10-26: a block read each transaction frame through the
    // 65,536-element collection cap, so a legal full batch (about 1.3 MB) or
    // 100 KB of contract code decoded alone and then made a block no peer could
    // read. Mempool admission has no size check, so a miner could build one.
    use custom_l1_node::core::{Block, BlockHeader};
    use custom_l1_node::crypto::pow::target_from_leading_zero_bits;

    let closures: Vec<ChannelClosure> = (0..MAX_BATCH_CLOSURES as u64)
        .map(|i| closure(i, 1_000 - i, i))
        .collect();
    let tx = Transaction::with_kind(TxKind::SettleBatch(closures), 0);
    assert!(
        tx.to_bytes().len() > 65_536,
        "the case must exceed the old cap"
    );

    let block = Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_789_200_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        vec![tx],
    );
    let decoded = Block::from_bytes(&block.to_bytes()).expect("a legal block must decode");
    assert_eq!(decoded.transactions.len(), 1);
}

#[test]
fn contract_code_up_to_its_declared_maximum_decodes() {
    use custom_l1_node::core::payload::{ContractDeploy, MAX_CONTRACT_CODE};

    for size in [100_000, MAX_CONTRACT_CODE] {
        let tx = Transaction::with_kind(
            TxKind::DeployContract(ContractDeploy {
                code: vec![0xab; size],
            }),
            0,
        );
        let back = Transaction::from_bytes(&tx.to_bytes())
            .unwrap_or_else(|e| panic!("{size} bytes of code: {e}"));
        assert_eq!(back.kind, tx.kind);
    }
    let over = Transaction::with_kind(
        TxKind::DeployContract(ContractDeploy {
            code: vec![0; MAX_CONTRACT_CODE + 1],
        }),
        0,
    );
    assert!(Transaction::from_bytes(&over.to_bytes()).is_err());
}

// ---------------------------------------------------------------------------
// trading payloads
// ---------------------------------------------------------------------------

#[test]
fn every_trading_kind_survives_a_round_trip_unchanged() {
    // Each of these is fixed-width apart from the route, so the failure this
    // catches is a field written in one order and read in another — which does
    // not fail to decode, it decodes to different numbers.
    let kinds = vec![
        TxKind::RegisterAsset(AssetRegistration {
            symbol: *b"MAYA2C\0\0",
            total_supply: primitive_types::U256::from(u64::MAX),
        }),
        TxKind::TransferAsset(AssetTransfer {
            asset: [0xAB; 32],
            recipient: [0xCD; 32],
            amount: primitive_types::U256::from(1),
        }),
        TxKind::CreatePool(PoolCreation {
            asset_a: [1u8; 32],
            asset_b: [2u8; 32],
            lp_fee_bps: 9_999,
            amount_a: primitive_types::U256::from(12_345),
            amount_b: primitive_types::U256::from(67_890),
        }),
        TxKind::AddLiquidity(LiquidityDeposit {
            pair: [3u8; 32],
            base_desired: primitive_types::U256::from(1),
            quote_desired: primitive_types::U256::from(2),
            min_shares: primitive_types::U256::from(3),
        }),
        TxKind::RemoveLiquidity(LiquidityWithdrawal {
            pair: [3u8; 32],
            shares: primitive_types::U256::from(4),
            min_base: primitive_types::U256::from(5),
            min_quote: primitive_types::U256::from(6),
        }),
        TxKind::Swap(SwapRequest {
            pair: [3u8; 32],
            direction: 1,
            amount_in: primitive_types::U256::from(7),
            min_out: primitive_types::U256::from(8),
            deadline: 9,
        }),
        TxKind::SwapRoute(SwapRoute {
            legs: (0..MAX_ROUTE_LEGS)
                .map(|index| RouteLeg {
                    pair: [index as u8; 32],
                    direction: (index % 2) as u8,
                })
                .collect(),
            amount_in: primitive_types::U256::from(10),
            min_out: primitive_types::U256::from(11),
            deadline: 12,
        }),
        TxKind::PlaceOrder(OrderPlacement {
            pair: [3u8; 32],
            side: 1,
            price: primitive_types::U256::from(13),
            amount: primitive_types::U256::from(14),
            expiry: 15,
        }),
        TxKind::CancelOrder([0xEF; 32]),
    ];

    for kind in kinds {
        let tx = Transaction::with_kind(kind.clone(), 42);
        let decoded = Transaction::from_bytes(&tx.to_bytes()).expect("decode");
        assert_eq!(decoded.kind, kind, "{} did not round trip", kind.label());
        assert_eq!(decoded.nonce, 42);
    }
}

#[test]
fn a_route_with_no_legs_is_rejected() {
    // Not merely useless: a zero-leg route has no input asset, so every later
    // stage would be reasoning about an asset nobody named.
    let mut encoded = Transaction::with_kind(
        TxKind::SwapRoute(SwapRoute {
            legs: vec![RouteLeg {
                pair: [3u8; 32],
                direction: 0,
            }],
            amount_in: primitive_types::U256::from(1),
            min_out: primitive_types::U256::from(0),
            deadline: 0,
        }),
        0,
    )
    .to_bytes();

    // The leg count sits before one leg, two 256-bit amounts and a `u64` deadline.
    let count_index = encoded.len() - ROUTE_LEG_SIZE - (32 + 32 + 8) - 8;
    encoded[count_index..count_index + 8].copy_from_slice(&0u64.to_le_bytes());

    assert!(Transaction::from_bytes(&encoded).is_err());
}

#[test]
fn a_route_longer_than_the_ceiling_is_rejected_before_it_is_allocated() {
    let mut encoded = Transaction::with_kind(
        TxKind::SwapRoute(SwapRoute {
            legs: vec![RouteLeg {
                pair: [3u8; 32],
                direction: 0,
            }],
            amount_in: primitive_types::U256::from(1),
            min_out: primitive_types::U256::from(0),
            deadline: 0,
        }),
        0,
    )
    .to_bytes();

    let count_index = encoded.len() - ROUTE_LEG_SIZE - (32 + 32 + 8) - 8;
    encoded[count_index..count_index + 8]
        .copy_from_slice(&(MAX_ROUTE_LEGS as u64 + 1).to_le_bytes());

    assert!(Transaction::from_bytes(&encoded).is_err());
}

#[test]
fn a_trading_payload_cannot_collide_with_a_transfer() {
    // The same property the channel payloads have, restated for the kinds added
    // since. A payload section is appended after fixed-width fields, so a
    // payload'd transaction is strictly longer and shares the transfer as a
    // prefix — which is what makes one signature unable to mean two things.
    let transfer = Transaction::new(vec![], vec![], 0).signing_bytes(&common::test_chain());

    for kind in [
        TxKind::Swap(SwapRequest {
            pair: [0u8; 32],
            direction: 0,
            amount_in: primitive_types::U256::from(0),
            min_out: primitive_types::U256::from(0),
            deadline: 0,
        }),
        TxKind::CancelOrder([0u8; 32]),
    ] {
        let payload = Transaction::with_kind(kind, 0).signing_bytes(&common::test_chain());
        assert!(payload.len() > transfer.len());
        assert_eq!(&payload[..transfer.len()], &transfer[..]);
    }
}

// ---------------------------------------------------------------------------
// oracle payloads
// ---------------------------------------------------------------------------

#[test]
fn every_oracle_kind_survives_a_round_trip_unchanged() {
    let kinds = vec![
        TxKind::CreateFeed(FeedCreation {
            name: *b"BTC/USD\0\0\0\0\0\0\0\0\0",
        }),
        TxKind::SubmitFeed(Box::new(FeedSubmission {
            feed_id: [0xAB; 32],
            round: u64::MAX,
            observed_height: 12_345,
            observations: (0..3u8)
                .map(|index| FeedObservation {
                    value: U256::from(1_000 + u64::from(index)),
                    public_key: Box::new(filled_public_key(index)),
                    signature: Box::new(filled_signature(index.wrapping_add(64))),
                })
                .collect(),
        })),
        TxKind::RotateAuthorities(Box::new(RegistryRotation {
            epoch: 7,
            authorities: (0..3u8).map(|index| ([index; 32], [index; 33])).collect(),
            quorum: 2,
            approvals: (0..2u8)
                .map(|index| {
                    (
                        Box::new(filled_public_key(index)),
                        Box::new(filled_signature(index.wrapping_add(32))),
                    )
                })
                .collect(),
        })),
        TxKind::SubmitBeacon(BeaconSubmission {
            height: u64::MAX,
            proof: [0xCD; 80],
        }),
    ];

    for kind in kinds {
        let tx = Transaction::with_kind(kind.clone(), 11);
        let decoded = Transaction::from_bytes(&tx.to_bytes()).expect("decode");
        assert_eq!(decoded.kind, kind, "{} did not round trip", kind.label());
        assert_eq!(decoded.nonce, 11);
    }
}

#[test]
fn a_feed_submission_with_no_observations_is_rejected() {
    // Not merely useless: a submission with nothing in it would reach the
    // median with an empty set, and every later stage would be reasoning about
    // a value nobody attested.
    let mut encoded = Transaction::with_kind(
        TxKind::SubmitFeed(Box::new(FeedSubmission {
            feed_id: [1u8; 32],
            round: 1,
            observed_height: 1,
            observations: vec![FeedObservation {
                value: U256::from(1),
                public_key: Box::new(filled_public_key(1)),
                signature: Box::new(filled_signature(2)),
            }],
        })),
        0,
    )
    .to_bytes();

    // The observation count sits directly before the single observation.
    let observation_size = 32
        + ML_DSA_PUBLIC_KEY_LEN
        + SLH_DSA_PUBLIC_KEY_LEN
        + ML_DSA_SIGNATURE_LENGTH
        + SLH_DSA_SIGNATURE_LENGTH;
    let count_index = encoded.len() - observation_size - 8;
    encoded[count_index..count_index + 8].copy_from_slice(&0u64.to_le_bytes());

    assert!(Transaction::from_bytes(&encoded).is_err());
}

#[test]
fn a_feed_submission_above_the_ceiling_is_rejected_before_it_is_allocated() {
    // Each observation costs a post-quantum signature verification, so an
    // unbounded count is a way to make every node on the network do unbounded
    // work for one transaction.
    let mut encoded = Transaction::with_kind(
        TxKind::SubmitFeed(Box::new(FeedSubmission {
            feed_id: [1u8; 32],
            round: 1,
            observed_height: 1,
            observations: vec![FeedObservation {
                value: U256::from(1),
                public_key: Box::new(filled_public_key(1)),
                signature: Box::new(filled_signature(2)),
            }],
        })),
        0,
    )
    .to_bytes();

    let observation_size = 32
        + ML_DSA_PUBLIC_KEY_LEN
        + SLH_DSA_PUBLIC_KEY_LEN
        + ML_DSA_SIGNATURE_LENGTH
        + SLH_DSA_SIGNATURE_LENGTH;
    let count_index = encoded.len() - observation_size - 8;
    encoded[count_index..count_index + 8]
        .copy_from_slice(&(MAX_OBSERVATIONS as u64 + 1).to_le_bytes());

    assert!(Transaction::from_bytes(&encoded).is_err());
}

#[test]
fn a_beacon_proof_is_exactly_eighty_bytes_on_the_wire() {
    // Pinned as a literal: a proof that changed size would change every
    // transaction carrying one, and the RFC fixes it at eighty.
    let plain = Transaction::with_kind(TxKind::Transfer, 0).to_bytes();
    let beacon = Transaction::with_kind(
        TxKind::SubmitBeacon(BeaconSubmission {
            height: 1,
            proof: [0u8; 80],
        }),
        0,
    )
    .to_bytes();

    // Tag, height, proof — and a transfer carries no payload section at all.
    assert_eq!(beacon.len(), plain.len() + 1 + 8 + 80);
}

// ---------------------------------------------------------------------------
// governance payloads
// ---------------------------------------------------------------------------

#[test]
fn every_governance_kind_survives_a_round_trip_unchanged() {
    let kinds = vec![
        TxKind::ClaimWork(WorkClaim {
            beneficiary: [0xAB; 32],
        }),
        TxKind::LockStake(StakeLock {
            amount: primitive_types::U256::from(u64::MAX),
            unlock_height: u64::MAX,
        }),
        TxKind::UnlockStake(StakeUnlock {
            amount: primitive_types::U256::from(1),
        }),
        TxKind::Propose(Box::new(ProposalSubmission {
            voting_blocks: 500,
            timelock_blocks: 1_000,
            changes: (1..=MAX_CHANGES as u16)
                .map(|tag| (tag, maya_dex::Amount::from_u64(u64::from(tag))))
                .collect(),
        })),
        TxKind::CastVote(Ballot {
            proposal: [0xCD; 32],
            choice: 3,
        }),
        TxKind::CancelProposal([0xEF; 32]),
    ];

    for kind in kinds {
        let tx = Transaction::with_kind(kind.clone(), 5);
        let decoded = Transaction::from_bytes(&tx.to_bytes()).expect("decode");
        assert_eq!(decoded.kind, kind, "{} did not round trip", kind.label());
        assert_eq!(decoded.nonce, 5);
    }
}

#[test]
fn a_proposal_with_no_changes_is_rejected() {
    // A proposal that changes nothing is a vote about nothing, and it would
    // still consume a deposit, a voting window, and everyone's attention.
    let mut encoded = Transaction::with_kind(
        TxKind::Propose(Box::new(ProposalSubmission {
            voting_blocks: 500,
            timelock_blocks: 1_000,
            changes: vec![(1, maya_dex::Amount::from_u64(25))],
        })),
        0,
    )
    .to_bytes();

    // The change count sits before one 34-byte change (2-byte tag, 32-byte value).
    let count_index = encoded.len() - 34 - 8;
    encoded[count_index..count_index + 8].copy_from_slice(&0u64.to_le_bytes());
    assert!(Transaction::from_bytes(&encoded).is_err());
}

#[test]
fn a_proposal_above_the_bundling_ceiling_is_rejected() {
    // A proposal is a single decision that voters accept or reject as a whole,
    // so bundling is a way to carry an unpopular change on the back of a
    // popular one. A bound is the difference between that and an omnibus.
    let mut encoded = Transaction::with_kind(
        TxKind::Propose(Box::new(ProposalSubmission {
            voting_blocks: 500,
            timelock_blocks: 1_000,
            changes: vec![(1, maya_dex::Amount::from_u64(25))],
        })),
        0,
    )
    .to_bytes();

    let count_index = encoded.len() - 34 - 8;
    encoded[count_index..count_index + 8].copy_from_slice(&(MAX_CHANGES as u64 + 1).to_le_bytes());
    assert!(Transaction::from_bytes(&encoded).is_err());
}

// ---------------------------------------------------------------------------
// channel identifiers and commitments
// ---------------------------------------------------------------------------

#[test]
fn channel_ids_are_deterministic_and_parameter_sensitive() {
    let a = [1u8; 32];
    let b = [2u8; 32];

    assert_eq!(
        derive_channel_id(&a, &b, primitive_types::U256::from(1_000), 0),
        derive_channel_id(&a, &b, primitive_types::U256::from(1_000), 0)
    );

    // The nonce is what lets one pair hold many channels without collision.
    assert_ne!(
        derive_channel_id(&a, &b, primitive_types::U256::from(1_000), 0),
        derive_channel_id(&a, &b, primitive_types::U256::from(1_000), 1)
    );
    assert_ne!(
        derive_channel_id(&a, &b, primitive_types::U256::from(1_000), 0),
        derive_channel_id(&a, &b, primitive_types::U256::from(2_000), 0)
    );
    // Ordering matters: A->B is not the same channel as B->A.
    assert_ne!(
        derive_channel_id(&a, &b, primitive_types::U256::from(1_000), 0),
        derive_channel_id(&b, &a, primitive_types::U256::from(1_000), 0)
    );
}

#[test]
fn channel_state_bytes_are_domain_separated_from_transactions() {
    let bytes = channel_state_signing_bytes(
        &[9u8; 32],
        4,
        primitive_types::U256::from(10),
        primitive_types::U256::from(20),
        &[3u8; 32],
    );

    // A channel state must never be reinterpretable as a transaction payload.
    assert!(bytes.starts_with(b"maya-flash.channel-state.v1"));
    assert!(!bytes.starts_with(b"custom-l1-node.tx.v1"));
}

#[test]
fn revocation_commitments_are_binding() {
    let secret = [5u8; 32];
    let commitment = revocation_commitment(&secret);

    assert_eq!(commitment, revocation_commitment(&secret));

    let mut other = secret;
    other[0] ^= 1;
    assert_ne!(commitment, revocation_commitment(&other));
    // The commitment must not simply be the secret.
    assert_ne!(commitment, secret);
}
