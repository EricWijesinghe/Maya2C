//! Names with commit-reveal (ADR-135 P-3, feature bit 43) against real block
//! execution: the replay tests `docs/threat-models/v4-names.md` lists, one per
//! attack. Written before the implementation (TDD); CI is the first compile.
//!
//! Every refusal asserts its specific reason. The attacknet scenario (threat
//! model item 13) is not here: it needs the attacknet harness and is NOT
//! VERIFIED in this change.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::core::name_payload::{NameAction, NameUpdate, validate_label};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::governance::ParameterTable;
use custom_l1_node::state::invariant_guard::{MODULES, Module};
use custom_l1_node::state::names::{
    GRACE_BLOCKS, MAX_COMMIT_AGE, MAX_COMMIT_PRUNE_PER_BLOCK, MIN_COMMIT_AGE, RESERVED_LABELS,
    YEAR_BLOCKS, name_commitment,
};
use custom_l1_node::state::shielded::FEE_SINK;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_governance::params::ParameterKey;
use primitive_types::U256;
use tempfile::TempDir;

mod common;

/// The v4 activation height these tests run under.
const V4: u64 = 10;
const SALT: [u8; 32] = [0x5A; 32];

struct Fixture {
    db: Arc<StateDB>,
    _dir: TempDir,
}

fn fixture(funded: &[Address]) -> Fixture {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    common::bind(&db);
    for address in funded {
        db.put_account(
            address,
            &Account {
                balance: U256::from(1_000_000_000_000_000_000_000_000u128),
                nonce: 0,
            },
        )
        .expect("fund");
    }
    Fixture {
        db: Arc::new(db),
        _dir: dir,
    }
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            version: 0,
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

/// A context at `height` with bit 43 active from [`V4`].
fn at(height: u64) -> BlockContext {
    BlockContext::at_height(height).with_names_activation(V4)
}

fn apply(f: &Fixture, txs: Vec<Transaction>, height: u64) -> Result<[u8; 32], NodeError> {
    f.db.apply_block(&block_of(txs), at(height))
}

fn commit(commitment: [u8; 32]) -> TxKind {
    TxKind::Name(NameAction::Commit { commitment })
}

fn register(label: &[u8], periods: u8, target: Address) -> TxKind {
    TxKind::Name(NameAction::Register {
        label: label.to_vec(),
        salt: SALT,
        periods,
        target,
    })
}

fn renew(label: &[u8], periods: u8) -> TxKind {
    TxKind::Name(NameAction::Renew {
        label: label.to_vec(),
        periods,
    })
}

fn update(label: &[u8], op: NameUpdate) -> TxKind {
    TxKind::Name(NameAction::Update {
        label: label.to_vec(),
        op,
    })
}

fn refusal(error: &NodeError) -> &'static str {
    match error {
        NodeError::NameRefused { reason } => *reason,
        other => panic!("expected NameRefused, got {other:?}"),
    }
}

fn balance(f: &Fixture, who: &Address) -> U256 {
    f.db.get_account(who).expect("read").balance
}

/// Commits `label` for `key` at `height` and returns the commitment.
fn commit_at(
    f: &Fixture,
    key: &HybridSigningKey,
    label: &[u8],
    nonce: u64,
    height: u64,
) -> [u8; 32] {
    let c = name_commitment(label, &key.address(), &SALT);
    apply(f, vec![signed(commit(c), nonce, key)], height).expect("commit");
    c
}

// M-1
#[test]
fn a_reveal_without_an_aged_commitment_by_the_same_sender_is_refused() {
    let alice = generate_signing_key().expect("key");
    let mallory = generate_signing_key().expect("key");
    let f = fixture(&[alice.address(), mallory.address()]);
    commit_at(&f, &alice, b"alice", 0, V4);

    // Mallory copies the reveal: the commitment hashes in her own address, so
    // there is no aged commitment of hers to match.
    let steal = signed(register(b"alice", 1, mallory.address()), 0, &mallory);
    let err = apply(&f, vec![steal], V4 + MIN_COMMIT_AGE + 1).unwrap_err();
    assert!(refusal(&err).contains("no commitment"), "{err}");
}

// M-1 (the producer case)
#[test]
fn a_producer_that_sees_a_reveal_cannot_register_the_label_in_the_same_block() {
    let victim = generate_signing_key().expect("key");
    let producer = generate_signing_key().expect("key");
    let f = fixture(&[victim.address(), producer.address()]);
    commit_at(&f, &victim, b"alice", 0, V4);
    // The producer commits its own copy the block it sees the reveal, and
    // tries to register in the very next block: its commitment is too young.
    commit_at(&f, &producer, b"alice", 0, V4 + 100);
    let racing = signed(register(b"alice", 1, producer.address()), 1, &producer);
    let err = apply(&f, vec![racing], V4 + 101).unwrap_err();
    assert!(refusal(&err).contains("too young"), "{err}");
}

// M-2
#[test]
fn a_censored_reveal_still_registers_once_included_within_max_commit_age() {
    let victim = generate_signing_key().expect("key");
    let f = fixture(&[victim.address()]);
    commit_at(&f, &victim, b"alice", 0, V4);
    let late = V4 + MAX_COMMIT_AGE;
    apply(
        &f,
        vec![signed(register(b"alice", 1, victim.address()), 1, &victim)],
        late,
    )
    .expect("registers at the last aged block");
    let name = f.db.get_name(b"alice").expect("read").expect("registered");
    assert_eq!(name.owner, victim.address());
}

// M-3
#[test]
fn a_duplicate_commitment_is_refused_and_the_age_does_not_reset() {
    let victim = generate_signing_key().expect("key");
    let f = fixture(&[victim.address()]);
    let c = commit_at(&f, &victim, b"alice", 0, V4);
    let again = signed(commit(c), 1, &victim);
    let err = apply(&f, vec![again], V4 + 50).unwrap_err();
    assert!(refusal(&err).contains("already exists"), "{err}");
    assert_eq!(
        f.db.commitment_height(&victim.address(), &c).expect("read"),
        Some(V4)
    );
}

// M-11
#[test]
fn a_commitment_older_than_max_commit_age_is_refused_and_pruned() {
    let victim = generate_signing_key().expect("key");
    let f = fixture(&[victim.address()]);
    let c = commit_at(&f, &victim, b"alice", 0, V4);
    let stale = V4 + MAX_COMMIT_AGE + 1;
    let err = apply(
        &f,
        vec![signed(register(b"alice", 1, victim.address()), 1, &victim)],
        stale,
    )
    .unwrap_err();
    assert!(refusal(&err).contains("expired"), "{err}");
    // A refused block writes nothing, so the stale commitment is still there
    // until a block at that height commits; the end-of-block pass prunes it.
    let v = victim.address();
    assert_eq!(f.db.commitment_height(&v, &c).expect("read"), Some(V4));
    apply(&f, vec![], stale).expect("empty block");
    assert_eq!(f.db.commitment_height(&v, &c).expect("read"), None);
    // And once pruned, the reveal is refused for want of any commitment.
    let err = apply(
        &f,
        vec![signed(register(b"alice", 1, victim.address()), 1, &victim)],
        stale + 1,
    )
    .unwrap_err();
    assert!(refusal(&err).contains("no commitment"), "{err}");
}

// M-11: pruning is height-exact and every node computes the same root.
#[test]
fn an_expired_commitment_is_pruned_at_its_expiry_and_the_root_is_deterministic() {
    let key = generate_signing_key().expect("key");
    let one = fixture(&[key.address()]);
    let two = fixture(&[key.address()]);
    let expiry = V4 + MAX_COMMIT_AGE + 1;
    let mut roots = Vec::new();
    for f in [&one, &two] {
        let c = commit_at(f, &key, b"alice", 0, V4);
        // The last block at which a reveal is still accepted keeps it.
        let kept = apply(f, vec![], expiry - 1).expect("before expiry");
        let k = key.address();
        assert_eq!(f.db.commitment_height(&k, &c).expect("read"), Some(V4));
        let pruned = apply(f, vec![], expiry).expect("at expiry");
        assert_eq!(f.db.commitment_height(&k, &c).expect("read"), None);
        assert_ne!(
            kept, pruned,
            "the commitment and its index row left the root"
        );
        roots.push(pruned);
    }
    assert_eq!(roots[0], roots[1], "two nodes prune to the same root");
}

/// Writes a commitment and its expiry-index row straight to disk, in the
/// layout `state/names.rs` documents, as if committed at height 0.
fn seed_commitment(f: &Fixture, commitment: &[u8; 32], committer: &Address) {
    let mut value = 0u64.to_le_bytes().to_vec();
    value.extend_from_slice(committer);
    f.db.raw_put_for_test(
        &[&b"nm:c:"[..], &committer[..], &commitment[..]].concat(),
        &value,
    )
    .expect("seed commitment");
    let expiry = MAX_COMMIT_AGE + 1;
    f.db.raw_put_for_test(
        &[
            &b"nm:x:"[..],
            &expiry.to_be_bytes()[..],
            &committer[..],
            &commitment[..],
        ]
        .concat(),
        &[],
    )
    .expect("seed index");
}

// M-11 / invariant 33: at most MAX_COMMIT_PRUNE_PER_BLOCK per block, in key
// order, the remainder first in the next block.
#[test]
fn the_prune_cap_carries_the_remainder_to_the_next_block_in_key_order() {
    const EXTRA: usize = 5;
    let committer = [7u8; 32];
    let seeded = fixture(&[]);
    let clean = fixture(&[]);
    let mut commitments: Vec<[u8; 32]> = (0..MAX_COMMIT_PRUNE_PER_BLOCK + EXTRA)
        .map(|i| *blake3::hash(&u64::try_from(i).unwrap().to_le_bytes()).as_bytes())
        .collect();
    for c in &commitments {
        seed_commitment(&seeded, c, &committer);
    }
    commitments.sort_unstable();

    let h = V4 + MAX_COMMIT_AGE + 1;
    apply(&seeded, vec![], h).expect("first prune");
    for (i, c) in commitments.iter().enumerate() {
        let left = seeded.db.commitment_height(&committer, c).expect("read");
        if i < MAX_COMMIT_PRUNE_PER_BLOCK {
            assert_eq!(left, None, "#{i} is within the cap, in key order");
        } else {
            assert_eq!(left, Some(0), "#{i} carries to the next block");
        }
    }

    let root = apply(&seeded, vec![], h + 1).expect("second prune");
    assert!(commitments.iter().all(|c| {
        seeded
            .db
            .commitment_height(&committer, c)
            .expect("read")
            .is_none()
    }));
    // Every seeded row, index rows included, is gone: the root is the root of
    // a chain that never held a commitment.
    apply(&clean, vec![], h).expect("clean");
    assert_eq!(root, apply(&clean, vec![], h + 1).expect("clean"));
}

// M-11: a reveal deletes its index row too, so the pass never touches a
// registered name's commitment and the block is not refused.
#[test]
fn a_reveal_removes_its_expiry_row_so_the_prune_pass_finds_nothing() {
    let owner = generate_signing_key().expect("key");
    let f = fixture(&[owner.address()]);
    commit_at(&f, &owner, b"alice", 0, V4);
    apply(
        &f,
        vec![signed(register(b"alice", 1, owner.address()), 1, &owner)],
        V4 + MIN_COMMIT_AGE,
    )
    .expect("register");
    // Two empty blocks either side of the commitment's expiry. Had the reveal
    // left its index row, the second would delete it and move the root.
    let expiry = V4 + MAX_COMMIT_AGE + 1;
    let before = apply(&f, vec![], expiry - 1).expect("before");
    let after = apply(&f, vec![], expiry).expect("at expiry");
    assert_eq!(before, after);
    assert!(f.db.get_name(b"alice").expect("read").is_some());
}

// M-6
#[test]
fn set_reverse_requires_the_name_to_target_the_sender() {
    let owner = generate_signing_key().expect("key");
    let other = generate_signing_key().expect("key");
    let f = fixture(&[owner.address(), other.address()]);
    commit_at(&f, &owner, b"scam", 0, V4);
    apply(
        &f,
        vec![signed(register(b"scam", 1, other.address()), 1, &owner)],
        V4 + MIN_COMMIT_AGE,
    )
    .expect("register pointing at someone else");

    // `other` never pointed the name at itself, so it may not claim it.
    let claim = signed(update(b"scam", NameUpdate::SetReverse), 0, &other);
    let err = apply(&f, vec![claim], V4 + MIN_COMMIT_AGE + 1).unwrap_err();
    assert!(refusal(&err).contains("does not target"), "{err}");
    assert!(f.db.reverse_name(&other.address()).expect("read").is_none());
}

// M-7 (reverse record moves with the name)
#[test]
fn transfer_clears_the_old_owners_reverse_record() {
    let owner = generate_signing_key().expect("key");
    let buyer = generate_signing_key().expect("key");
    let f = fixture(&[owner.address(), buyer.address()]);
    commit_at(&f, &owner, b"alice", 0, V4);
    let h = V4 + MIN_COMMIT_AGE;
    apply(
        &f,
        vec![signed(register(b"alice", 1, owner.address()), 1, &owner)],
        h,
    )
    .expect("register");
    apply(
        &f,
        vec![signed(update(b"alice", NameUpdate::SetReverse), 2, &owner)],
        h + 1,
    )
    .expect("reverse");
    assert!(f.db.reverse_name(&owner.address()).expect("read").is_some());

    apply(
        &f,
        vec![signed(
            update(b"alice", NameUpdate::Transfer(buyer.address())),
            3,
            &owner,
        )],
        h + 2,
    )
    .expect("transfer");
    assert!(
        f.db.reverse_name(&owner.address()).expect("read").is_none(),
        "the old owner's address no longer names a label it does not own"
    );
    assert_eq!(
        f.db.get_name(b"alice").expect("read").expect("held").owner,
        buyer.address()
    );
}

// M-8
#[test]
fn only_the_owner_renews_during_grace_and_anyone_registers_after() {
    let owner = generate_signing_key().expect("key");
    let other = generate_signing_key().expect("key");
    let f = fixture(&[owner.address(), other.address()]);
    commit_at(&f, &owner, b"alice", 0, V4);
    let h = V4 + MIN_COMMIT_AGE;
    apply(
        &f,
        vec![signed(register(b"alice", 1, owner.address()), 1, &owner)],
        h,
    )
    .expect("register");
    let expiry = h + YEAR_BLOCKS;

    let during = expiry + 1;
    let err = apply(&f, vec![signed(renew(b"alice", 1), 0, &other)], during).unwrap_err();
    assert!(refusal(&err).contains("owner"), "{err}");
    apply(&f, vec![signed(renew(b"alice", 1), 2, &owner)], during).expect("owner renews in grace");

    // Lapse the renewed name past grace, then another key may register it.
    let renewed_expiry = expiry + YEAR_BLOCKS;
    let after = renewed_expiry + GRACE_BLOCKS + 1;
    let other_commit = name_commitment(b"alice", &other.address(), &SALT);
    apply(
        &f,
        vec![signed(commit(other_commit), 0, &other)],
        after - MIN_COMMIT_AGE - 1,
    )
    .expect("commit");
    apply(
        &f,
        vec![signed(register(b"alice", 1, other.address()), 1, &other)],
        after,
    )
    .expect("free after grace");
    assert_eq!(
        f.db.get_name(b"alice").expect("read").expect("held").owner,
        other.address()
    );
}

// M-9
#[test]
fn renewal_cannot_push_expiry_past_ten_years() {
    let owner = generate_signing_key().expect("key");
    let f = fixture(&[owner.address()]);
    commit_at(&f, &owner, b"alice", 0, V4);
    let h = V4 + MIN_COMMIT_AGE;
    apply(
        &f,
        vec![signed(register(b"alice", 10, owner.address()), 1, &owner)],
        h,
    )
    .expect("ten years");
    apply(&f, vec![signed(renew(b"alice", 10), 2, &owner)], h + 1).expect("renew");
    let name = f.db.get_name(b"alice").expect("read").expect("held");
    // The cap is ten years from the renewing block, not from the registration.
    assert_eq!(
        name.expires_at,
        (h + 1) + 10 * YEAR_BLOCKS,
        "capped at ten years from now"
    );
    // A renewal that the cap would turn into nothing is refused, not charged.
    let err = apply(&f, vec![signed(renew(b"alice", 1), 3, &owner)], h + 1).unwrap_err();
    assert!(refusal(&err).contains("ten years"), "{err}");
}

// M-12
#[test]
fn refused_labels_dash_a_double_dash_two_chars_64_bytes_and_non_ascii() {
    assert!(validate_label(b"abc").is_ok());
    assert!(validate_label(b"a1-b2").is_ok());
    for bad in [
        &b"ab"[..],
        b"-abc",
        b"abc-",
        b"a--c",
        b"ABC",
        b"a b",
        b"a\0c",
        "a\u{e9}c".as_bytes(),
    ] {
        assert!(validate_label(bad).is_err(), "{bad:?} must be refused");
    }
    let sixty_four = vec![b'a'; 64];
    assert!(validate_label(&sixty_four).is_err());
    assert!(validate_label(&vec![b'a'; 63]).is_ok());
}

#[test]
fn a_label_refused_at_decode_never_reaches_execution() {
    let owner = generate_signing_key().expect("key");
    let mut bytes = Vec::new();
    TxKind::Name(NameAction::Register {
        label: b"a--c".to_vec(),
        salt: SALT,
        periods: 1,
        target: owner.address(),
    })
    .encode_into(&mut bytes);
    let mut reader = custom_l1_node::core::codec::ByteReader::new(&bytes);
    assert!(TxKind::decode(&mut reader).is_err());
}

// Reserved label (ADR-135 P-3, O-3): held by governance, not by a key.
#[test]
fn the_reserved_label_cannot_be_registered_by_anyone() {
    let owner = generate_signing_key().expect("key");
    let f = fixture(&[owner.address()]);
    commit_at(&f, &owner, b"maya2c", 0, V4);
    let err = apply(
        &f,
        vec![signed(register(b"maya2c", 1, owner.address()), 1, &owner)],
        V4 + MIN_COMMIT_AGE,
    )
    .unwrap_err();
    assert!(refusal(&err).contains("reserved"), "{err}");
}

// O-3: the hold is a governed parameter, so clearing its bit releases the
// label. The table is written directly, as a passed proposal's execution
// writes it (`apply_changes`); the proposal path itself is governance's tests.
#[test]
fn a_governance_release_of_the_reserved_bit_lets_the_label_register() {
    assert_eq!(
        maya_governance::params::RESERVED_LABELS_ALL_HELD,
        (1u64 << RESERVED_LABELS.len()) - 1,
        "the governed default holds every compiled reserved label"
    );
    assert_eq!(RESERVED_LABELS[0], b"maya2c");
    let owner = generate_signing_key().expect("key");
    let f = fixture(&[owner.address()]);
    let released = ParameterTable::new().with(
        ParameterKey::NameReservedLabels,
        maya_dex::Amount::from_u64(0),
    );
    f.db.raw_put_for_test(b"g:params", &released.encode())
        .expect("released table");
    commit_at(&f, &owner, b"maya2c", 0, V4);
    apply(
        &f,
        vec![signed(register(b"maya2c", 1, owner.address()), 1, &owner)],
        V4 + MIN_COMMIT_AGE,
    )
    .expect("released by governance");
    assert_eq!(
        f.db.get_name(b"maya2c").expect("read").expect("held").owner,
        owner.address()
    );
}

// Breaker (ADR-135 P-3): names have their own module; a renewal is never
// halted, so a breaker cannot run a name out of its grace period.
#[test]
fn a_names_breaker_halts_register_and_update_but_never_renew() {
    assert!(MODULES.contains(&Module::Names));
    assert_eq!(Module::from_tag(Module::Names.tag()), Some(Module::Names));
    for kind in [
        commit([1; 32]),
        register(b"alice", 1, [2; 32]),
        update(b"alice", NameUpdate::SetReverse),
    ] {
        assert_eq!(Module::of(&kind), Some(Module::Names));
    }
    assert_eq!(Module::of(&renew(b"alice", 1)), None);

    let owner = generate_signing_key().expect("key");
    let f = fixture(&[owner.address()]);
    let tripped = custom_l1_node::state::invariant_guard::BreakerRecord {
        module: Module::Names,
        invariant: custom_l1_node::state::invariant_guard::Invariant::PoolValuePerShare,
        tripped_at: V4,
        until: V4 + 100,
    };
    f.db.raw_put_for_test(
        &custom_l1_node::state::invariant_guard::guard_key(Module::Names),
        &tripped.encode(),
    )
    .expect("trip");
    let halted = signed(commit([1; 32]), 0, &owner);
    assert!(matches!(
        apply(&f, vec![halted], V4 + 1).unwrap_err(),
        NodeError::ModuleHalted { .. }
    ));
}

#[test]
fn a_renewal_lands_while_the_names_breaker_is_tripped() {
    let owner = generate_signing_key().expect("key");
    let f = fixture(&[owner.address()]);
    commit_at(&f, &owner, b"alice", 0, V4);
    let h = V4 + MIN_COMMIT_AGE;
    apply(
        &f,
        vec![signed(register(b"alice", 1, owner.address()), 1, &owner)],
        h,
    )
    .expect("register");
    let tripped = custom_l1_node::state::invariant_guard::BreakerRecord {
        module: Module::Names,
        invariant: custom_l1_node::state::invariant_guard::Invariant::PoolValuePerShare,
        tripped_at: h,
        until: h + 100,
    };
    f.db.raw_put_for_test(
        &custom_l1_node::state::invariant_guard::guard_key(Module::Names),
        &tripped.encode(),
    )
    .expect("trip");
    apply(&f, vec![signed(renew(b"alice", 1), 2, &owner)], h + 1)
        .expect("a renewal is never halted");
    let blocked = signed(update(b"alice", NameUpdate::SetReverse), 3, &owner);
    assert!(matches!(
        apply(&f, vec![blocked], h + 2).unwrap_err(),
        NodeError::ModuleHalted { .. }
    ));
}

// Copied commitment: keyed by committer, so Mallory submitting Alice's exact
// commitment first neither blocks Alice's commit nor her reveal.
#[test]
fn a_copied_commitment_does_not_block_the_original_committer() {
    let alice = generate_signing_key().expect("key");
    let mallory = generate_signing_key().expect("key");
    let f = fixture(&[alice.address(), mallory.address()]);
    let c = name_commitment(b"alice", &alice.address(), &SALT);
    apply(&f, vec![signed(commit(c), 0, &mallory)], V4).expect("the copy lands");
    apply(&f, vec![signed(commit(c), 0, &alice)], V4 + 1).expect("alice still commits");
    apply(
        &f,
        vec![signed(register(b"alice", 1, alice.address()), 1, &alice)],
        V4 + 1 + MIN_COMMIT_AGE,
    )
    .expect("alice reveals");
    assert_eq!(
        f.db.get_name(b"alice").expect("read").expect("held").owner,
        alice.address()
    );
    // Mallory's copy is useless to her: the commitment hashes in Alice's address.
    let err = apply(
        &f,
        vec![signed(
            register(b"alice", 1, mallory.address()),
            1,
            &mallory,
        )],
        V4 + 2 + MIN_COMMIT_AGE,
    )
    .unwrap_err();
    assert!(refusal(&err).contains("no commitment"), "{err}");
}

// A commitment past expiry but not yet pruned may be made again: it can
// never be revealed, so it must not block the same sender and salt.
#[test]
fn an_expired_unpruned_commitment_can_be_made_again() {
    let owner = generate_signing_key().expect("key");
    let f = fixture(&[owner.address()]);
    let c = name_commitment(b"alice", &owner.address(), &SALT);
    // Seeded as a backlog row: due at MAX_COMMIT_AGE + 1, never pruned because
    // no block at or after that height has committed yet.
    seed_commitment(&f, &c, &owner.address());
    let again = V4 + MAX_COMMIT_AGE + 1;
    apply(&f, vec![signed(commit(c), 0, &owner)], again).expect("re-commit");
    assert_eq!(
        f.db.commitment_height(&owner.address(), &c).expect("read"),
        Some(again),
        "the old row was replaced, not pruned out from under the new one"
    );
    apply(
        &f,
        vec![signed(register(b"alice", 1, owner.address()), 1, &owner)],
        again + MIN_COMMIT_AGE,
    )
    .expect("the new commitment reveals");
}

// The Names and Nft layers sit in the proof's layer order where the root
// folds them, so an account proof verifies once they and `up:` hold rows.
#[test]
fn an_account_proof_verifies_with_names_nft_and_schedule_rows() {
    let owner = generate_signing_key().expect("key");
    let f = fixture(&[owner.address()]);
    commit_at(&f, &owner, b"alice", 0, V4);
    f.db.raw_put_for_test(b"n:c:proof-test", b"x")
        .expect("nft row");
    f.db.raw_put_for_test(b"up:proof-test", b"x")
        .expect("schedule row");
    let proof =
        f.db.account_proof(&owner.address())
            .expect("read")
            .expect("account exists");
    let layers: Vec<_> = proof.layers.iter().map(|digest| digest.layer).collect();
    use custom_l1_node::state::proof::StateLayer;
    for layer in [
        StateLayer::Names,
        StateLayer::Nft,
        StateLayer::UpgradeSchedule,
    ] {
        assert!(layers.contains(&layer), "{layer:?}");
    }
    assert!(
        proof
            .verify(&f.db.state_root().expect("root"))
            .expect("verifies")
    );
}

// M-6/M-7: retargeting a name clears the reverse record of the address it
// no longer resolves to.
#[test]
fn set_target_clears_the_old_targets_reverse_record() {
    let owner = generate_signing_key().expect("key");
    let elsewhere = [0x42; 32];
    let f = fixture(&[owner.address()]);
    commit_at(&f, &owner, b"alice", 0, V4);
    let h = V4 + MIN_COMMIT_AGE;
    apply(
        &f,
        vec![signed(register(b"alice", 1, owner.address()), 1, &owner)],
        h,
    )
    .expect("register");
    apply(
        &f,
        vec![signed(update(b"alice", NameUpdate::SetReverse), 2, &owner)],
        h + 1,
    )
    .expect("reverse");
    apply(
        &f,
        vec![signed(
            update(b"alice", NameUpdate::SetTarget(elsewhere)),
            3,
            &owner,
        )],
        h + 2,
    )
    .expect("retarget");
    assert!(f.db.reverse_name(&owner.address()).expect("read").is_none());
}

// O-3: short labels cost more than long ones; the fee burns to FEE_SINK and a
// commitment costs nothing.
#[test]
fn a_registration_burns_the_length_class_fee_and_a_commitment_is_free() {
    let short = generate_signing_key().expect("key");
    let long = generate_signing_key().expect("key");
    let f = fixture(&[short.address(), long.address()]);
    let before = balance(&f, &short.address());
    commit_at(&f, &short, b"abc", 0, V4);
    assert_eq!(
        balance(&f, &short.address()),
        before,
        "CommitName is free (O-3)"
    );

    let sink_before = balance(&f, &FEE_SINK);
    apply(
        &f,
        vec![signed(register(b"abc", 1, short.address()), 1, &short)],
        V4 + MIN_COMMIT_AGE,
    )
    .expect("register");
    let paid = balance(&f, &FEE_SINK) - sink_before;
    assert!(!paid.is_zero(), "a three-character name is not free");

    commit_at(&f, &long, b"abcdefgh", 0, V4);
    let long_before = balance(&f, &FEE_SINK);
    apply(
        &f,
        vec![signed(register(b"abcdefgh", 1, long.address()), 1, &long)],
        V4 + MIN_COMMIT_AGE,
    )
    .expect("register long");
    let long_paid = balance(&f, &FEE_SINK) - long_before;
    assert!(long_paid < paid, "a long name costs less than a short one");
    // Eric, 2026-11-12: 5+ characters is small but not free.
    assert!(!long_paid.is_zero(), "a long name is not free");
}

// Dark until bit 43 activates (ADR-135 P-3), on admission and apply alike.
#[test]
fn names_are_refused_below_activation() {
    let owner = generate_signing_key().expect("key");
    let f = fixture(&[owner.address()]);
    let err =
        f.db.apply_block(
            &block_of(vec![signed(commit([1; 32]), 0, &owner)]),
            BlockContext::at_height(V4 - 1).with_names_activation(V4),
        )
        .unwrap_err();
    assert!(matches!(err, NodeError::NameInactive { .. }), "{err}");
}

// 11
#[test]
fn a_reorg_restores_owner_expiry_and_the_consumed_commitment() {
    let owner = generate_signing_key().expect("key");
    let f = fixture(&[owner.address()]);
    let c = commit_at(&f, &owner, b"alice", 0, V4);
    let h = V4 + MIN_COMMIT_AGE;
    let root_before = f.db.state_root().expect("root");

    let mut block = block_of(vec![signed(
        register(b"alice", 1, owner.address()),
        1,
        &owner,
    )]);
    block.header.state_root = f.db.preview_root(&block, at(h)).expect("preview");
    let id = [9u8; 32];
    f.db.apply_block_journaled(&block, &id, at(h))
        .expect("register");
    assert!(f.db.get_name(b"alice").expect("read").is_some());
    f.db.revert_block(&id).expect("revert");

    assert!(f.db.get_name(b"alice").expect("read").is_none());
    assert_eq!(
        f.db.commitment_height(&owner.address(), &c).expect("read"),
        Some(V4)
    );
    assert_eq!(f.db.state_root().expect("root"), root_before);
}

// 12
#[test]
fn the_root_is_unchanged_when_no_name_exists() {
    let key = generate_signing_key().expect("key");
    let dark = fixture(&[key.address()]);
    let lit = fixture(&[key.address()]);
    let empty = block_of(vec![]);
    let a = dark
        .db
        .apply_block(&empty, BlockContext::at_height(V4))
        .expect("dark");
    let b = lit.db.apply_block(&empty, at(V4)).expect("lit");
    assert_eq!(a, b);
}
