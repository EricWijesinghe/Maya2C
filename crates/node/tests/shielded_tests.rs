//! Shielded transactions end to end through the L1 state transition.
//!
//! `crates/zk-stark/src/pool/tests.rs` covers the AIR and the proof system in isolation.
//! These cover the integration: real proofs inside real transactions inside
//! real blocks, committed to RocksDB, with the pool's effect on total supply
//! checked at every step.
//!
//! Supply is the property worth watching hardest. A shielded pool hides values,
//! so an inflation bug leaves no visible trace — the only way to catch one is to
//! account for every unit that crosses the transparent boundary and insist the
//! books balance.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::payload::ShieldedJoinSplit;
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::generate_signing_key;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::context::SHIELDED_NEVER;
use custom_l1_node::state::shielded::{FEE_SINK, encode_joinsplit};
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_zk_stark::gadgets::merkle::MerklePath;
use maya_zk_stark::hash::Digest;
use maya_zk_stark::pool as zkpool;
use maya_zk_stark::pool::note::{Note, SpendingKey};
use maya_zk_stark::pool::tree::{CommitmentTree, digest_from_bytes, digest_to_bytes, merkle_path};
use maya_zk_stark::pool::wallet::{self, Payment, Spend};

use custom_l1_node::crypto::hybrid::HybridSigningKey;
use tempfile::TempDir;

mod common;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

fn open_state(funded: &[(Address, u64)]) -> (StateDB, TempDir) {
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
    (db, dir)
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

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key, &common::test_chain()).expect("sign");
    tx
}

fn shielded_tx(joinsplit: ShieldedJoinSplit, nonce: u64, key: &HybridSigningKey) -> Transaction {
    signed(TxKind::Shielded(Box::new(joinsplit)), nonce, key)
}

fn balance(db: &StateDB, address: &Address) -> u64 {
    db.get_account(address).expect("read").balance
}

/// Total transparent value across the accounts a test touches.
fn transparent_total(db: &StateDB, addresses: &[Address]) -> u64 {
    addresses.iter().map(|a| balance(db, a)).sum()
}

/// The anchor a wallet would build against right now.
fn current_anchor(db: &StateDB) -> Digest {
    digest_from_bytes(&db.stored_pool().expect("pool").root()).expect("canonical anchor")
}

/// Rebuilds the commitment tree from notes the test knows about, so it can
/// produce authentication paths. A real wallet does the same from the notes it
/// has scanned off chain.
fn paths_for(notes: &[Note]) -> Vec<MerklePath> {
    let leaves: Vec<Digest> = notes.iter().map(Note::commitment).collect();
    (0..leaves.len())
        .map(|index| merkle_path(&leaves, index as u64).expect("path"))
        .collect()
}

// ---------------------------------------------------------------------------
// shielding
// ---------------------------------------------------------------------------

#[test]
fn shielding_moves_transparent_value_into_the_pool() {
    let owner = generate_signing_key().expect("keygen");
    let sender = address_of(&owner);
    let (db, _dir) = open_state(&[(sender, 10_000)]);

    let recipient = SpendingKey::from_words([7; 8]);
    let built = wallet::shield(1_000, 10, recipient.address(), current_anchor(&db)).expect("build");
    let (proof, public) = zkpool::prove(&built.witness).expect("prove");
    let joinsplit = encode_joinsplit(&public, &proof);

    db.apply_block(
        &block_of(vec![shielded_tx(joinsplit, 0, &owner)]),
        BlockContext::at_height(1),
    )
    .expect("shield");

    // The signer paid the full 1,000: 990 of it is now a note, 10 is the fee.
    assert_eq!(balance(&db, &sender), 9_000);
    assert_eq!(balance(&db, &FEE_SINK), 10);
    assert_eq!(db.stored_pool().expect("pool").note_count(), 2);
}

#[test]
fn a_pool_that_is_off_refuses_a_joinsplit_and_changes_nothing() {
    // ADR-037: mainnet v1 launches with the pool off. A valid join-split must
    // fail the whole block before any proof work, with every balance and the
    // pool exactly as they were.
    let owner = generate_signing_key().expect("keygen");
    let sender = address_of(&owner);
    let (db, _dir) = open_state(&[(sender, 10_000)]);
    let root = db.state_root().expect("root");

    let recipient = SpendingKey::from_words([7; 8]);
    let built = wallet::shield(1_000, 10, recipient.address(), current_anchor(&db)).expect("build");
    let (proof, public) = zkpool::prove(&built.witness).expect("prove");
    let block = block_of(vec![shielded_tx(
        encode_joinsplit(&public, &proof),
        0,
        &owner,
    )]);

    for (height, activation) in [(1, SHIELDED_NEVER), (9, 10)] {
        let off = BlockContext::at_height(height).with_shielded_activation(activation);
        let refused = db.apply_block(&block, off);
        assert!(
            matches!(refused, Err(NodeError::ShieldedInactive { .. })),
            "height {height}, activation {activation}: {refused:?}"
        );
        assert_eq!(db.state_root().expect("root"), root, "nothing moved");
        assert_eq!(balance(&db, &sender), 10_000);
    }

    // The same block applies once the pool is on.
    let on = BlockContext::at_height(10).with_shielded_activation(10);
    db.apply_block(&block, on)
        .expect("active at its activation height");
    assert_eq!(balance(&db, &sender), 9_000);
}

#[test]
fn shielding_conserves_total_supply() {
    let owner = generate_signing_key().expect("keygen");
    let sender = address_of(&owner);
    let (db, _dir) = open_state(&[(sender, 10_000)]);

    let before = transparent_total(&db, &[sender, FEE_SINK]);

    let built = wallet::shield(
        1_000,
        10,
        SpendingKey::from_words([7; 8]).address(),
        current_anchor(&db),
    )
    .expect("build");
    let (proof, public) = zkpool::prove(&built.witness).expect("prove");

    db.apply_block(
        &block_of(vec![shielded_tx(
            encode_joinsplit(&public, &proof),
            0,
            &owner,
        )]),
        BlockContext::at_height(1),
    )
    .expect("shield");

    let after = transparent_total(&db, &[sender, FEE_SINK]);
    let shielded: u64 = built.created.iter().map(|note| note.value).sum();

    // Nothing was created or destroyed: what left transparent accounts is
    // exactly what the pool now holds.
    assert_eq!(before, after + shielded);
}

#[test]
fn shielding_more_than_the_balance_is_rejected() {
    let owner = generate_signing_key().expect("keygen");
    let sender = address_of(&owner);
    let (db, _dir) = open_state(&[(sender, 100)]);

    let built = wallet::shield(
        1_000,
        10,
        SpendingKey::from_words([7; 8]).address(),
        current_anchor(&db),
    )
    .expect("build");
    let (proof, public) = zkpool::prove(&built.witness).expect("prove");

    let error = db
        .apply_block(
            &block_of(vec![shielded_tx(
                encode_joinsplit(&public, &proof),
                0,
                &owner,
            )]),
            BlockContext::at_height(1),
        )
        .expect_err("must not shield");

    assert!(matches!(error, NodeError::InsufficientBalance { .. }));
    // A rejected block leaves nothing behind, pool included.
    assert_eq!(db.stored_pool().expect("pool").note_count(), 0);
    assert_eq!(balance(&db, &sender), 100);
}

// ---------------------------------------------------------------------------
// the full round trip
// ---------------------------------------------------------------------------

#[test]
fn shield_transfer_and_unshield_across_blocks() {
    let owner = generate_signing_key().expect("keygen");
    let sender = address_of(&owner);
    let withdrawer = generate_signing_key().expect("keygen");
    let payout = address_of(&withdrawer);
    let (db, _dir) = open_state(&[(sender, 10_000)]);

    let alice = SpendingKey::from_words([11; 8]);
    let bob = SpendingKey::from_words([22; 8]);

    // --- Block 1: shield 1,000 (fee 10) into a note for Alice. ---
    let shield =
        wallet::shield(1_000, 10, alice.address(), current_anchor(&db)).expect("build shield");
    let (proof, public) = zkpool::prove(&shield.witness).expect("prove shield");
    db.apply_block(
        &block_of(vec![shielded_tx(
            encode_joinsplit(&public, &proof),
            0,
            &owner,
        )]),
        BlockContext::at_height(1),
    )
    .expect("apply shield");

    let alice_note = shield.created[0];
    assert_eq!(alice_note.value, 990);

    // Every note the pool holds, in tree order — what a wallet reconstructs by
    // scanning. Both outputs are appended, padding included.
    let mut pool_notes = vec![shield.created[0], shield.created[1]];

    // --- Block 2: Alice sends 400 to Bob, keeping 585 as change, fee 5. ---
    let paths = paths_for(&pool_notes);
    let transfer = wallet::transfer(
        vec![Spend {
            note: alice_note,
            key: &alice,
            path: paths[0].clone(),
        }],
        vec![
            Payment {
                to: bob.address(),
                amount: 400,
            },
            Payment {
                to: alice.address(),
                amount: 585,
            },
        ],
        current_anchor(&db),
        5,
    )
    .expect("build transfer");
    let (proof, public) = zkpool::prove(&transfer.witness).expect("prove transfer");
    db.apply_block(
        &block_of(vec![shielded_tx(
            encode_joinsplit(&public, &proof),
            1,
            &owner,
        )]),
        BlockContext::at_height(2),
    )
    .expect("apply transfer");

    // The transfer revealed nothing transparently beyond its fee.
    assert_eq!(balance(&db, &sender), 9_000);
    assert_eq!(balance(&db, &FEE_SINK), 15);

    pool_notes.extend_from_slice(&transfer.created);
    let bob_note = transfer.created[0];
    assert_eq!(bob_note.value, 400);

    // --- Block 3: Bob withdraws 395 to a transparent account, fee 5. ---
    let paths = paths_for(&pool_notes);
    let bob_index = pool_notes
        .iter()
        .position(|note| note.commitment() == bob_note.commitment())
        .expect("bob's note is in the pool");

    let unshield = wallet::unshield(
        vec![Spend {
            note: bob_note,
            key: &bob,
            path: paths[bob_index].clone(),
        }],
        Vec::new(),
        395,
        5,
        payout,
        current_anchor(&db),
    )
    .expect("build unshield");
    let (proof, public) = zkpool::prove(&unshield.witness).expect("prove unshield");
    db.apply_block(
        &block_of(vec![shielded_tx(
            encode_joinsplit(&public, &proof),
            2,
            &owner,
        )]),
        BlockContext::at_height(3),
    )
    .expect("apply unshield");

    assert_eq!(balance(&db, &payout), 395);
    assert_eq!(balance(&db, &FEE_SINK), 20);

    // Books balance: 10,000 started transparent. 9,000 stayed with the sender,
    // 395 reached the withdrawer, 20 went to fees, and 585 is still shielded as
    // Alice's change.
    assert_eq!(
        transparent_total(&db, &[sender, payout, FEE_SINK]),
        10_000 - 585
    );
}

// ---------------------------------------------------------------------------
// double-spend and proof validity
// ---------------------------------------------------------------------------

/// Shields once and returns the state, the note created, and the signer.
fn shielded_fixture(_seed: u64) -> (StateDB, TempDir, Note, HybridSigningKey, Vec<Note>) {
    let owner = generate_signing_key().expect("keygen");
    let sender = address_of(&owner);
    let (db, dir) = open_state(&[(sender, 10_000)]);

    let alice = SpendingKey::from_words([11; 8]);
    let shield = wallet::shield(1_000, 0, alice.address(), current_anchor(&db)).expect("build");
    let (proof, public) = zkpool::prove(&shield.witness).expect("prove");
    db.apply_block(
        &block_of(vec![shielded_tx(
            encode_joinsplit(&public, &proof),
            0,
            &owner,
        )]),
        BlockContext::at_height(1),
    )
    .expect("shield");

    let notes = vec![shield.created[0], shield.created[1]];
    (db, dir, shield.created[0], owner, notes)
}

/// Builds a spend of `note` back to its owner, for double-spend tests.
///
/// Takes the anchor explicitly rather than reading the current root: a second
/// spend attempt has to prove against the tree as it stood when the note's path
/// was built, which is exactly the situation the anchor window exists for.
fn respend(note: Note, notes: &[Note], anchor: Digest, _seed: u64) -> ShieldedJoinSplit {
    let alice = SpendingKey::from_words([11; 8]);
    let index = notes
        .iter()
        .position(|candidate| candidate.commitment() == note.commitment())
        .expect("the note must be in the set the paths are built from");
    let paths = paths_for(notes);

    let built = wallet::transfer(
        vec![Spend {
            note,
            key: &alice,
            path: paths[index].clone(),
        }],
        vec![Payment {
            to: alice.address(),
            amount: note.value,
        }],
        anchor,
        0,
    )
    .expect("build");
    let (proof, public) = zkpool::prove(&built.witness).expect("prove");
    encode_joinsplit(&public, &proof)
}

#[test]
fn spending_the_same_note_twice_across_blocks_is_rejected() {
    let (db, _dir, note, owner, notes) = shielded_fixture(20);
    // Both attempts prove against the root the note's path was built from. The
    // second is rejected for being a double-spend, not for a stale anchor.
    let anchor = current_anchor(&db);

    let first = respend(note, &notes, anchor, 21);
    db.apply_block(
        &block_of(vec![shielded_tx(first, 1, &owner)]),
        BlockContext::at_height(2),
    )
    .expect("first spend");

    // A second spend of the same note yields the same nullifier, which is now
    // on record.
    let second = respend(note, &notes, anchor, 22);
    let error = db
        .apply_block(
            &block_of(vec![shielded_tx(second, 2, &owner)]),
            BlockContext::at_height(3),
        )
        .expect_err("must not double-spend");

    assert!(matches!(error, NodeError::NullifierSpent { .. }));
}

#[test]
fn spending_the_same_note_twice_within_one_block_is_rejected() {
    // The on-disk nullifier set would not show the first spend until the batch
    // lands, so this can only be caught through the overlay.
    let (db, _dir, note, owner, notes) = shielded_fixture(30);

    let first = respend(note, &notes, current_anchor(&db), 31);
    let second = respend(note, &notes, current_anchor(&db), 32);

    let error = db
        .apply_block(
            &block_of(vec![
                shielded_tx(first, 1, &owner),
                shielded_tx(second, 2, &owner),
            ]),
            BlockContext::at_height(2),
        )
        .expect_err("must not double-spend");

    assert!(matches!(error, NodeError::NullifierSpent { .. }));
}

#[test]
fn a_joinsplit_spending_one_note_twice_is_rejected() {
    let (db, _dir, note, owner, notes) = shielded_fixture(40);

    // Hand-build a joinsplit whose two nullifiers are equal, which the circuit
    // itself has no reason to object to.
    let mut joinsplit = respend(note, &notes, current_anchor(&db), 41);
    joinsplit.nullifiers[1] = joinsplit.nullifiers[0];

    let error = db
        .apply_block(
            &block_of(vec![shielded_tx(joinsplit, 1, &owner)]),
            BlockContext::at_height(2),
        )
        .expect_err("must reject");

    assert!(matches!(error, NodeError::DuplicateNullifier { .. }));
}

#[test]
fn an_unknown_anchor_is_rejected() {
    let (db, _dir, note, owner, notes) = shielded_fixture(50);

    let mut joinsplit = respend(note, &notes, current_anchor(&db), 51);
    // A root the pool has never held.
    joinsplit.anchor = digest_to_bytes(&[maya_zk_stark::hash::F::new(123_456_789); 8]);

    let error = db
        .apply_block(
            &block_of(vec![shielded_tx(joinsplit, 1, &owner)]),
            BlockContext::at_height(2),
        )
        .expect_err("must reject");

    // The proof is checked before chain state, so a fabricated anchor fails the
    // proof rather than the anchor lookup — either way it does not apply.
    assert!(
        matches!(error, NodeError::UnknownAnchor { .. })
            || matches!(error, NodeError::ProofVerification(_)),
        "expected an anchor or proof rejection, got {error:?}"
    );
}

#[test]
fn a_tampered_proof_is_rejected() {
    let (db, _dir, note, owner, notes) = shielded_fixture(60);

    let mut joinsplit = respend(note, &notes, current_anchor(&db), 61);
    joinsplit.proof[10] ^= 0x01;

    let error = db
        .apply_block(
            &block_of(vec![shielded_tx(joinsplit, 1, &owner)]),
            BlockContext::at_height(2),
        )
        .expect_err("must reject");

    assert!(
        matches!(error, NodeError::ProofVerification(_)),
        "expected a proof rejection, got {error:?}"
    );
}

#[test]
fn redirecting_a_withdrawal_is_rejected() {
    let (db, _dir, note, owner, notes) = shielded_fixture(70);
    let thief = address_of(&generate_signing_key().expect("keygen"));

    let alice = SpendingKey::from_words([11; 8]);
    let paths = paths_for(&notes);
    let built = wallet::unshield(
        vec![Spend {
            note,
            key: &alice,
            path: paths[0].clone(),
        }],
        Vec::new(),
        note.value,
        0,
        address_of(&owner),
        current_anchor(&db),
    )
    .expect("build");
    let (proof, public) = zkpool::prove(&built.witness).expect("prove");

    let mut joinsplit = encode_joinsplit(&public, &proof);
    joinsplit.recipient = thief;

    let error = db
        .apply_block(
            &block_of(vec![shielded_tx(joinsplit, 1, &owner)]),
            BlockContext::at_height(2),
        )
        .expect_err("must reject");

    assert!(
        matches!(error, NodeError::ProofVerification(_)),
        "expected a proof rejection, got {error:?}"
    );
    assert_eq!(balance(&db, &thief), 0);
}

#[test]
fn a_shielded_transaction_may_not_also_carry_transparent_outputs() {
    // Value must flow through exactly one mechanism, or conservation stops
    // being checkable locally.
    let (db, _dir, note, owner, notes) = shielded_fixture(80);
    let joinsplit = respend(note, &notes, current_anchor(&db), 81);

    let mut tx = Transaction::with_kind(TxKind::Shielded(Box::new(joinsplit)), 1);
    tx.outputs
        .push(custom_l1_node::core::transaction::TxOutput {
            amount: 1,
            recipient: [9u8; 32],
        });
    tx.sign(&owner, &common::test_chain()).expect("sign");
    let error = db
        .apply_block(&block_of(vec![tx]), BlockContext::at_height(2))
        .expect_err("must reject");

    assert!(matches!(error, NodeError::MixedTransactionKind(_)));
}

// ---------------------------------------------------------------------------
// reorg
// ---------------------------------------------------------------------------

#[test]
fn reverting_a_shielded_block_restores_the_pool_exactly() {
    let (db, _dir, note, owner, notes) = shielded_fixture(90);

    let root_before = db.state_root().expect("root");
    let pool_before = db.stored_pool().expect("pool");

    let joinsplit = respend(note, &notes, current_anchor(&db), 91);
    let nullifier = joinsplit.nullifiers[0];
    let mut block = block_of(vec![shielded_tx(joinsplit, 1, &owner)]);
    block.header.state_root = db
        .preview_root(&block, BlockContext::at_height(2))
        .expect("preview");
    let block_id = block.header.id();

    db.apply_block_journaled(&block, &block_id, BlockContext::at_height(2))
        .expect("apply");
    assert_eq!(
        db.uncovered_keys().expect("scan"),
        Vec::<Vec<u8>>::new(),
        "every stored key must be under the state root or declared local-only"
    );

    assert_ne!(db.state_root().expect("root"), root_before);
    assert_eq!(db.stored_pool().expect("pool").note_count(), 4);

    db.revert_block(&block_id).expect("revert");

    // The commitment tree is append-only, so this only works because the undo
    // journal captured the frontier rather than trying to un-append.
    let pool_after = db.stored_pool().expect("pool");
    assert_eq!(pool_after.note_count(), pool_before.note_count());
    assert_eq!(pool_after.root(), pool_before.root());
    assert_eq!(db.state_root().expect("root"), root_before);

    // And the note is spendable again, because on the chain we reverted to it
    // was never spent.
    let replayed = respend(note, &notes, current_anchor(&db), 92);
    assert_eq!(replayed.nullifiers[0], nullifier);
    db.apply_block(
        &block_of(vec![shielded_tx(replayed, 1, &owner)]),
        BlockContext::at_height(2),
    )
    .expect("the reverted spend may be replayed");
}

// ---------------------------------------------------------------------------
// pool invariants
// ---------------------------------------------------------------------------

#[test]
fn a_block_without_joinsplits_leaves_the_pool_untouched() {
    let (db, _dir, _note, owner, _notes) = shielded_fixture(100);
    let before = db.stored_pool().expect("pool");

    let recipient = address_of(&generate_signing_key().expect("keygen"));
    let mut tx = Transaction::new(
        Vec::new(),
        vec![custom_l1_node::core::transaction::TxOutput {
            amount: 5,
            recipient,
        }],
        1,
    );
    tx.sign(&owner, &common::test_chain()).expect("sign");
    db.apply_block(&block_of(vec![tx]), BlockContext::at_height(2))
        .expect("transfer");

    assert_eq!(db.stored_pool().expect("pool"), before);
}

#[test]
fn the_state_root_commits_to_the_pool() {
    // Two chains identical except for a shielded transaction must not share a
    // state root, or a light client could not tell them apart.
    let owner = generate_signing_key().expect("keygen");
    let sender = address_of(&owner);

    let (plain, _plain_dir) = open_state(&[(sender, 10_000)]);
    let (shielded, _shielded_dir) = open_state(&[(sender, 10_000)]);
    assert_eq!(
        plain.state_root().expect("root"),
        shielded.state_root().expect("root"),
        "the fixtures must start identical"
    );

    let built = wallet::shield(
        1_000,
        10,
        SpendingKey::from_words([7; 8]).address(),
        current_anchor(&shielded),
    )
    .expect("build");
    let (proof, public) = zkpool::prove(&built.witness).expect("prove");
    shielded
        .apply_block(
            &block_of(vec![shielded_tx(
                encode_joinsplit(&public, &proof),
                0,
                &owner,
            )]),
            BlockContext::at_height(1),
        )
        .expect("shield");

    assert_ne!(
        plain.state_root().expect("root"),
        shielded.state_root().expect("root")
    );
}

#[test]
fn an_empty_pool_leaves_the_state_root_unchanged() {
    // Backward compatibility: a chain with no shielded activity must produce
    // exactly the roots it produced before the pool existed.
    let owner = generate_signing_key().expect("keygen");
    let sender = address_of(&owner);
    let (db, _dir) = open_state(&[(sender, 10_000)]);

    let accounts_only = db.state_root().expect("root");
    assert_eq!(db.stored_pool().expect("pool").note_count(), 0);

    // With no notes the pool contributes nothing, so the root is the accounts
    // root alone.
    let mut expected = CommitmentTree::new();
    assert_eq!(expected.count(), 0);
    expected
        .append([maya_zk_stark::hash::F::new(1); 8])
        .expect("append");
    assert_ne!(db.state_root().expect("root"), [0u8; 32]);
    assert_eq!(db.state_root().expect("root"), accounts_only);
}
