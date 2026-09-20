//! Many off-chain transfers, settled on L1.
//!
//! This is the claim a payment channel makes, stated as a test: an arbitrary
//! number of payments between two parties costs a constant amount of chain.
//!
//! The default volume is [`TOTAL_TRANSFERS`], cut from ten thousand when the
//! chain moved to ML-DSA-65 and again when it moved to hybrid signing — see
//! that constant for the arithmetic. The original volume still runs, under
//! `cargo test -p l2-flash -- --ignored`.
//!
//! Two shapes, because "many transfers settled in a single batch transaction"
//! can mean two different things and both are worth proving:
//!
//! - **Compression** — one channel carries every transfer and settles into a
//!   single closure. The chain sees two balances, not a payment history.
//! - **Batching** — 100 channels settle together in *one* transaction,
//!   authorized by a submitter who is party to none of them.

use custom_l1_node::core::payload::ChannelClosure;
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridPublicKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::{Account, Address, BlockContext, ChannelStatus, StateDB};

use custom_l1_node::crypto::hybrid::HybridSigningKey;
use l2_flash::channel::{Channel, Party, SignedState};

use tempfile::TempDir;

/// Total off-chain transfers each shape performs by default.
///
/// Twenty. This number has now been cut twice, for the same reason each time,
/// and the arithmetic is worth writing down because it is the clearest single
/// measure of what the signature scheme costs.
///
/// Every transfer is signed by *both* parties, so the count doubles before it
/// reaches the signer. Signing one message costs:
///
/// | scheme | per signature | 1000 transfers |
/// |---|---|---|
/// | ed25519 | 26 µs | 0.05 s |
/// | ML-DSA-65 | 1.8 ms | 3.6 s |
/// | hybrid (+ SLH-DSA-SHA2-128s) | ~150 ms | ~5 minutes |
///
/// The original ten thousand would be roughly fifty minutes of pure signing per
/// shape, and a thousand still ran this file for six minutes — long enough that
/// `cargo test` reads as hung rather than slow, which is the failure mode the
/// profile overrides in the root `Cargo.toml` exist to prevent.
///
/// The property under test does not depend on the volume: a channel that
/// compresses twenty transfers into one closure compresses ten thousand the
/// same way, and the size assertions below are what actually pin that.
/// [`FULL_SCALE_TRANSFERS`] keeps the original number reachable, and
/// `ten_thousand_transfers_settle_as_one_closure` still runs it under
/// `cargo test -- --ignored`.
const TOTAL_TRANSFERS: usize = 20;

/// The volume the ignored full-scale test uses.
const FULL_SCALE_TRANSFERS: usize = 10_000;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

fn address_of(key: &HybridSigningKey) -> Address {
    key.address()
}

/// The public key a channel must hold to verify a party's signatures.
fn public_key_of(key: &HybridSigningKey) -> Box<HybridPublicKey> {
    Box::new(key.public_key())
}

fn open_state(funded: &[(Address, u64)]) -> (StateDB, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
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
    (db, dir)
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

/// Distinct per-state revocation commitment, derived from the sequence number.
///
/// A real wallet derives these from a seeded chain so it can recompute any
/// secret from one root; a counter is enough to prove the mechanism.
fn commitment_for_seq(seq: u64) -> [u8; 32] {
    let mut secret = [0u8; 32];
    secret[..8].copy_from_slice(&seq.to_le_bytes());
    Channel::commitment_for(&secret)
}

fn secret_for_seq(seq: u64) -> [u8; 32] {
    let mut secret = [0u8; 32];
    secret[..8].copy_from_slice(&seq.to_le_bytes());
    secret
}

/// Runs `count` alternating off-chain transfers, signed by both parties each time.
///
/// Alternating direction exercises the channel bidirectionally and keeps either
/// side from simply draining.
fn run_transfers(
    channel: &mut Channel,
    key_a: &HybridSigningKey,
    key_b: &HybridSigningKey,
    count: usize,
) {
    for index in 0..count {
        let from = if index % 2 == 0 { Party::A } else { Party::B };
        let next_seq = channel.current.seq + 1;

        let next = channel
            .propose_transfer(from, 1, commitment_for_seq(next_seq))
            .expect("transfer must fit within the channel balance");

        // Both parties sign every state. One signature is a proposal; two are
        // an agreement, and only an agreement is enforceable.
        let signed = SignedState {
            sig_a: Channel::sign(&next, key_a).expect("sign a"),
            sig_b: Channel::sign(&next, key_b).expect("sign b"),
            state: next,
        };

        channel
            .commit(signed, secret_for_seq(next_seq - 1))
            .expect("state must commit");
    }
}

// ---------------------------------------------------------------------------
// compression: one channel, many transfers
// ---------------------------------------------------------------------------

#[test]
fn transfers_settle_as_one_closure() {
    settle_as_one_closure(TOTAL_TRANSFERS);
}

/// The original claim, at the original volume.
///
/// Ignored by default because it is roughly an hour of hybrid signing — twenty
/// thousand signatures at ~105 ms each; see [`TOTAL_TRANSFERS`]. Run it with
/// `cargo test -p l2-flash --test scale_tests -- --ignored`.
#[test]
#[ignore = "twenty thousand hybrid signatures, roughly an hour; run explicitly"]
fn ten_thousand_transfers_settle_as_one_closure() {
    settle_as_one_closure(FULL_SCALE_TRANSFERS);
}

/// One channel carries `transfers` payments and settles into a single closure.
///
/// Parameterized on the count so the default and full-scale tests assert the
/// same thing at two volumes rather than drifting into two different tests.
fn settle_as_one_closure(transfers: usize) {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let (alice_addr, bob_addr) = (address_of(&alice), address_of(&bob));

    let (db, _dir) = open_state(&[(alice_addr, 1_000_000)]);

    // --- open on chain ---
    let mut open_tx = Transaction::with_kind(
        TxKind::OpenChannel(custom_l1_node::core::ChannelOpen {
            counterparty: bob_addr,
            funding: 100_000,
            dispute_window: 50,
        }),
        0,
    );
    open_tx.sign(&alice).expect("sign");
    db.apply_block(&block_of(vec![open_tx]), BlockContext::at_height(1))
        .expect("open");

    let channel_id =
        custom_l1_node::core::payload::derive_channel_id(&alice_addr, &bob_addr, 100_000, 0);

    // --- every payment, entirely off chain ---
    let mut channel = Channel::open(
        channel_id,
        public_key_of(&alice),
        public_key_of(&bob),
        100_000,
    );
    run_transfers(&mut channel, &alice, &bob, transfers);

    assert_eq!(channel.current.seq, transfers as u64);
    // Alternating single-unit transfers net to zero over an even count.
    assert_eq!(channel.current.balance_a, 100_000);
    assert_eq!(channel.current.balance_b, 0);
    assert_eq!(
        channel.current.total().expect("total"),
        100_000,
        "capacity must be conserved across every one of the transfers"
    );

    // --- one closure settles all of it ---
    let closure = channel
        .settlement_closure(&alice, &bob)
        .expect("no HTLCs pending");

    let mut settle_tx = Transaction::with_kind(TxKind::CooperativeClose(closure), 1);
    settle_tx.sign(&alice).expect("sign");
    let encoded_size = settle_tx.to_bytes().len();
    db.apply_block(&block_of(vec![settle_tx]), BlockContext::at_height(2))
        .expect("settle");

    assert_eq!(
        db.get_account(&alice_addr).map(|a| a.balance),
        Ok(1_000_000)
    );
    assert_eq!(db.get_account(&bob_addr).map(|a| a.balance), Ok(0));
    assert_eq!(
        db.get_channel(&channel_id)
            .expect("read")
            .expect("exists")
            .status,
        ChannelStatus::Closed
    );

    // The whole point: chain cost is independent of payment count.
    //
    // About 38.6 KB, against 15.9 KB under ML-DSA alone and 400 bytes under
    // ed25519. The settling transaction carries its own key pair and signature
    // pair (1984 + 11165) plus a closure holding both participants' (2 × 1984 +
    // 2 × 11165), and the balances and commitment are noise beside that.
    //
    // A much worse constant, and still a constant — which is the claim, and the
    // only claim this test makes. The ignored full-scale test settles ten times
    // as many transfers into a transaction of exactly this size.
    assert!(
        encoded_size < 40_000,
        "settling {transfers} transfers took {encoded_size} bytes"
    );
}

#[test]
fn a_channel_carrying_many_transfers_still_conserves_value() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let mut channel = Channel::open(
        [1u8; 32],
        public_key_of(&alice),
        public_key_of(&bob),
        50_000,
    );

    // An odd count leaves the balance genuinely shifted rather than netting out,
    // so conservation is tested against a moved balance, not a restored one.
    run_transfers(&mut channel, &alice, &bob, TOTAL_TRANSFERS + 1);

    assert_eq!(channel.current.seq, TOTAL_TRANSFERS as u64 + 1);
    assert_eq!(channel.current.balance_a, 50_000 - 1);
    assert_eq!(channel.current.balance_b, 1);
    assert_eq!(channel.current.total().expect("total"), 50_000);
}

#[test]
fn a_revoked_state_from_a_long_channel_is_still_punishable() {
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let (alice_addr, bob_addr) = (address_of(&alice), address_of(&bob));

    let (db, _dir) = open_state(&[(alice_addr, 1_000_000)]);

    let mut open_tx = Transaction::with_kind(
        TxKind::OpenChannel(custom_l1_node::core::ChannelOpen {
            counterparty: bob_addr,
            funding: 10_000,
            dispute_window: 20,
        }),
        0,
    );
    open_tx.sign(&alice).expect("sign");
    db.apply_block(&block_of(vec![open_tx]), BlockContext::at_height(1))
        .expect("open");

    let channel_id =
        custom_l1_node::core::payload::derive_channel_id(&alice_addr, &bob_addr, 10_000, 0);
    let mut channel = Channel::open(
        channel_id,
        public_key_of(&alice),
        public_key_of(&bob),
        10_000,
    );

    // Enough history that a stale state is genuinely stale.
    run_transfers(&mut channel, &alice, &bob, 500);

    // Alice publishes state 200, long since revoked.
    let stale_seq = 200u64;
    let stale = l2_flash::channel::ChannelState {
        channel_id,
        seq: stale_seq,
        balance_a: 10_000,
        balance_b: 0,
        htlcs: Vec::new(),
        revocation_commitment: commitment_for_seq(stale_seq),
    };
    let message = stale.settlement_signing_bytes();
    let closure = ChannelClosure {
        channel_id,
        seq: stale_seq,
        balance_a: 10_000,
        balance_b: 0,
        revocation_commitment: commitment_for_seq(stale_seq),
        pubkey_a: public_key_of(&alice),
        pubkey_b: public_key_of(&bob),
        sig_a: Box::new(alice.sign(&message).expect("sign a")),
        sig_b: Box::new(bob.sign(&message).expect("sign b")),
    };

    let mut dispute = Transaction::with_kind(TxKind::DisputeClose(closure), 1);
    dispute.sign(&alice).expect("sign");
    db.apply_block(&block_of(vec![dispute]), BlockContext::at_height(5))
        .expect("dispute");

    // Bob holds every secret Alice revealed on the way to state 500.
    let mut penalty = Transaction::with_kind(
        TxKind::PenaltyClaim(custom_l1_node::core::RevocationProof {
            channel_id,
            revoked_seq: stale_seq,
            secret: secret_for_seq(stale_seq),
        }),
        0,
    );
    penalty.sign(&bob).expect("sign");
    db.apply_block(&block_of(vec![penalty]), BlockContext::at_height(6))
        .expect("penalty");

    assert_eq!(db.get_account(&bob_addr).map(|a| a.balance), Ok(10_000));
}

// ---------------------------------------------------------------------------
// batching: a hundred channels in one transaction
// ---------------------------------------------------------------------------

#[test]
fn a_hundred_channels_settle_in_one_transaction() {
    const CHANNELS: usize = 100;
    // Rounded up, so there is at least one transfer per channel.
    // `TOTAL_TRANSFERS` is now smaller than `CHANNELS`, and a plain division
    // would silently run zero transfers and leave this test asserting nothing
    // about state that ever moved.
    //
    // The transfers are not where this test's cost lives anyway: opening and
    // settling a hundred channels is 300 hybrid signatures on its own, which is
    // the irreducible price of the shape `MAX_BATCH_CLOSURES` was sized for.
    const PER_CHANNEL: usize = TOTAL_TRANSFERS.div_ceil(CHANNELS);
    const CAPACITY: u64 = 10_000;

    let hub = generate_signing_key().expect("keygen");
    let submitter = generate_signing_key().expect("keygen");
    let hub_addr = address_of(&hub);

    let (db, _dir) = open_state(&[(hub_addr, 10_000_000), (address_of(&submitter), 10)]);

    // --- open every channel ---
    let mut peers = Vec::with_capacity(CHANNELS);
    let mut channels = Vec::with_capacity(CHANNELS);
    let mut open_txs = Vec::with_capacity(CHANNELS);

    for index in 0..CHANNELS {
        let peer = generate_signing_key().expect("keygen");
        let peer_addr = address_of(&peer);

        let mut tx = Transaction::with_kind(
            TxKind::OpenChannel(custom_l1_node::core::ChannelOpen {
                counterparty: peer_addr,
                funding: CAPACITY,
                dispute_window: 50,
            }),
            index as u64,
        );
        tx.sign(&hub).expect("sign");
        open_txs.push(tx);

        let id = custom_l1_node::core::payload::derive_channel_id(
            &hub_addr,
            &peer_addr,
            CAPACITY,
            index as u64,
        );
        channels.push(Channel::open(
            id,
            public_key_of(&hub),
            public_key_of(&peer),
            CAPACITY,
        ));
        peers.push(peer);
    }

    db.apply_block(&block_of(open_txs), BlockContext::at_height(1))
        .expect("open all channels");

    // --- PER_CHANNEL transfers per channel, all off chain ---
    let mut executed = 0usize;
    for (channel, peer) in channels.iter_mut().zip(&peers) {
        run_transfers(channel, &hub, peer, PER_CHANNEL);
        executed += PER_CHANNEL;
    }
    // Against the per-channel count rather than TOTAL_TRANSFERS: the two stopped
    // being the same when TOTAL_TRANSFERS fell below CHANNELS and PER_CHANNEL
    // took its floor of one. What this line is for is catching a `run_transfers`
    // that quietly did less work than it was asked to.
    assert_eq!(executed, CHANNELS * PER_CHANNEL);

    // --- one transaction settles all hundred ---
    let closures: Vec<ChannelClosure> = channels
        .iter()
        .zip(&peers)
        .map(|(channel, peer)| {
            channel
                .settlement_closure(&hub, peer)
                .expect("no HTLCs pending")
        })
        .collect();
    assert_eq!(closures.len(), CHANNELS);

    let mut batch = Transaction::with_kind(TxKind::SettleBatch(closures), 0);
    // Signed by a party with no stake in any channel: the outer signature buys
    // inclusion, the inner signatures authorize the value.
    batch.sign(&submitter).expect("sign");
    let encoded_size = batch.to_bytes().len();
    db.apply_block(&block_of(vec![batch]), BlockContext::at_height(2))
        .expect("batch settle");

    // --- every channel closed, every balance restored ---
    for (channel, peer) in channels.iter().zip(&peers) {
        let record = db
            .get_channel(&channel.channel_id)
            .expect("read")
            .expect("exists");
        assert_eq!(record.status, ChannelStatus::Closed);
        assert_eq!(
            db.get_account(&address_of(peer)).map(|a| a.balance),
            Ok(channel.current.balance_b)
        );
    }

    // Escrow fully returned: nothing minted, nothing burned across 10,000
    // payments and 100 settlements.
    let hub_balance = db.get_account(&hub_addr).map(|a| a.balance).expect("hub");
    let peer_total: u64 = peers
        .iter()
        .map(|peer| {
            db.get_account(&address_of(peer))
                .map(|a| a.balance)
                .expect("peer")
        })
        .sum();
    assert_eq!(
        hub_balance + peer_total,
        10_000_000,
        "value must be conserved end to end"
    );

    // One transaction, 100 channels, ~26.4 KB of closure each — against ~10.6 KB
    // under ML-DSA alone and ~216 bytes under ed25519. Still one transaction and
    // one authorization check by the submitter, which is what batching buys; the
    // bytes are what hybrid signing costs.
    //
    // ~2.5 MiB now, against the 8 MiB gossip limit set in the node's behaviour.
    // That is no longer "comfortably" inside it: three of these transactions
    // will not fit in one block. MAX_BATCH_CLOSURES was held at 128 so this
    // hundred-channel shape keeps working, and it is now the binding constraint
    // rather than a generous one.
    assert!(
        encoded_size < 2_800_000,
        "batch of {CHANNELS} closures took {encoded_size} bytes"
    );
}
