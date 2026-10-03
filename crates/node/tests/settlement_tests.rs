//! On-chain settlement, dispute handling, and penalties.
//!
//! The adversarial cases here are the point. A channel is an escrow that two
//! parties can drain by presenting signed paper, so the questions that matter
//! are: can someone present paper they were not given, paper that was already
//! superseded, or paper that adds up to more than was escrowed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::payload::{
    ChannelClosure, ChannelId, ChannelOpen, RevocationProof, channel_state_signing_bytes,
    derive_channel_id, revocation_commitment,
};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::generate_signing_key;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::{Account, Address, BlockContext, ChannelStatus, StateDB};

use custom_l1_node::crypto::hybrid::HybridSigningKey;
use tempfile::TempDir;

mod common;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

struct Fixture {
    db: StateDB,
    _dir: TempDir,
}

fn fixture(funded: &[(Address, u64)]) -> Fixture {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    common::bind(&db);
    for (address, balance) in funded {
        db.put_account(
            address,
            &Account {
                balance: *balance,
                nonce: 0,
            },
        )
        .expect("fund");
    }
    Fixture { db, _dir: dir }
}

fn address_of(key: &HybridSigningKey) -> Address {
    key.address()
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn signed_tx(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key, &common::test_chain()).expect("sign");
    tx
}

/// Builds a closure signed by both participants.
fn signed_closure(
    channel_id: ChannelId,
    seq: u64,
    balance_a: u64,
    balance_b: u64,
    commitment: [u8; 32],
    key_a: &HybridSigningKey,
    key_b: &HybridSigningKey,
) -> ChannelClosure {
    let message = channel_state_signing_bytes(&channel_id, seq, balance_a, balance_b, &commitment);
    ChannelClosure {
        channel_id,
        seq,
        balance_a,
        balance_b,
        revocation_commitment: commitment,
        // The chain binds these to the recorded participants by hashing them,
        // so a closure must now carry the keys it is signed by.
        pubkey_a: Box::new(key_a.public_key()),
        pubkey_b: Box::new(key_b.public_key()),
        sig_a: Box::new(key_a.sign(&message).expect("sign")),
        sig_b: Box::new(key_b.sign(&message).expect("sign")),
    }
}

/// Opens a channel funded by `key_a`, returning its identifier.
fn open_channel(
    fixture: &Fixture,
    key_a: &HybridSigningKey,
    party_b: Address,
    funding: u64,
    dispute_window: u64,
    nonce: u64,
) -> ChannelId {
    let tx = signed_tx(
        TxKind::OpenChannel(ChannelOpen {
            counterparty: party_b,
            funding,
            dispute_window,
        }),
        nonce,
        key_a,
    );
    fixture
        .db
        .apply_block(&block_of(vec![tx]), BlockContext::at_height(1))
        .expect("open channel");
    assert_eq!(
        fixture.db.uncovered_keys().expect("scan"),
        Vec::<Vec<u8>>::new(),
        "every stored key must be under the state root or declared local-only"
    );

    derive_channel_id(&address_of(key_a), &party_b, funding, nonce)
}

// ---------------------------------------------------------------------------
// opening
// ---------------------------------------------------------------------------

#[test]
fn opening_a_channel_escrows_the_funding() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let f = fixture(&[(alice_addr, 10_000)]);

    let id = open_channel(&f, &alice, address_of(&bob), 4_000, 50, 0);

    // The funds have left Alice's account but belong to nobody yet.
    assert_eq!(f.db.get_account(&alice_addr).map(|a| a.balance), Ok(6_000));

    let record = f.db.get_channel(&id).expect("read").expect("exists");
    assert_eq!(record.capacity, 4_000);
    assert_eq!(record.status, ChannelStatus::Open);
    assert_eq!(record.party_a, alice_addr);
    assert_eq!(record.party_b, address_of(&bob));
}

#[test]
fn opening_a_channel_beyond_the_balance_is_rejected() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let f = fixture(&[(address_of(&alice), 100)]);

    let tx = signed_tx(
        TxKind::OpenChannel(ChannelOpen {
            counterparty: address_of(&bob),
            funding: 5_000,
            dispute_window: 50,
        }),
        0,
        &alice,
    );

    assert!(matches!(
        f.db.apply_block(&block_of(vec![tx]), BlockContext::at_height(1)),
        Err(NodeError::InsufficientBalance { .. })
    ));
}

#[test]
fn a_channel_operation_may_not_also_carry_transfer_outputs() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let f = fixture(&[(address_of(&alice), 10_000)]);

    let mut tx = Transaction::with_kind(
        TxKind::OpenChannel(ChannelOpen {
            counterparty: address_of(&bob),
            funding: 1_000,
            dispute_window: 50,
        }),
        0,
    );
    // Two value-moving mechanisms in one transaction.
    tx.outputs.push(custom_l1_node::core::TxOutput {
        amount: 500,
        recipient: [9u8; 32],
    });
    tx.sign(&alice, &common::test_chain()).expect("sign");
    assert!(matches!(
        f.db.apply_block(&block_of(vec![tx]), BlockContext::at_height(1)),
        Err(NodeError::MixedTransactionKind(_))
    ));
}

// ---------------------------------------------------------------------------
// cooperative close
// ---------------------------------------------------------------------------

#[test]
fn a_cooperative_close_pays_both_parties() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let (alice_addr, bob_addr) = (address_of(&alice), address_of(&bob));
    let f = fixture(&[(alice_addr, 10_000)]);

    let id = open_channel(&f, &alice, bob_addr, 4_000, 50, 0);
    let closure = signed_closure(id, 7, 1_500, 2_500, [1u8; 32], &alice, &bob);

    f.db.apply_block(
        &block_of(vec![signed_tx(
            TxKind::CooperativeClose(closure),
            1,
            &alice,
        )]),
        BlockContext::at_height(2),
    )
    .expect("close");

    assert_eq!(f.db.get_account(&alice_addr).map(|a| a.balance), Ok(7_500));
    assert_eq!(f.db.get_account(&bob_addr).map(|a| a.balance), Ok(2_500));
    assert_eq!(
        f.db.get_channel(&id).expect("read").expect("exists").status,
        ChannelStatus::Closed
    );
}

#[test]
fn a_closure_that_does_not_conserve_capacity_is_rejected() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let f = fixture(&[(address_of(&alice), 10_000)]);

    let id = open_channel(&f, &alice, address_of(&bob), 4_000, 50, 0);
    // Both parties happily sign themselves richer than the escrow.
    let closure = signed_closure(id, 1, 5_000, 5_000, [1u8; 32], &alice, &bob);

    assert!(matches!(
        f.db.apply_block(
            &block_of(vec![signed_tx(
                TxKind::CooperativeClose(closure),
                1,
                &alice
            )]),
            BlockContext::at_height(2)
        ),
        Err(NodeError::ChannelCapacityMismatch { .. })
    ));
}

#[test]
fn a_closure_missing_a_participant_signature_is_rejected() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let impostor = generate_signing_key().expect("keygen");
    let f = fixture(&[(address_of(&alice), 10_000)]);

    let id = open_channel(&f, &alice, address_of(&bob), 4_000, 50, 0);
    // Alice and a stranger sign; Bob never agreed to this split.
    let closure = signed_closure(id, 1, 4_000, 0, [1u8; 32], &alice, &impostor);

    assert!(matches!(
        f.db.apply_block(
            &block_of(vec![signed_tx(
                TxKind::CooperativeClose(closure),
                1,
                &alice
            )]),
            BlockContext::at_height(2)
        ),
        Err(NodeError::ClosureSignature { party: "b", .. })
    ));
}

#[test]
fn a_third_party_may_submit_but_not_alter_a_closure() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let carol = generate_signing_key().expect("keygen");
    let (alice_addr, bob_addr) = (address_of(&alice), address_of(&bob));
    let f = fixture(&[(alice_addr, 10_000), (address_of(&carol), 10)]);

    let id = open_channel(&f, &alice, bob_addr, 4_000, 50, 0);
    let closure = signed_closure(id, 1, 1_000, 3_000, [1u8; 32], &alice, &bob);

    // Carol pays for inclusion. This must work — it is the basis of batching.
    f.db.apply_block(
        &block_of(vec![signed_tx(
            TxKind::CooperativeClose(closure.clone()),
            0,
            &carol,
        )]),
        BlockContext::at_height(2),
    )
    .expect("third-party submission is allowed");

    assert_eq!(f.db.get_account(&bob_addr).map(|a| a.balance), Ok(3_000));

    // But tampering with the balances invalidates the inner signatures.
    let f2 = fixture(&[(alice_addr, 10_000), (address_of(&carol), 10)]);
    let id2 = open_channel(&f2, &alice, bob_addr, 4_000, 50, 0);
    let mut tampered = signed_closure(id2, 1, 1_000, 3_000, [1u8; 32], &alice, &bob);
    tampered.balance_a = 4_000;
    tampered.balance_b = 0;

    assert!(matches!(
        f2.db.apply_block(
            &block_of(vec![signed_tx(
                TxKind::CooperativeClose(tampered),
                0,
                &carol
            )]),
            BlockContext::at_height(2)
        ),
        Err(NodeError::ClosureSignature { .. })
    ));
}

// ---------------------------------------------------------------------------
// dispute and penalty
// ---------------------------------------------------------------------------

#[test]
fn a_dispute_close_does_not_pay_out_immediately() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let (alice_addr, bob_addr) = (address_of(&alice), address_of(&bob));
    let f = fixture(&[(alice_addr, 10_000)]);

    let id = open_channel(&f, &alice, bob_addr, 4_000, 50, 0);
    let closure = signed_closure(id, 3, 4_000, 0, [1u8; 32], &alice, &bob);

    f.db.apply_block(
        &block_of(vec![signed_tx(TxKind::DisputeClose(closure), 1, &alice)]),
        BlockContext::at_height(10),
    )
    .expect("dispute");

    // Paying out now would make fraud costless.
    assert_eq!(f.db.get_account(&alice_addr).map(|a| a.balance), Ok(6_000));
    assert_eq!(f.db.get_account(&bob_addr).map(|a| a.balance), Ok(0));

    let record = f.db.get_channel(&id).expect("read").expect("exists");
    assert_eq!(record.status, ChannelStatus::Disputed);
    assert_eq!(record.dispute_deadline, 60);
    assert_eq!(record.dispute_closer, alice_addr);
}

#[test]
fn publishing_a_revoked_state_forfeits_the_whole_channel() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let (alice_addr, bob_addr) = (address_of(&alice), address_of(&bob));
    let f = fixture(&[(alice_addr, 10_000)]);

    let id = open_channel(&f, &alice, bob_addr, 4_000, 50, 0);

    // Alice publishes an old state in which she still held everything. Both
    // parties did sign it once — that is precisely why sequence numbers alone
    // are not enough and revocation is required.
    let secret = [42u8; 32];
    let stale = signed_closure(
        id,
        2,
        4_000,
        0,
        revocation_commitment(&secret),
        &alice,
        &bob,
    );
    f.db.apply_block(
        &block_of(vec![signed_tx(TxKind::DisputeClose(stale), 1, &alice)]),
        BlockContext::at_height(10),
    )
    .expect("dispute");

    // Bob holds the revocation secret because Alice handed it over when they
    // advanced past that state.
    f.db.apply_block(
        &block_of(vec![signed_tx(
            TxKind::PenaltyClaim(RevocationProof {
                channel_id: id,
                revoked_seq: 2,
                secret,
            }),
            0,
            &bob,
        )]),
        BlockContext::at_height(12),
    )
    .expect("penalty");

    // Everything goes to the victim; the cheater gets nothing back.
    assert_eq!(f.db.get_account(&bob_addr).map(|a| a.balance), Ok(4_000));
    assert_eq!(f.db.get_account(&alice_addr).map(|a| a.balance), Ok(6_000));
    assert_eq!(
        f.db.get_channel(&id).expect("read").expect("exists").status,
        ChannelStatus::Closed
    );
}

#[test]
fn the_closing_party_cannot_claim_its_own_penalty() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let f = fixture(&[(address_of(&alice), 10_000)]);

    let id = open_channel(&f, &alice, address_of(&bob), 4_000, 50, 0);
    let secret = [42u8; 32];
    let stale = signed_closure(
        id,
        2,
        2_000,
        2_000,
        revocation_commitment(&secret),
        &alice,
        &bob,
    );

    f.db.apply_block(
        &block_of(vec![signed_tx(TxKind::DisputeClose(stale), 1, &alice)]),
        BlockContext::at_height(10),
    )
    .expect("dispute");

    // Alice knows the secret too. Letting her "catch" herself would be a
    // one-transaction drain of Bob's side.
    assert!(matches!(
        f.db.apply_block(
            &block_of(vec![signed_tx(
                TxKind::PenaltyClaim(RevocationProof {
                    channel_id: id,
                    revoked_seq: 2,
                    secret,
                }),
                2,
                &alice,
            )]),
            BlockContext::at_height(12)
        ),
        Err(NodeError::PenaltyByCloser { .. })
    ));
}

#[test]
fn a_wrong_revocation_secret_is_rejected() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let f = fixture(&[(address_of(&alice), 10_000)]);

    let id = open_channel(&f, &alice, address_of(&bob), 4_000, 50, 0);
    let stale = signed_closure(
        id,
        2,
        4_000,
        0,
        revocation_commitment(&[42u8; 32]),
        &alice,
        &bob,
    );
    f.db.apply_block(
        &block_of(vec![signed_tx(TxKind::DisputeClose(stale), 1, &alice)]),
        BlockContext::at_height(10),
    )
    .expect("dispute");

    // Guessing does not work: the commitment is binding.
    assert!(matches!(
        f.db.apply_block(
            &block_of(vec![signed_tx(
                TxKind::PenaltyClaim(RevocationProof {
                    channel_id: id,
                    revoked_seq: 2,
                    secret: [7u8; 32],
                }),
                0,
                &bob,
            )]),
            BlockContext::at_height(12)
        ),
        Err(NodeError::InvalidRevocationProof { .. })
    ));
}

#[test]
fn a_dispute_cannot_be_finalized_before_its_window_closes() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let f = fixture(&[(address_of(&alice), 10_000)]);

    let id = open_channel(&f, &alice, address_of(&bob), 4_000, 50, 0);
    let closure = signed_closure(id, 2, 4_000, 0, [1u8; 32], &alice, &bob);
    f.db.apply_block(
        &block_of(vec![signed_tx(TxKind::DisputeClose(closure), 1, &alice)]),
        BlockContext::at_height(10),
    )
    .expect("dispute");

    // Finalizing early would close the very window the penalty depends on.
    assert!(matches!(
        f.db.apply_block(
            &block_of(vec![signed_tx(TxKind::FinalizeDispute(id), 2, &alice)]),
            BlockContext::at_height(59)
        ),
        Err(NodeError::DisputeWindowOpen { .. })
    ));
}

#[test]
fn an_unchallenged_dispute_finalizes_at_its_claimed_balances() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let (alice_addr, bob_addr) = (address_of(&alice), address_of(&bob));
    let f = fixture(&[(alice_addr, 10_000)]);

    let id = open_channel(&f, &alice, bob_addr, 4_000, 50, 0);
    let closure = signed_closure(id, 2, 2_500, 1_500, [1u8; 32], &alice, &bob);
    f.db.apply_block(
        &block_of(vec![signed_tx(TxKind::DisputeClose(closure), 1, &alice)]),
        BlockContext::at_height(10),
    )
    .expect("dispute");

    f.db.apply_block(
        &block_of(vec![signed_tx(TxKind::FinalizeDispute(id), 2, &alice)]),
        BlockContext::at_height(60),
    )
    .expect("finalize");

    assert_eq!(f.db.get_account(&alice_addr).map(|a| a.balance), Ok(8_500));
    assert_eq!(f.db.get_account(&bob_addr).map(|a| a.balance), Ok(1_500));
}

#[test]
fn a_newer_state_overrides_a_stale_dispute_without_a_penalty() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let (alice_addr, bob_addr) = (address_of(&alice), address_of(&bob));
    let f = fixture(&[(alice_addr, 10_000)]);

    let id = open_channel(&f, &alice, bob_addr, 4_000, 50, 0);

    let stale = signed_closure(id, 2, 4_000, 0, [1u8; 32], &alice, &bob);
    f.db.apply_block(
        &block_of(vec![signed_tx(TxKind::DisputeClose(stale), 1, &alice)]),
        BlockContext::at_height(10),
    )
    .expect("dispute");

    // Bob need not prove fraud — presenting a later state is enough.
    let newer = signed_closure(id, 9, 1_000, 3_000, [2u8; 32], &alice, &bob);
    f.db.apply_block(
        &block_of(vec![signed_tx(TxKind::DisputeClose(newer), 0, &bob)]),
        BlockContext::at_height(11),
    )
    .expect("newer state");

    let record = f.db.get_channel(&id).expect("read").expect("exists");
    assert_eq!(record.dispute_seq, 9);
    assert_eq!(record.dispute_closer, bob_addr);
    assert_eq!(record.dispute_balance_b, 3_000);
}

#[test]
fn an_older_state_cannot_override_a_newer_dispute() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let f = fixture(&[(address_of(&alice), 10_000)]);

    let id = open_channel(&f, &alice, address_of(&bob), 4_000, 50, 0);

    let newer = signed_closure(id, 9, 1_000, 3_000, [2u8; 32], &alice, &bob);
    f.db.apply_block(
        &block_of(vec![signed_tx(TxKind::DisputeClose(newer), 1, &alice)]),
        BlockContext::at_height(10),
    )
    .expect("dispute");

    let older = signed_closure(id, 3, 4_000, 0, [1u8; 32], &alice, &bob);
    assert!(matches!(
        f.db.apply_block(
            &block_of(vec![signed_tx(TxKind::DisputeClose(older), 0, &bob)]),
            BlockContext::at_height(11)
        ),
        Err(NodeError::StaleChannelState { .. })
    ));
}

#[test]
fn a_closed_channel_cannot_be_settled_twice() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let f = fixture(&[(address_of(&alice), 10_000)]);

    let id = open_channel(&f, &alice, address_of(&bob), 4_000, 50, 0);
    let closure = signed_closure(id, 1, 2_000, 2_000, [1u8; 32], &alice, &bob);

    f.db.apply_block(
        &block_of(vec![signed_tx(
            TxKind::CooperativeClose(closure.clone()),
            1,
            &alice,
        )]),
        BlockContext::at_height(2),
    )
    .expect("close");

    // Replaying the same closure would double-spend the escrow.
    assert!(matches!(
        f.db.apply_block(
            &block_of(vec![signed_tx(
                TxKind::CooperativeClose(closure),
                2,
                &alice
            )]),
            BlockContext::at_height(3)
        ),
        Err(NodeError::ChannelState { .. })
    ));
}

// ---------------------------------------------------------------------------
// batch settlement
// ---------------------------------------------------------------------------

#[test]
fn one_transaction_settles_many_channels() {
    let alice = generate_signing_key().expect("keygen");
    let submitter = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);

    const CHANNELS: usize = 25;
    let f = fixture(&[(alice_addr, 1_000_000), (address_of(&submitter), 10)]);

    let mut peers = Vec::new();
    let mut ids = Vec::new();
    for index in 0..CHANNELS {
        let peer = generate_signing_key().expect("keygen");
        let id = open_channel(&f, &alice, address_of(&peer), 1_000, 50, index as u64);
        ids.push(id);
        peers.push(peer);
    }

    let closures: Vec<ChannelClosure> = ids
        .iter()
        .zip(&peers)
        .map(|(id, peer)| signed_closure(*id, 5, 400, 600, [1u8; 32], &alice, peer))
        .collect();

    // A single transaction, signed by someone with no stake in any channel.
    f.db.apply_block(
        &block_of(vec![signed_tx(
            TxKind::SettleBatch(closures),
            0,
            &submitter,
        )]),
        BlockContext::at_height(2),
    )
    .expect("batch settle");

    for peer in &peers {
        assert_eq!(
            f.db.get_account(&address_of(peer)).map(|a| a.balance),
            Ok(600)
        );
    }
    // 1_000_000 - 25_000 escrowed + 25 * 400 returned.
    assert_eq!(
        f.db.get_account(&alice_addr).map(|a| a.balance),
        Ok(1_000_000 - 25_000 + 10_000)
    );
}

#[test]
fn one_forged_closure_rejects_the_entire_batch() {
    let alice = generate_signing_key().expect("keygen");
    let submitter = generate_signing_key().expect("keygen");
    let impostor = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);

    let f = fixture(&[(alice_addr, 100_000), (address_of(&submitter), 10)]);

    let peer_one = generate_signing_key().expect("keygen");
    let peer_two = generate_signing_key().expect("keygen");
    let id_one = open_channel(&f, &alice, address_of(&peer_one), 1_000, 50, 0);
    let id_two = open_channel(&f, &alice, address_of(&peer_two), 1_000, 50, 1);

    let good = signed_closure(id_one, 1, 500, 500, [1u8; 32], &alice, &peer_one);
    // peer_two never signed this; the impostor did.
    let forged = signed_closure(id_two, 1, 0, 1_000, [1u8; 32], &alice, &impostor);

    assert!(matches!(
        f.db.apply_block(
            &block_of(vec![signed_tx(
                TxKind::SettleBatch(vec![good, forged]),
                0,
                &submitter
            )]),
            BlockContext::at_height(2)
        ),
        Err(NodeError::ClosureSignature { .. })
    ));

    // Atomicity: the valid closure in the same batch must not have applied.
    assert_eq!(
        f.db.get_account(&address_of(&peer_one)).map(|a| a.balance),
        Ok(0)
    );
    assert_eq!(
        f.db.get_channel(&id_one)
            .expect("read")
            .expect("exists")
            .status,
        ChannelStatus::Open
    );
}

#[test]
fn a_batch_naming_one_channel_twice_settles_it_once() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let submitter = generate_signing_key().expect("keygen");
    let f = fixture(&[(address_of(&alice), 100_000), (address_of(&submitter), 10)]);

    let id = open_channel(&f, &alice, address_of(&bob), 1_000, 50, 0);
    let closure = signed_closure(id, 1, 400, 600, [1u8; 32], &alice, &bob);

    // The second entry sees the first's effect through the shared overlay.
    assert!(matches!(
        f.db.apply_block(
            &block_of(vec![signed_tx(
                TxKind::SettleBatch(vec![closure.clone(), closure]),
                0,
                &submitter
            )]),
            BlockContext::at_height(2)
        ),
        Err(NodeError::ChannelState { .. })
    ));
}

// ---------------------------------------------------------------------------
// state root
// ---------------------------------------------------------------------------

#[test]
fn channels_are_committed_to_the_state_root() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let f = fixture(&[(address_of(&alice), 10_000)]);

    let before = f.db.state_root().expect("root");
    open_channel(&f, &alice, address_of(&bob), 4_000, 50, 0);
    let after = f.db.state_root().expect("root");

    // Escrowed value that no commitment covered would be value a light client
    // could not verify.
    assert_ne!(before, after);
}
