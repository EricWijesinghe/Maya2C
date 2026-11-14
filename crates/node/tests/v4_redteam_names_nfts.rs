//! Red team for names and unique assets (ADR-135 P-2 bit 42, P-3 bit 43;
//! V4-V1). Each test is one attack attempt against real block execution,
//! named for the attack, asserting the refusal and that nothing moved. The
//! threat-model replays (M-1..M-12, N-1..) are in `names_tests.rs` and
//! `nft_tests.rs`; this file is the attacks that cross into other rules:
//! stateless verification, fees, nonces and lapsed records.
//!
//! Classes tried (V4-V1 brief): value creation, fee bypass, unbounded
//! per-block work, replay, squatting. Per-block work is pinned by
//! `names_tests::the_prune_cap_carries_the_remainder_to_the_next_block_in_key_order`
//! (the one end-of-block pass). Executing a form reads and writes a fixed
//! number of records, no loop, but the block builder's filter (`StateDB::select_applicable_within`) copies the
//! whole overlay for every frame that is not a plain transfer, so a block full
//! of v4 forms (or sponsored transfers) costs every builder work quadratic in
//! the state the block touches. Open as M3 (BACKLOG, v4 activation blockers):
//! one benchmark over a block of v4 forms before any v4 activation height is
//! written.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use custom_l1_node::core::name_payload::{NameAction, NameUpdate};
use custom_l1_node::core::nft_payload::NftAction;
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::names::{
    GRACE_BLOCKS, MAX_COMMIT_AGE, MIN_COMMIT_AGE, YEAR_BLOCKS, name_commitment,
};
use custom_l1_node::state::nft::collection_id;
use custom_l1_node::state::shielded::FEE_SINK;
use custom_l1_node::state::{
    Account, Address, BlockContext, StateDB, StatelessError, verify_block,
};
use primitive_types::U256;
use tempfile::TempDir;

mod common;

const V4: u64 = 1;
const SALT: [u8; 32] = [0x6B; 32];
/// Far above any name fee here (the 5+ class is 10^21 a year by default).
const FUNDS: u128 = 1_000_000_000_000_000_000_000_000_000;
const FILLERS: u8 = 64;

/// Bits 42 and 43 from [`V4`], and the stateless switch from height 1.
fn at(height: u64) -> BlockContext {
    BlockContext::at_height(height)
        .with_names_activation(V4)
        .with_nft_activation(V4)
        .with_stateless_activation(1)
}

fn key(seed: u8) -> HybridSigningKey {
    signing_key_from_seed(&[seed; 32]).expect("key")
}

struct Net {
    db: StateDB,
    tip: BlockHeader,
    _dir: TempDir,
}

impl Net {
    /// Funds `accounts` and enough fillers to keep a witness non-trivial.
    fn new(accounts: &[Address]) -> Self {
        let dir = TempDir::new().unwrap();
        let db = StateDB::open(dir.path()).unwrap();
        common::bind(&db);
        let fillers = (0..FILLERS).map(|i| *blake3::hash(&[0xF2, i]).as_bytes());
        for a in accounts.iter().copied().chain(fillers) {
            db.put_account(
                &a,
                &Account {
                    balance: U256::from(FUNDS),
                    nonce: 0,
                },
            )
            .unwrap();
        }
        let tip = BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: db.state_root().unwrap(),
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        };
        Self { db, tip, _dir: dir }
    }

    /// A block on the tip at `height` declaring the full node's root; the
    /// full node's refusal if it would not apply.
    fn build(&self, transactions: Vec<Transaction>, height: u64) -> Result<Block, NodeError> {
        let draft = Block::new(
            BlockHeader {
                version: 0,
                prev_hash: self.tip.id(),
                state_root: [0; 32],
                timestamp: self.tip.timestamp + 1,
                nonce: 0,
                difficulty_target: self.tip.difficulty_target,
                tx_root: [0; 32],
            },
            transactions,
        );
        let root = self.db.preview_root(&draft, at(height))?;
        Ok(Block::new(
            BlockHeader {
                state_root: root,
                ..draft.header
            },
            draft.transactions,
        ))
    }

    fn apply(&mut self, transactions: Vec<Transaction>, height: u64) -> Result<(), NodeError> {
        let block = self.build(transactions, height)?;
        self.db
            .apply_block_journaled(&block, &block.header.id(), at(height))?;
        self.tip = block.header;
        Ok(())
    }
}

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key, &common::test_chain()).unwrap();
    tx
}

fn commit(label: &[u8], who: &HybridSigningKey) -> TxKind {
    TxKind::Name(NameAction::Commit {
        commitment: name_commitment(label, &who.address(), &SALT),
    })
}

fn register(label: &[u8], target: Address) -> TxKind {
    TxKind::Name(NameAction::Register {
        label: label.to_vec(),
        salt: SALT,
        periods: 1,
        target,
    })
}

fn set_reverse(label: &[u8]) -> TxKind {
    TxKind::Name(NameAction::Update {
        label: label.to_vec(),
        op: NameUpdate::SetReverse,
    })
}

fn pay(from: &HybridSigningKey, nonce: u64, to: Address, amount: u64) -> Transaction {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: U256::from(amount),
            recipient: to,
        }],
        nonce,
    );
    tx.sign(from, &common::test_chain()).unwrap();
    tx
}

fn balance(n: &Net, who: &Address) -> U256 {
    n.db.get_account(who).unwrap().balance
}

// FINDING (fixed in this change): `settle_names` prunes expired commitments
// at the end of a block that names none of them, so the names layer is an
// end-of-block pass layer. Left out of `PASS_LAYERS`, a stateless node
// folded the parent's names digest into a transfer-only block's root and
// called an honest block Invalid. It must say Unverifiable instead.
#[test]
fn a_transfer_block_that_prunes_a_commitment_is_unverifiable_statelessly_not_invalid() {
    let (alice, bob) = (key(0x41), key(0x42));
    let mut n = Net::new(&[alice.address(), bob.address()]);
    n.apply(vec![], 1).expect("the stateless switch");
    n.apply(vec![signed(commit(b"alice", &alice), 0, &alice)], 2)
        .expect("commit");
    // The first height at which the commitment is refused: the prune is due.
    let due = 2 + MAX_COMMIT_AGE + 1;
    let parent = n.db.state_root().unwrap();
    let block = n
        .build(vec![pay(&bob, 0, alice.address(), 1)], due)
        .expect("an honest block");
    let witness = n.db.block_witness(&block).expect("witness");
    match verify_block(&parent, due, &block, witness, &common::test_chain()) {
        Err(StatelessError::Unverifiable(reason)) => {
            assert!(reason.contains("end-of-block pass"), "{reason}");
        }
        other => panic!("an honest block must not be judged: {other:?}"),
    }
    // And the full node does prune it in that block.
    n.apply(vec![pay(&bob, 0, alice.address(), 1)], due)
        .expect("the honest block applies");
    let c = name_commitment(b"alice", &alice.address(), &SALT);
    assert_eq!(n.db.commitment_height(&alice.address(), &c).unwrap(), None);
}

// Squatting through a stale reverse record: after a name lapses and a
// stranger registers it, the old target's reverse record is gone, so a
// wallet never shows the stranger's name for the old target's address, and
// the stranger cannot set a reverse record on an address that is not his.
#[test]
fn a_lapsed_name_reregistered_by_a_stranger_carries_no_old_reverse_record() {
    let (alice, mallory) = (key(0x43), key(0x44));
    let mut n = Net::new(&[alice.address(), mallory.address()]);
    n.apply(vec![signed(commit(b"alice", &alice), 0, &alice)], 2)
        .unwrap();
    let h = 2 + MIN_COMMIT_AGE;
    n.apply(
        vec![signed(register(b"alice", alice.address()), 1, &alice)],
        h,
    )
    .unwrap();
    n.apply(vec![signed(set_reverse(b"alice"), 2, &alice)], h + 1)
        .unwrap();
    assert!(n.db.reverse_name(&alice.address()).unwrap().is_some());

    let lapsed = h + YEAR_BLOCKS + GRACE_BLOCKS + 1;
    n.apply(
        vec![signed(commit(b"alice", &mallory), 0, &mallory)],
        lapsed,
    )
    .unwrap();
    let take = lapsed + MIN_COMMIT_AGE;
    // Mallory points the name at Alice, hoping her old reverse record vouches.
    n.apply(
        vec![signed(register(b"alice", alice.address()), 1, &mallory)],
        take,
    )
    .expect("a lapsed name is free");
    assert_eq!(n.db.reverse_name(&alice.address()).unwrap(), None);
    let err = n
        .apply(vec![signed(set_reverse(b"alice"), 2, &mallory)], take + 1)
        .unwrap_err();
    assert!(err.to_string().contains("does not target"), "{err}");
    let name = n.db.get_name(b"alice").unwrap().unwrap();
    assert_eq!(name.owner, mallory.address());
}

// Fee bypass: a reveal from an account that cannot pay the length-class fee
// is refused whole: no record, the commitment survives, nothing burned.
#[test]
fn a_reveal_that_cannot_pay_the_name_fee_is_refused_and_the_commitment_survives() {
    let (rich, poor) = (key(0x45), key(0x46));
    let mut n = Net::new(&[rich.address()]);
    // A balance that covers no 3-byte name (10^24 a year by default).
    n.apply(vec![pay(&rich, 0, poor.address(), 1_000)], 2)
        .unwrap();
    n.apply(vec![signed(commit(b"abc", &poor), 0, &poor)], 3)
        .unwrap();
    let sink = balance(&n, &FEE_SINK);
    let err = n
        .apply(
            vec![signed(register(b"abc", poor.address()), 1, &poor)],
            3 + MIN_COMMIT_AGE,
        )
        .unwrap_err();
    assert!(
        matches!(err, NodeError::InsufficientBalance { .. }),
        "{err:?}"
    );
    assert_eq!(n.db.get_name(b"abc").unwrap(), None);
    let c = name_commitment(b"abc", &poor.address(), &SALT);
    assert_eq!(
        n.db.commitment_height(&poor.address(), &c).unwrap(),
        Some(3)
    );
    assert_eq!(balance(&n, &FEE_SINK), sink);
    assert_eq!(balance(&n, &poor.address()), U256::from(1_000));
}

// Replay: a registration and a commitment, re-offered after they applied,
// are refused on their nonces; the name is registered and paid for once.
#[test]
fn a_replayed_commit_or_registration_is_refused_on_its_nonce() {
    let alice = key(0x47);
    let mut n = Net::new(&[alice.address()]);
    let c = signed(commit(b"alice", &alice), 0, &alice);
    n.apply(vec![c.clone()], 2).unwrap();
    let r = signed(register(b"alice", alice.address()), 1, &alice);
    let h = 2 + MIN_COMMIT_AGE;
    n.apply(vec![r.clone()], h).unwrap();
    let paid = balance(&n, &FEE_SINK);
    for tx in [c, r] {
        let err = n.apply(vec![tx], h + 1).unwrap_err();
        assert!(matches!(err, NodeError::InvalidNonce { .. }), "{err:?}");
    }
    assert_eq!(balance(&n, &FEE_SINK), paid, "the fee burned once");
}

// Resurrection: a burned item can be neither transferred nor burned again,
// and its owner-index row stays gone.
#[test]
fn a_burned_item_cannot_be_transferred_or_burned_again() {
    let (creator, buyer) = (key(0x48), key(0x49));
    let mut n = Net::new(&[creator.address(), buyer.address()]);
    let create = TxKind::Nft(NftAction::CreateCollection {
        max_items: 2,
        metadata_hash: [0xA1; 32],
    });
    n.apply(vec![signed(create, 0, &creator)], 2).unwrap();
    let id = collection_id(&creator.address(), 0);
    let mint = TxKind::Nft(NftAction::MintItem {
        collection: id,
        metadata_hash: [0xB2; 32],
    });
    n.apply(vec![signed(mint, 1, &creator)], 3).unwrap();
    let burn = TxKind::Nft(NftAction::BurnItem {
        collection: id,
        index: 0,
    });
    n.apply(vec![signed(burn.clone(), 2, &creator)], 4).unwrap();

    let transfer = TxKind::Nft(NftAction::TransferItem {
        collection: id,
        index: 0,
        to: buyer.address(),
    });
    let err = n.apply(vec![signed(transfer, 3, &creator)], 5).unwrap_err();
    assert!(err.to_string().contains("no such item"), "{err}");
    let err = n.apply(vec![signed(burn, 3, &creator)], 5).unwrap_err();
    assert!(err.to_string().contains("no such item"), "{err}");
    assert_eq!(n.db.get_nft_item(&id, 0).unwrap(), None);
    assert!(
        !n.db
            .nft_owner_index_contains(&creator.address(), &id, 0)
            .unwrap()
    );
    assert!(
        !n.db
            .nft_owner_index_contains(&buyer.address(), &id, 0)
            .unwrap()
    );
}

// Replay across owners: A sells to B, B returns it to A; A's original signed
// transfer to B cannot be replayed to take it back to B.
#[test]
fn a_replayed_item_transfer_cannot_move_the_item_again() {
    let (a, b) = (key(0x4A), key(0x4B));
    let mut n = Net::new(&[a.address(), b.address()]);
    let create = TxKind::Nft(NftAction::CreateCollection {
        max_items: 1,
        metadata_hash: [0xA1; 32],
    });
    n.apply(vec![signed(create, 0, &a)], 2).unwrap();
    let id = collection_id(&a.address(), 0);
    let mint = TxKind::Nft(NftAction::MintItem {
        collection: id,
        metadata_hash: [0xB2; 32],
    });
    n.apply(vec![signed(mint, 1, &a)], 3).unwrap();
    let to = |who: Address| {
        TxKind::Nft(NftAction::TransferItem {
            collection: id,
            index: 0,
            to: who,
        })
    };
    let sale = signed(to(b.address()), 2, &a);
    n.apply(vec![sale.clone()], 4).unwrap();
    n.apply(vec![signed(to(a.address()), 0, &b)], 5).unwrap();
    let err = n.apply(vec![sale], 6).unwrap_err();
    assert!(matches!(err, NodeError::InvalidNonce { .. }), "{err:?}");
    let item = n.db.get_nft_item(&id, 0).unwrap().unwrap();
    assert_eq!(item.owner, a.address());
}
