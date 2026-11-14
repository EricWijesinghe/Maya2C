//! Red team for capped tokens (ADR-135 P-1, bit 41; V4-V1). Each test is
//! one attack attempt against real block execution, named for the attack,
//! and asserts both the refusal and that nothing moved. The threat-model
//! replays live in `asset_mint_tests.rs`; this file is the attacks that
//! cross into other rules: the native coin, fees, nonces, vaults and the
//! conservation guard.
//!
//! Classes tried (V4-V1 brief): value creation, fee bypass, unbounded
//! per-block work, replay, squatting. Per-block work: executing a form reads
//! and writes a fixed number of records (`state/asset_mint.rs`, no loop), but
//! the block builder's filter (`StateDB::select_applicable_within`) copies the
//! whole overlay for every frame that is not a plain transfer, so a block full
//! of v4 forms (or sponsored transfers) costs every builder work quadratic in
//! the state the block touches. Open as M3 (BACKLOG, v4 activation blockers):
//! one benchmark over a block of v4 forms before any v4 activation height is
//! written.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::asset_mint_payload::AssetMintAction;
use custom_l1_node::core::vault_payload::{VaultAction, VaultConfig};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::asset::NO_AUTHORITY;
use custom_l1_node::state::fees::{FEE_COLLECTOR, FEE_KEY, FeeRecord};
use custom_l1_node::state::{
    Account, Address, AssetId, BlockContext, NATIVE_ASSET, StateDB, derive_asset_id,
};
use maya_dex::Amount;
use primitive_types::U256;
use tempfile::TempDir;

mod common;

const V4: u64 = 10;
const SYMBOL: [u8; 8] = *b"RED\0\0\0\0\0";
const FUNDS: u64 = 10_000_000;

struct Net {
    db: StateDB,
    _dir: TempDir,
}

fn net(funded: &[Address]) -> Net {
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    common::bind(&db);
    for a in funded {
        db.put_account(
            a,
            &Account {
                balance: U256::from(FUNDS),
                nonce: 0,
            },
        )
        .unwrap();
    }
    Net { db, _dir: dir }
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [0; 32],
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
    tx.sign(key, &common::test_chain()).unwrap();
    tx
}

fn apply(n: &Net, txs: Vec<Transaction>, height: u64) -> Result<[u8; 32], NodeError> {
    let context = BlockContext::at_height(height).with_asset_mint_activation(V4);
    n.db.apply_block(&block_of(txs), context)
}

fn mint(asset: AssetId, to: Address, amount: u64) -> TxKind {
    TxKind::AssetMint(AssetMintAction::Mint {
        asset,
        to,
        amount: U256::from(amount),
    })
}

fn burn(asset: AssetId, amount: u64) -> TxKind {
    TxKind::AssetMint(AssetMintAction::Burn {
        asset,
        amount: U256::from(amount),
    })
}

/// `key` registers supply 100, cap 1,000, authority itself, at nonce 0.
fn issue(n: &Net, key: &HybridSigningKey) -> AssetId {
    let reg = TxKind::AssetMint(AssetMintAction::RegisterCapped {
        symbol: SYMBOL,
        total_supply: U256::from(100),
        cap: U256::from(1_000),
        authority: key.address(),
    });
    apply(n, vec![signed(reg, 0, key)], V4).expect("register");
    derive_asset_id(&key.address(), 0, &SYMBOL)
}

fn supply(n: &Net, asset: &AssetId) -> U256 {
    n.db.get_capped_asset(asset)
        .unwrap()
        .expect("registered")
        .base
        .total_supply
}

fn held(n: &Net, asset: &AssetId, who: &Address) -> U256 {
    n.db.asset_balance(asset, who).unwrap()
}

// Value creation: the native coin is not a capped asset, so neither the
// mint authority nor anyone else can issue or destroy it through tags 56/57.
#[test]
fn native_coin_cannot_be_minted_or_burned_through_the_capped_forms() {
    let key = generate_signing_key().unwrap();
    let me = key.address();
    let n = net(&[me]);
    let err = apply(&n, vec![signed(mint(NATIVE_ASSET, me, 1), 0, &key)], V4).unwrap_err();
    assert!(err.to_string().contains("native coin"), "{err}");
    let err = apply(&n, vec![signed(burn(NATIVE_ASSET, 1), 0, &key)], V4).unwrap_err();
    assert!(err.to_string().contains("native coin"), "{err}");
    assert_eq!(n.db.get_account(&me).unwrap().balance, U256::from(FUNDS));
}

// Value creation: an attacker names a victim's asset and itself as `to`.
// The mint authority is the record's, never the transaction's claim.
#[test]
fn a_stranger_cannot_mint_a_victims_asset_to_itself() {
    let victim = generate_signing_key().unwrap();
    let thief = generate_signing_key().unwrap();
    let n = net(&[victim.address(), thief.address()]);
    let asset = issue(&n, &victim);
    let tx = signed(mint(asset, thief.address(), 900), 0, &thief);
    let err = apply(&n, vec![tx], V4 + 1).unwrap_err();
    assert!(err.to_string().contains("mint authority"), "{err}");
    assert_eq!(supply(&n, &asset), U256::from(100));
    assert!(held(&n, &asset, &thief.address()).is_zero());
}

// Value creation, conservation: mint to a third party, burns by both
// holders. Supply equals the sum of balances at every step, and the
// conservation guard (which runs inside apply) accepted each block.
#[test]
fn supply_equals_the_sum_of_balances_through_mint_and_burn_by_others() {
    let issuer = generate_signing_key().unwrap();
    let holder = generate_signing_key().unwrap();
    let (i, h) = (issuer.address(), holder.address());
    let n = net(&[i, h]);
    let asset = issue(&n, &issuer);
    apply(&n, vec![signed(mint(asset, h, 400), 1, &issuer)], V4 + 1).expect("mint");
    apply(&n, vec![signed(burn(asset, 150), 0, &holder)], V4 + 2).expect("holder burns");
    apply(&n, vec![signed(burn(asset, 30), 2, &issuer)], V4 + 3).expect("issuer burns");
    let total = held(&n, &asset, &i) + held(&n, &asset, &h);
    assert_eq!(supply(&n, &asset), U256::from(100 + 400 - 150 - 30));
    assert_eq!(total, supply(&n, &asset));
}

// Design pin, not a bug: the cap bounds *outstanding* supply. Units a holder
// burns can be minted again by the authority, up to the cap and no further.
// A wallet that shows "max supply" must say outstanding, not lifetime.
#[test]
fn the_cap_bounds_outstanding_supply_so_burned_units_can_be_reminted() {
    let issuer = generate_signing_key().unwrap();
    let me = issuer.address();
    let n = net(&[me]);
    let asset = issue(&n, &issuer);
    apply(&n, vec![signed(mint(asset, me, 900), 1, &issuer)], V4 + 1).expect("to cap");
    apply(&n, vec![signed(burn(asset, 500), 2, &issuer)], V4 + 2).expect("burn");
    apply(&n, vec![signed(mint(asset, me, 500), 3, &issuer)], V4 + 3).expect("re-mint");
    assert_eq!(supply(&n, &asset), U256::from(1_000));
    let over = signed(mint(asset, me, 1), 4, &issuer);
    assert!(apply(&n, vec![over], V4 + 4).is_err(), "never past the cap");
}

// Replay: the same signed mint, offered again in a later block, is refused
// on its nonce, and the supply rises once.
#[test]
fn a_replayed_mint_is_refused_on_its_nonce() {
    let issuer = generate_signing_key().unwrap();
    let me = issuer.address();
    let n = net(&[me]);
    let asset = issue(&n, &issuer);
    let tx = signed(mint(asset, me, 10), 1, &issuer);
    apply(&n, vec![tx.clone()], V4 + 1).expect("first");
    let err = apply(&n, vec![tx.clone()], V4 + 2).unwrap_err();
    assert!(matches!(err, NodeError::InvalidNonce { .. }), "{err}");
    let err = apply(&n, vec![tx.clone(), tx], V4 + 3).unwrap_err();
    assert!(matches!(err, NodeError::InvalidNonce { .. }), "{err}");
    assert_eq!(supply(&n, &asset), U256::from(110));
}

// Fee bypass: with fees live, a mint that pays no byte fee is refused, and
// one paying exactly the required fee is applied.
#[test]
fn a_capped_form_without_the_byte_fee_is_refused_when_fees_are_live() {
    let issuer = generate_signing_key().unwrap();
    let me = issuer.address();
    let n = net(&[me]);
    let asset = issue(&n, &issuer);
    let record = FeeRecord {
        base_fee: Amount::from_u64(1),
        min_base_fee: Amount::from_u64(1),
        target_block_bytes: 1_000_000,
        change_denominator: 8,
    };
    n.db.raw_put_for_test(FEE_KEY, &record.encode()).unwrap();
    let free = signed(mint(asset, me, 10), 1, &issuer);
    let err = apply(&n, vec![free], V4 + 1).unwrap_err();
    assert!(matches!(err, NodeError::FeeTooLow { .. }), "{err}");
    assert_eq!(supply(&n, &asset), U256::from(100));

    let paying = |fee: u64| {
        let mut tx = Transaction::with_kind(mint(asset, me, 10), 1);
        tx.outputs.push(TxOutput {
            amount: U256::from(fee),
            recipient: FEE_COLLECTOR,
        });
        tx.sign(&issuer, &common::test_chain()).unwrap();
        tx
    };
    // Base fee 1 per byte, fixed-width encoding: the fee is the size.
    let required = paying(1).to_bytes().len() as u64;
    let short = apply(&n, vec![paying(required - 1)], V4 + 1).unwrap_err();
    assert!(matches!(short, NodeError::FeeTooLow { .. }), "{short}");
    apply(&n, vec![paying(required)], V4 + 1).expect("paid exactly");
    assert_eq!(supply(&n, &asset), U256::from(110));
}

// Squatting: an attacker who copies the victim's symbol and nonce cannot
// pre-register the id the victim will derive; the creator is in the id.
#[test]
fn an_attacker_cannot_preregister_the_id_a_victim_will_derive() {
    let victim = generate_signing_key().unwrap();
    let squatter = generate_signing_key().unwrap();
    let n = net(&[victim.address(), squatter.address()]);
    let squatted = issue(&n, &squatter);
    let asset = issue(&n, &victim);
    assert_ne!(squatted, asset);
    let r = n.db.get_capped_asset(&asset).unwrap().unwrap();
    assert_eq!(r.base.creator, victim.address());
    assert_eq!(r.authority, victim.address());
}

// A stolen vault key cannot use the capped forms to skip the vault delay:
// no burn of vault holdings, no renounce of the vault's mint authority.
#[test]
fn a_vault_cannot_burn_or_renounce_through_the_capped_forms() {
    let owner = generate_signing_key().unwrap();
    let guardian = generate_signing_key().unwrap();
    let me = owner.address();
    let n = net(&[me]);
    let asset = issue(&n, &owner);
    let configure = TxKind::Vault(Box::new(VaultAction::Configure(VaultConfig {
        delay_blocks: 10,
        limit: U256::from(100),
        guardians: vec![guardian.address()],
    })));
    apply(&n, vec![signed(configure, 1, &owner)], V4 + 1).expect("vault");
    let err = apply(&n, vec![signed(burn(asset, 100), 2, &owner)], V4 + 2).unwrap_err();
    assert!(err.to_string().contains("vault"), "{err}");
    let renounce = TxKind::AssetMint(AssetMintAction::Renounce { asset });
    let err = apply(&n, vec![signed(renounce, 2, &owner)], V4 + 2).unwrap_err();
    assert!(err.to_string().contains("vault"), "{err}");
    let r = n.db.get_capped_asset(&asset).unwrap().unwrap();
    assert_eq!(r.authority, me);
    assert_ne!(r.authority, NO_AUTHORITY);
    assert_eq!(held(&n, &asset, &me), U256::from(100));
}
