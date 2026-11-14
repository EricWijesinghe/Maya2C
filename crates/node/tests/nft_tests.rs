//! Unique assets (ADR-135 P-2, feature bit 42) against real block execution:
//! the replay tests `docs/threat-models/v4-nfts.md` lists, one per attack.
//!
//! Every refusal asserts its specific reason, so a stub that refuses
//! everything fails these tests rather than passing them. Written before the
//! implementation (TDD); CI is the first compile of this file.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::core::nft_payload::NftAction;
use custom_l1_node::core::vault_payload::{VaultAction, VaultConfig};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::invariant_guard::{MODULES, Module};
use custom_l1_node::state::nft::{
    COLLECTION_RECORD_LEN, ITEM_RECORD_LEN, NftCollection, NftItem, collection_id,
};
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use primitive_types::U256;
use tempfile::TempDir;

mod common;

/// The v4 activation height these tests run under.
const V4: u64 = 10;

const HASH_A: [u8; 32] = [0xA1; 32];
const HASH_B: [u8; 32] = [0xB2; 32];

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
                balance: U256::from(1_000_000u64),
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

/// A context at `height` with bit 42 active from [`V4`].
fn at(height: u64) -> BlockContext {
    BlockContext::at_height(height).with_nft_activation(V4)
}

fn create(max_items: u64, metadata_hash: [u8; 32]) -> TxKind {
    TxKind::Nft(NftAction::CreateCollection {
        max_items,
        metadata_hash,
    })
}

fn mint_item(collection: [u8; 32], metadata_hash: [u8; 32]) -> TxKind {
    TxKind::Nft(NftAction::MintItem {
        collection,
        metadata_hash,
    })
}

fn transfer_item(collection: [u8; 32], index: u64, to: Address) -> TxKind {
    TxKind::Nft(NftAction::TransferItem {
        collection,
        index,
        to,
    })
}

fn burn_item(collection: [u8; 32], index: u64) -> TxKind {
    TxKind::Nft(NftAction::BurnItem { collection, index })
}

fn apply(f: &Fixture, txs: Vec<Transaction>, height: u64) -> Result<[u8; 32], NodeError> {
    f.db.apply_block(&block_of(txs), at(height))
}

fn item(f: &Fixture, collection: &[u8; 32], index: u64) -> Option<NftItem> {
    f.db.get_nft_item(collection, index).expect("read")
}

fn refusal(error: &NodeError) -> &'static str {
    match error {
        NodeError::NftRefused { reason } => *reason,
        other => panic!("expected NftRefused, got {other:?}"),
    }
}

/// A creator with one collection at nonce 0, `max_items` long.
struct Creator {
    f: Fixture,
    key: HybridSigningKey,
    creator: Address,
    collection: [u8; 32],
}

fn collection_of(max_items: u64) -> Creator {
    let key = generate_signing_key().expect("key");
    let creator = key.address();
    let f = fixture(&[creator]);
    apply(&f, vec![signed(create(max_items, HASH_A), 0, &key)], V4).expect("create");
    let collection = collection_id(&creator, 0);
    Creator {
        f,
        key,
        creator,
        collection,
    }
}

// N-1
#[test]
fn a_transfer_by_a_non_owner_is_refused() {
    let c = collection_of(10);
    apply(
        &c.f,
        vec![signed(mint_item(c.collection, HASH_A), 1, &c.key)],
        V4 + 1,
    )
    .expect("mint");

    let thief = generate_signing_key().expect("key");
    let f = &c.f;
    let steal = signed(transfer_item(c.collection, 0, thief.address()), 0, &thief);
    let err = apply(f, vec![steal], V4 + 2).unwrap_err();
    assert!(refusal(&err).contains("does not own"), "{err}");
    assert_eq!(
        item(f, &c.collection, 0).expect("still minted").owner,
        c.creator
    );
}

// N-1 (collection-scoped key)
#[test]
fn an_item_index_in_another_collection_does_not_authorise_a_transfer() {
    let c = collection_of(10);
    apply(
        &c.f,
        vec![signed(mint_item(c.collection, HASH_A), 1, &c.key)],
        V4 + 1,
    )
    .expect("mint");
    // A second collection by the same creator, at nonce 1, holds no item 0.
    apply(&c.f, vec![signed(create(10, HASH_B), 2, &c.key)], V4 + 2).expect("second");
    let other = collection_id(&c.creator, 2);
    assert!(item(&c.f, &other, 0).is_none());

    let spend = signed(transfer_item(other, 0, [9; 32]), 3, &c.key);
    let err = apply(&c.f, vec![spend], V4 + 3).unwrap_err();
    assert!(refusal(&err).contains("no such item"), "{err}");
    assert_eq!(
        item(&c.f, &c.collection, 0).expect("untouched").owner,
        c.creator
    );
}

// N-2
#[test]
fn a_burn_then_mint_never_reuses_an_index() {
    let c = collection_of(10);
    apply(
        &c.f,
        vec![signed(mint_item(c.collection, HASH_A), 1, &c.key)],
        V4 + 1,
    )
    .expect("mint 0");
    apply(
        &c.f,
        vec![signed(burn_item(c.collection, 0), 2, &c.key)],
        V4 + 2,
    )
    .expect("burn 0");
    assert!(item(&c.f, &c.collection, 0).is_none());

    apply(
        &c.f,
        vec![signed(mint_item(c.collection, HASH_B), 3, &c.key)],
        V4 + 3,
    )
    .expect("mint");
    assert!(
        item(&c.f, &c.collection, 0).is_none(),
        "index 0 stays burnt"
    );
    let fresh = item(&c.f, &c.collection, 1).expect("the new item is index 1");
    assert_eq!(fresh.metadata_hash, HASH_B);
}

// N-3
#[test]
fn a_transfer_moves_the_owner_index_row_in_the_same_write() {
    let c = collection_of(10);
    apply(
        &c.f,
        vec![signed(mint_item(c.collection, HASH_A), 1, &c.key)],
        V4 + 1,
    )
    .expect("mint");
    let buyer = generate_signing_key().expect("key");
    let to = buyer.address();
    apply(
        &c.f,
        vec![signed(transfer_item(c.collection, 0, to), 2, &c.key)],
        V4 + 2,
    )
    .expect("transfer");

    assert_eq!(item(&c.f, &c.collection, 0).expect("item").owner, to);
    assert!(
        c.f.db
            .nft_owner_index_contains(&to, &c.collection, 0)
            .expect("read")
    );
    assert!(
        !c.f.db
            .nft_owner_index_contains(&c.creator, &c.collection, 0)
            .expect("read"),
        "the old owner's row is gone in the same write, not left behind"
    );
}

// N-4
#[test]
fn two_mints_in_one_block_at_max_minus_one_apply_one_and_refuse_one() {
    let c = collection_of(2);
    apply(
        &c.f,
        vec![signed(mint_item(c.collection, HASH_A), 1, &c.key)],
        V4 + 1,
    )
    .expect("mint 1");

    // minted is now max - 1. Two mints in one block: the first takes the last
    // slot and the second must see it full, so the whole block is refused and
    // nothing is written.
    let root_before = c.f.db.state_root().expect("root");
    let txs = vec![
        signed(mint_item(c.collection, HASH_B), 2, &c.key),
        signed(mint_item(c.collection, HASH_B), 3, &c.key),
    ];
    let err = apply(&c.f, txs, V4 + 2).unwrap_err();
    assert!(refusal(&err).contains("full"), "{err}");
    assert!(item(&c.f, &c.collection, 1).is_none());
    assert_eq!(c.f.db.state_root().expect("root"), root_before);
}

// N-4 (bound)
#[test]
fn a_mint_in_a_full_collection_is_refused() {
    let c = collection_of(1);
    apply(
        &c.f,
        vec![signed(mint_item(c.collection, HASH_A), 1, &c.key)],
        V4 + 1,
    )
    .expect("mint");
    let err = apply(
        &c.f,
        vec![signed(mint_item(c.collection, HASH_A), 2, &c.key)],
        V4 + 2,
    )
    .unwrap_err();
    assert!(refusal(&err).contains("full"), "{err}");
}

// N-4 (sender check on mint)
#[test]
fn only_the_creator_mints_into_a_collection() {
    let c = collection_of(10);
    let other = generate_signing_key().expect("key");
    let f = fixture(&[other.address()]);
    apply(&f, vec![signed(create(10, HASH_A), 0, &other)], V4).expect("own collection");
    let err = apply(
        &c.f,
        vec![signed(mint_item(c.collection, HASH_A), 0, &other)],
        V4 + 1,
    )
    .unwrap_err();
    assert!(refusal(&err).contains("creator"), "{err}");
}

// N-5
#[test]
fn a_collection_id_is_fixed_width_and_bound_to_creator_and_nonce() {
    let a = [1u8; 32];
    let b = [2u8; 32];
    assert_ne!(collection_id(&a, 0), collection_id(&b, 0));
    assert_ne!(collection_id(&a, 0), collection_id(&a, 1));
    assert_eq!(collection_id(&a, 7), collection_id(&a, 7));
}

// N-8
#[test]
fn a_collection_larger_than_the_index_bound_is_refused() {
    let key = generate_signing_key().expect("key");
    let f = fixture(&[key.address()]);
    let err = apply(
        &f,
        vec![signed(create(u64::from(u32::MAX) + 2, HASH_A), 0, &key)],
        V4,
    )
    .unwrap_err();
    assert!(refusal(&err).contains("max_items"), "{err}");
    let zero = apply(&f, vec![signed(create(0, HASH_A), 0, &key)], V4).unwrap_err();
    assert!(refusal(&zero).contains("max_items"), "{zero}");
}

// N-3 / N-2 under reorg
#[test]
fn a_reorg_restores_owner_minted_and_the_owner_index() {
    let c = collection_of(10);
    apply(
        &c.f,
        vec![signed(mint_item(c.collection, HASH_A), 1, &c.key)],
        V4 + 1,
    )
    .expect("mint");
    let root_before = c.f.db.state_root().expect("root");

    let buyer = generate_signing_key().expect("key");
    let mut block = block_of(vec![signed(
        transfer_item(c.collection, 0, buyer.address()),
        2,
        &c.key,
    )]);
    block.header.state_root = c.f.db.preview_root(&block, at(V4 + 2)).expect("preview");
    let id = [7u8; 32];
    c.f.db
        .apply_block_journaled(&block, &id, at(V4 + 2))
        .expect("transfer");
    c.f.db.revert_block(&id).expect("revert");

    assert_eq!(item(&c.f, &c.collection, 0).expect("back").owner, c.creator);
    assert!(
        c.f.db
            .nft_owner_index_contains(&c.creator, &c.collection, 0)
            .expect("read")
    );
    assert!(
        !c.f.db
            .nft_owner_index_contains(&buyer.address(), &c.collection, 0)
            .expect("read")
    );
    assert_eq!(c.f.db.state_root().expect("root"), root_before);
}

// N-10
#[test]
fn an_nft_transfer_from_a_vault_is_refused() {
    let c = collection_of(10);
    apply(
        &c.f,
        vec![signed(mint_item(c.collection, HASH_A), 1, &c.key)],
        V4 + 1,
    )
    .expect("mint");
    // Configure the creator's account as a vault (ADR-030), then try to move an item.
    let guardian = generate_signing_key().expect("key");
    let config = VaultAction::Configure(VaultConfig {
        delay_blocks: 10,
        limit: U256::from(100),
        guardians: vec![guardian.address()],
    });
    apply(
        &c.f,
        vec![signed(TxKind::Vault(Box::new(config)), 2, &c.key)],
        V4 + 2,
    )
    .expect("vault");
    let err = apply(
        &c.f,
        vec![signed(transfer_item(c.collection, 0, [9; 32]), 3, &c.key)],
        V4 + 3,
    )
    .unwrap_err();
    assert!(matches!(err, NodeError::Vault(_)), "{err}");
    assert_eq!(
        item(&c.f, &c.collection, 0).expect("untouched").owner,
        c.creator
    );
}

// N-9
#[test]
fn an_nft_breaker_halts_mint_and_transfer_but_not_a_native_transfer() {
    assert!(MODULES.contains(&Module::Nft));
    let c = collection_of(10);
    apply(
        &c.f,
        vec![signed(mint_item(c.collection, HASH_A), 1, &c.key)],
        V4 + 1,
    )
    .expect("mint");
    for kind in [
        create(1, HASH_A),
        mint_item(c.collection, HASH_A),
        transfer_item(c.collection, 0, [9; 32]),
        burn_item(c.collection, 0),
    ] {
        assert_eq!(Module::of(&kind), Some(Module::Nft));
    }
    let tripped = custom_l1_node::state::invariant_guard::BreakerRecord {
        module: Module::Nft,
        invariant: custom_l1_node::state::invariant_guard::Invariant::PoolValuePerShare,
        tripped_at: V4,
        until: V4 + 100,
    };
    c.f.db
        .raw_put_for_test(
            &custom_l1_node::state::invariant_guard::guard_key(Module::Nft),
            &tripped.encode(),
        )
        .expect("trip");
    let halted = signed(transfer_item(c.collection, 0, [9; 32]), 2, &c.key);
    assert!(matches!(
        apply(&c.f, vec![halted], V4 + 2).unwrap_err(),
        NodeError::ModuleHalted { .. }
    ));
    // A native transfer belongs to no module, so the same breaker does not stop it.
    assert_eq!(Module::of(&TxKind::Transfer), None);
}

// 10 (root)
#[test]
fn the_root_is_unchanged_when_no_collection_exists() {
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

// Metadata-hash-only rule (ADR-135 P-2, threat model N-7): nothing but a
// 32-byte hash is stored per item and per collection.
#[test]
fn the_item_and_collection_records_hold_a_hash_and_nothing_else() {
    assert_eq!(
        ITEM_RECORD_LEN, 65,
        "owner(32) + metadata_hash(32) + version(1)"
    );
    assert_eq!(
        COLLECTION_RECORD_LEN, 81,
        "creator(32) + max_items(8) + minted(8) + metadata_hash(32) + version(1)"
    );
    let c = collection_of(10);
    let stored =
        c.f.db
            .get_nft_collection(&c.collection)
            .expect("read")
            .expect("exists");
    assert_eq!(
        stored,
        NftCollection {
            creator: c.creator,
            max_items: 10,
            minted: 0,
            metadata_hash: HASH_A,
        }
    );
    assert_eq!(stored.encode().len(), COLLECTION_RECORD_LEN);
    apply(
        &c.f,
        vec![signed(mint_item(c.collection, HASH_B), 1, &c.key)],
        V4 + 1,
    )
    .expect("mint");
    let minted = item(&c.f, &c.collection, 0).expect("item");
    assert_eq!(minted.encode().len(), ITEM_RECORD_LEN);
    assert_eq!(minted.metadata_hash, HASH_B);
}
