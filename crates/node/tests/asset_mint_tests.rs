//! Capped-asset mint and burn (ADR-135 P-1, feature bit 41) against real block
//! execution: the replay tests `docs/threat-models/v4-tokens.md` lists, one per
//! attack, each named for what it pins.
//!
//! Every refusal asserts the specific error, so a stub that refuses everything
//! fails these tests rather than passing them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::core::asset_mint_payload::AssetMintAction;
use custom_l1_node::core::dex_payload::{AssetRegistration, AssetTransfer};
use custom_l1_node::core::multisig_tx::{MultisigAuth, multisig_address};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::network::mempool::Mempool;
use custom_l1_node::state::asset::{AssetRecordV2, NO_AUTHORITY};
use custom_l1_node::state::invariant_guard::{BreakerRecord, Invariant, Module, guard_key};
use custom_l1_node::state::{Account, Address, AssetId, BlockContext, StateDB, derive_asset_id};
use maya_crypto_pq::multisig::{Approval, MultisigPolicy, PolicyKey};
use maya_crypto_pq::suite::{MasterSeed, MlDsa87, SignatureSuite};
use primitive_types::U256;
use tempfile::TempDir;

mod common;

/// The v4 activation height these tests run under.
const V4: u64 = 10;

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

/// A context at `height` with bit 41 active from [`V4`].
fn at(height: u64) -> BlockContext {
    BlockContext::at_height(height).with_asset_mint_activation(V4)
}

const SYMBOL: [u8; 8] = *b"CAP\0\0\0\0\0";

fn register(supply: u64, cap: U256, authority: Address) -> TxKind {
    TxKind::AssetMint(AssetMintAction::RegisterCapped {
        symbol: SYMBOL,
        total_supply: U256::from(supply),
        cap,
        authority,
    })
}

fn mint(asset: AssetId, to: Address, amount: U256) -> TxKind {
    TxKind::AssetMint(AssetMintAction::Mint { asset, to, amount })
}

fn burn(asset: AssetId, amount: U256) -> TxKind {
    TxKind::AssetMint(AssetMintAction::Burn { asset, amount })
}

fn renounce(asset: AssetId) -> TxKind {
    TxKind::AssetMint(AssetMintAction::Renounce { asset })
}

fn apply(f: &Fixture, txs: Vec<Transaction>, height: u64) -> Result<[u8; 32], NodeError> {
    f.db.apply_block(&block_of(txs), at(height))
}

fn record(f: &Fixture, asset: &AssetId) -> AssetRecordV2 {
    f.db.get_capped_asset(asset)
        .expect("read")
        .expect("registered")
}

fn balance(f: &Fixture, asset: &AssetId, holder: &Address) -> U256 {
    f.db.asset_balance(asset, holder).expect("read")
}

/// An issuer with a capped asset registered at nonce 0, supply 100, cap 1000.
struct Issued {
    f: Fixture,
    key: HybridSigningKey,
    issuer: Address,
    asset: AssetId,
}

fn issued() -> Issued {
    let key = generate_signing_key().expect("key");
    let issuer = key.address();
    let f = fixture(&[issuer]);
    let reg = signed(register(100, U256::from(1_000), issuer), 0, &key);
    apply(&f, vec![reg], V4).expect("register");
    let asset = derive_asset_id(&issuer, 0, &SYMBOL);
    Issued {
        f,
        key,
        issuer,
        asset,
    }
}

fn refusal(error: &NodeError) -> &'static str {
    match error {
        NodeError::AssetMintRefused { reason } => reason,
        other => panic!("expected AssetMintRefused, got {other:?}"),
    }
}

// 1 (T-1)
#[test]
fn a_mint_whose_sum_wraps_past_u256_max_is_refused() {
    let s = issued();
    let tx = signed(mint(s.asset, s.issuer, U256::MAX), 1, &s.key);
    let err = apply(&s.f, vec![tx], V4 + 1).unwrap_err();
    assert!(refusal(&err).contains("overflows"), "{err}");
    assert_eq!(record(&s.f, &s.asset).base.total_supply, U256::from(100));
}

// 2 (T-1, bound)
#[test]
fn a_mint_at_exactly_the_cap_is_applied_and_one_over_is_refused() {
    let s = issued();
    let over = signed(mint(s.asset, s.issuer, U256::from(901)), 1, &s.key);
    let err = apply(&s.f, vec![over], V4 + 1).unwrap_err();
    assert!(refusal(&err).contains("above the cap"), "{err}");

    let exact = signed(mint(s.asset, s.issuer, U256::from(900)), 1, &s.key);
    apply(&s.f, vec![exact], V4 + 1).expect("mint to the cap");
    let r = record(&s.f, &s.asset);
    assert_eq!(r.base.total_supply, U256::from(1_000));
    assert_eq!(balance(&s.f, &s.asset, &s.issuer), U256::from(1_000));

    let one = signed(mint(s.asset, s.issuer, U256::one()), 2, &s.key);
    assert!(
        apply(&s.f, vec![one], V4 + 2).is_err(),
        "nothing past the cap"
    );
}

// 3 (T-3)
#[test]
fn a_reorg_that_drops_a_renounce_restores_the_authority_and_cap() {
    let s = issued();
    let root_before = s.f.db.state_root().expect("root");
    let before = record(&s.f, &s.asset);

    let mut block = block_of(vec![signed(renounce(s.asset), 1, &s.key)]);
    block.header.state_root = s.f.db.preview_root(&block, at(V4 + 1)).expect("preview");
    let id = [5u8; 32];
    s.f.db
        .apply_block_journaled(&block, &id, at(V4 + 1))
        .expect("renounce");
    let renounced = record(&s.f, &s.asset);
    assert_eq!(renounced.authority, NO_AUTHORITY);
    assert_eq!(renounced.cap, U256::from(100));
    assert_eq!(
        s.f.db.uncovered_keys().expect("scan"),
        Vec::<Vec<u8>>::new(),
        "the v2 record is under the root (invariant 25)"
    );

    s.f.db.revert_block(&id).expect("revert");
    assert_eq!(record(&s.f, &s.asset), before);
    assert_eq!(s.f.db.state_root().expect("root"), root_before);

    // And the restored authority can mint again, on the surviving branch.
    let again = signed(mint(s.asset, s.issuer, U256::from(5)), 1, &s.key);
    apply(&s.f, vec![again], V4 + 1).expect("authority restored");
}

// 4 (T-4)
#[test]
fn a_multisig_member_below_threshold_cannot_mint_as_the_authority() {
    let members: Vec<_> = (1..=3u8)
        .map(|i| MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([i; 32])))
        .collect();
    let policy = MultisigPolicy::new(
        2,
        members
            .iter()
            .map(|k| PolicyKey {
                suite: MlDsa87::ID,
                public_key: MlDsa87::public_key(k),
            })
            .collect(),
    )
    .expect("2-of-3");
    let account = multisig_address(&policy);
    let f = fixture(&[account]);

    let frame = |kind: TxKind, nonce: u64, signers: &[u8]| {
        let mut tx = Transaction::with_kind(kind, nonce);
        tx.multisig = Some(Box::new(MultisigAuth::unsigned(policy.clone())));
        let message = tx.signing_bytes(&common::test_chain());
        let approvals = signers
            .iter()
            .map(|&i| Approval {
                index: i,
                signature: MlDsa87::sign(&members[usize::from(i)], &message).expect("sign"),
            })
            .collect();
        tx.multisig = Some(Box::new(
            MultisigAuth::new(policy.clone(), approvals).expect("shape"),
        ));
        tx
    };

    // The policy account registers itself as the authority (a quorum signs).
    let reg = frame(register(0, U256::from(50), account), 0, &[0, 1]);
    apply(&f, vec![reg], V4).expect("register under the policy");
    let asset = derive_asset_id(&account, 0, &SYMBOL);

    // One member alone: the frame does not verify, so nothing is minted.
    let lone = frame(mint(asset, account, U256::from(7)), 1, &[0]);
    assert!(apply(&f, vec![lone], V4 + 1).is_err());

    // A member's own single-key account is a different sender.
    let member_key = generate_signing_key().expect("key");
    let g = fixture(&[member_key.address(), account]);
    let reg = frame(register(0, U256::from(50), account), 0, &[0, 1]);
    apply(&g, vec![reg], V4).expect("register");
    let own = signed(
        mint(asset, member_key.address(), U256::from(7)),
        0,
        &member_key,
    );
    let err = apply(&g, vec![own], V4 + 1).unwrap_err();
    assert!(
        refusal(&err).contains("not the asset's mint authority"),
        "{err}"
    );

    // The quorum itself can.
    let quorum = frame(mint(asset, account, U256::from(7)), 1, &[1, 2]);
    apply(&f, vec![quorum], V4 + 1).expect("a quorum mints");
    assert_eq!(balance(&f, &asset, &account), U256::from(7));
}

// 5 (T-5)
#[test]
fn a_zero_amount_mint_is_refused_and_leaves_the_v1_record_untouched() {
    let key = generate_signing_key().expect("key");
    let f = fixture(&[key.address()]);
    let reg = signed(
        TxKind::RegisterAsset(AssetRegistration {
            symbol: SYMBOL,
            total_supply: U256::from(40),
        }),
        0,
        &key,
    );
    apply(&f, vec![reg], V4).expect("v1 register");
    let asset = derive_asset_id(&key.address(), 0, &SYMBOL);
    let root = f.db.state_root().unwrap();

    let zero = signed(mint(asset, key.address(), U256::zero()), 1, &key);
    let err = apply(&f, vec![zero], V4 + 1).unwrap_err();
    assert!(refusal(&err).contains("zero"), "{err}");
    let zero_burn = signed(burn(asset, U256::zero()), 1, &key);
    assert!(refusal(&apply(&f, vec![zero_burn], V4 + 1).unwrap_err()).contains("zero"));
    // A v1 asset has no authority, so a real mint is refused too.
    let one = signed(mint(asset, key.address(), U256::one()), 1, &key);
    assert!(refusal(&apply(&f, vec![one], V4 + 1).unwrap_err()).contains("no mint authority"));
    assert_eq!(
        f.db.state_root().unwrap(),
        root,
        "the v1 record was not rewritten"
    );
    let lifted = record(&f, &asset);
    assert_eq!(
        (lifted.cap, lifted.authority),
        (U256::from(40), NO_AUTHORITY)
    );
}

// 6 (T-6)
#[test]
fn a_burn_over_the_senders_balance_is_refused_even_if_supply_covers_it() {
    let s = issued();
    let holder = generate_signing_key().expect("key");
    s.f.db
        .put_account(
            &holder.address(),
            &Account {
                balance: U256::from(1_000u64),
                nonce: 0,
            },
        )
        .unwrap();
    let pay = signed(
        TxKind::TransferAsset(AssetTransfer {
            asset: s.asset,
            recipient: holder.address(),
            amount: U256::from(10),
        }),
        1,
        &s.key,
    );
    apply(&s.f, vec![pay], V4 + 1).expect("transfer");

    let over = signed(burn(s.asset, U256::from(11)), 0, &holder);
    assert!(matches!(
        apply(&s.f, vec![over], V4 + 2).unwrap_err(),
        NodeError::InsufficientAssetBalance { .. }
    ));

    let exact = signed(burn(s.asset, U256::from(10)), 0, &holder);
    apply(&s.f, vec![exact], V4 + 2).expect("burn the whole balance");
    let r = record(&s.f, &s.asset);
    assert_eq!(r.base.total_supply, U256::from(90));
    assert_eq!(r.cap, U256::from(1_000), "a burn leaves the cap");
    assert_eq!(balance(&s.f, &s.asset, &s.issuer), U256::from(90));
}

// 7 (T-7)
#[test]
fn a_registration_with_cap_below_supply_is_refused() {
    let key = generate_signing_key().expect("key");
    let f = fixture(&[key.address()]);
    let below = signed(register(101, U256::from(100), key.address()), 0, &key);
    let err = apply(&f, vec![below], V4).unwrap_err();
    assert!(refusal(&err).contains("above the cap"), "{err}");

    // The authority is the sender or none, never a third party.
    let other = signed(register(1, U256::from(100), [9; 32]), 0, &key);
    assert!(refusal(&apply(&f, vec![other], V4).unwrap_err()).contains("authority"));

    // A cap past the DEX's supply ceiling would overflow pool prices.
    let huge = signed(register(1, U256::MAX, key.address()), 0, &key);
    assert!(refusal(&apply(&f, vec![huge], V4).unwrap_err()).contains("MAX_ASSET_SUPPLY"));

    let equal = signed(register(100, U256::from(100), NO_AUTHORITY), 0, &key);
    apply(&f, vec![equal], V4).expect("supply == cap");
}

// 8 (T-9)
#[test]
fn a_mint_one_block_before_v4_activation_is_refused_and_at_it_is_applied() {
    let key = generate_signing_key().expect("key");
    let f = fixture(&[key.address()]);
    let reg = || signed(register(1, U256::from(10), key.address()), 0, &key);
    assert_eq!(
        apply(&f, vec![reg()], V4 - 1).unwrap_err(),
        NodeError::AssetMintInactive {
            height: V4 - 1,
            activation: V4
        }
    );
    // The compiled default is never.
    assert!(matches!(
        f.db.apply_block(
            &block_of(vec![reg()]),
            BlockContext::at_height(u64::MAX - 1)
        )
        .unwrap_err(),
        NodeError::AssetMintInactive { .. }
    ));
    apply(&f, vec![reg()], V4).expect("at activation");

    // Admission refuses it too: no schedule names bit 41 here.
    let mempool = Mempool::new(Arc::clone(&f.db));
    let asset = derive_asset_id(&key.address(), 0, &SYMBOL);
    let pending = signed(mint(asset, key.address(), U256::one()), 1, &key);
    assert!(matches!(
        mempool.validate(&pending).unwrap_err(),
        NodeError::AssetMintInactive { .. }
    ));
}

// 9 (T-10)
#[test]
fn a_dex_breaker_halts_mint_and_burn() {
    let s = issued();
    for kind in [
        mint(s.asset, s.issuer, U256::one()),
        burn(s.asset, U256::one()),
        renounce(s.asset),
        register(1, U256::from(2), s.issuer),
    ] {
        assert_eq!(Module::of(&kind), Some(Module::Dex));
    }
    let tripped = BreakerRecord {
        module: Module::Dex,
        invariant: Invariant::PoolValuePerShare,
        tripped_at: V4,
        until: V4 + 100,
    };
    s.f.db
        .raw_put_for_test(&guard_key(Module::Dex), &tripped.encode())
        .expect("trip");
    let tx = signed(mint(s.asset, s.issuer, U256::one()), 1, &s.key);
    assert!(matches!(
        apply(&s.f, vec![tx], V4 + 1).unwrap_err(),
        NodeError::ModuleHalted { .. }
    ));
}

// 10
#[test]
fn the_root_is_unchanged_on_a_chain_that_never_mints() {
    // The same v1 registration under a binary with bit 41 dark and one with it
    // active: one root, because nothing rewrote the record and the fee is zero.
    let key = generate_signing_key().expect("key");
    let reg = || {
        signed(
            TxKind::RegisterAsset(AssetRegistration {
                symbol: SYMBOL,
                total_supply: U256::from(40),
            }),
            0,
            &key,
        )
    };
    let dark = fixture(&[key.address()]);
    let lit = fixture(&[key.address()]);
    let a = dark
        .db
        .apply_block(&block_of(vec![reg()]), BlockContext::at_height(V4))
        .unwrap();
    let b = apply(&lit, vec![reg()], V4).unwrap();
    assert_eq!(a, b);
}

#[test]
fn a_renounce_is_permanent_and_a_second_one_is_refused() {
    let s = issued();
    apply(&s.f, vec![signed(renounce(s.asset), 1, &s.key)], V4 + 1).expect("renounce");
    let again = signed(renounce(s.asset), 2, &s.key);
    assert!(refusal(&apply(&s.f, vec![again], V4 + 2).unwrap_err()).contains("no mint authority"));
    let late = signed(mint(s.asset, s.issuer, U256::one()), 2, &s.key);
    assert!(refusal(&apply(&s.f, vec![late], V4 + 2).unwrap_err()).contains("no mint authority"));
}
