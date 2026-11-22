//! Sponsored fees (ADR-135 P-4, feature bit 44, wire v13) against real block
//! execution and the real mempool: the replay tests
//! `docs/threat-models/v4-paymaster.md` lists, one per attack, plus the drain
//! tests the I4 brief makes blocking — an attacker can never make a sponsor
//! pay beyond what it signed for, with many frames in one block and across a
//! reorg.
//!
//! Every refusal asserts the specific error, so a stub that refuses
//! everything fails these tests rather than passing them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::sponsor_tx::{MAX_SPONSOR_WINDOW, MAX_SPONSORED_PENDING_PER_SPONSOR};
use custom_l1_node::core::transaction::{ChainTag, TxOutput};
use custom_l1_node::core::vault_payload::{VaultAction, VaultConfig};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::crypto::validator_keys as keys;
use custom_l1_node::error::NodeError;
use custom_l1_node::genesis::{Allocation, BftGenesis, FeesGenesis, GenesisConfig};
use custom_l1_node::network::mempool::Mempool;
use custom_l1_node::state::fees::{FEE_COLLECTOR, FEE_KEY, FeeRecord};
use custom_l1_node::state::shielded::FEE_SINK;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use custom_l1_node::upgrade::{FeatureSet, ProtocolUpgrade};
use maya_crypto_pq::suite::{
    HybridMlDsa65SlhDsa128s, MasterSeed, MlDsa87, SignatureSuite, SuiteId, suite_address,
};
use maya_dex::Amount;
use primitive_types::U256;
use tempfile::TempDir;

mod common;

/// The v4 activation height these tests run under.
const V4: u64 = 10;
/// Base fee per byte. The floor is the same, so it never falls between blocks.
const BASE_FEE: u64 = 2;
/// What every funded account starts with.
const FUNDS: u64 = 100_000_000;
/// A ceiling well above any frame here: about 2 × 25 KB.
const GENEROUS: u64 = 1_000_000;

type SponsorKey = <HybridMlDsa65SlhDsa128s as SignatureSuite>::SigningKey;

struct Fixture {
    db: Arc<StateDB>,
    _dir: TempDir,
}

fn fee_record() -> FeeRecord {
    FeeRecord {
        base_fee: Amount::from_u64(BASE_FEE),
        min_base_fee: Amount::from_u64(BASE_FEE),
        target_block_bytes: 1_000_000_000,
        change_denominator: 8,
    }
}

fn fixture(funded: &[Address]) -> Fixture {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    common::bind(&db);
    db.raw_put_for_test(FEE_KEY, &fee_record().encode())
        .expect("fees on");
    for address in funded {
        fund(&db, address, U256::from(FUNDS));
    }
    Fixture {
        db: Arc::new(db),
        _dir: dir,
    }
}

fn fund(db: &StateDB, address: &Address, balance: U256) {
    db.put_account(address, &Account { balance, nonce: 0 })
        .expect("fund");
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

/// A context at `height` with bit 44 active from [`V4`].
fn at(height: u64) -> BlockContext {
    BlockContext::at_height(height).with_sponsored_fees_activation(V4)
}

fn apply(f: &Fixture, txs: Vec<Transaction>, height: u64) -> Result<[u8; 32], NodeError> {
    f.db.apply_block(&block_of(txs), at(height))
}

fn balance(f: &Fixture, who: &Address) -> U256 {
    f.db.get_account(who).expect("read").balance
}

fn nonce(f: &Fixture, who: &Address) -> u64 {
    f.db.get_account(who).expect("read").nonce
}

fn user(seed: u8) -> HybridSigningKey {
    signing_key_from_seed(&[seed; 32]).expect("key")
}

fn sponsor_key(seed: u8) -> SponsorKey {
    HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes([seed; 32]))
}

fn sponsor_address(key: &SponsorKey) -> Address {
    suite_address(
        SuiteId::HybridMlDsa65SlhDsa128s,
        &HybridMlDsa65SlhDsa128s::public_key(key),
    )
}

const PAYEE: Address = [0x77; 32];

/// A signed hybrid transfer of `amount` to [`PAYEE`] with no fee output.
fn inner(key: &HybridSigningKey, nonce: u64, amount: u64) -> Transaction {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: U256::from(amount),
            recipient: PAYEE,
        }],
        nonce,
    );
    tx.sign(key, &common::test_chain()).expect("sign");
    tx
}

fn sponsor_on(
    mut tx: Transaction,
    key: &SponsorKey,
    max_fee: U256,
    valid_until: u64,
    chain: &ChainTag,
) -> Transaction {
    tx.sponsor_with::<HybridMlDsa65SlhDsa128s>(key, max_fee, valid_until, chain)
        .expect("sponsor");
    tx
}

fn sponsored(tx: Transaction, key: &SponsorKey, max_fee: u64, valid_until: u64) -> Transaction {
    sponsor_on(
        tx,
        key,
        U256::from(max_fee),
        valid_until,
        &common::test_chain(),
    )
}

/// What the fee rule charges for `tx`'s encoded bytes.
fn required(tx: &Transaction) -> U256 {
    U256::from(BASE_FEE) * U256::from(tx.to_bytes().len())
}

fn refusal(error: &NodeError) -> &'static str {
    match error {
        NodeError::SponsorshipRefused { reason } => reason,
        other => panic!("expected SponsorshipRefused, got {other:?}"),
    }
}

fn total(f: &Fixture, accounts: &[Address]) -> U256 {
    accounts
        .iter()
        .chain([PAYEE, FEE_COLLECTOR, FEE_SINK].iter())
        .fold(U256::zero(), |sum, a| sum + balance(f, a))
}

// ---- Drain (blocking) ---------------------------------------------------

#[test]
fn many_sponsored_frames_in_one_block_debit_exactly_their_fees_and_never_past_max_fee() {
    let alice = user(0x31);
    let key = sponsor_key(0x5A);
    let payer = sponsor_address(&key);
    let f = fixture(&[alice.address(), payer]);
    let before = total(&f, &[alice.address(), payer]);

    let frames: Vec<Transaction> = (0..6)
        .map(|n| sponsored(inner(&alice, n, 10), &key, GENEROUS, V4 + 100))
        .collect();
    let owed = frames.iter().fold(U256::zero(), |s, t| s + required(t));
    let ceiling = U256::from(GENEROUS) * U256::from(frames.len());
    apply(&f, frames, V4).expect("six sponsored frames");

    assert_eq!(
        balance(&f, &payer),
        U256::from(FUNDS) - owed,
        "the sponsor pays each frame's own fee, once"
    );
    assert!(owed < ceiling, "and that is below what it signed for");
    assert_eq!(balance(&f, &FEE_SINK), owed, "the whole base fee burns");
    assert_eq!(
        balance(&f, &alice.address()),
        U256::from(FUNDS - 60),
        "the sender pays only its outputs"
    );
    assert_eq!(nonce(&f, &alice.address()), 6);
    assert_eq!(
        total(&f, &[alice.address(), payer]),
        before,
        "value conserved"
    );
}

#[test]
fn a_block_whose_sponsored_fees_pass_the_sponsor_balance_is_refused_whole() {
    let alice = user(0x32);
    let key = sponsor_key(0x5B);
    let payer = sponsor_address(&key);
    let f = fixture(&[alice.address()]);

    let frames: Vec<Transaction> = (0..3)
        .map(|n| sponsored(inner(&alice, n, 1), &key, GENEROUS, V4 + 100))
        .collect();
    // Enough for two frames, one unit short of the third.
    let two = required(&frames[0]) + required(&frames[1]);
    let short = two + required(&frames[2]) - U256::one();
    fund(&f.db, &payer, short);
    let root = f.db.state_root().expect("root");

    let err = apply(&f, frames.clone(), V4).unwrap_err();
    assert!(
        matches!(err, NodeError::InsufficientBalance { .. }),
        "{err:?}"
    );
    assert_eq!(f.db.state_root().expect("root"), root, "nothing applied");
    assert_eq!(balance(&f, &payer), short);
    assert_eq!(nonce(&f, &alice.address()), 0, "no nonce consumed");

    // The builder keeps the two the sponsor can pay and drops the third,
    // leaving its debit nowhere in the overlay.
    let kept =
        f.db.select_applicable(frames, at(V4), target_from_leading_zero_bits(0))
            .expect("select");
    assert_eq!(kept.len(), 2);
    apply(&f, kept, V4).expect("the two that fit");
    assert_eq!(balance(&f, &payer), short - two);
    assert_eq!(nonce(&f, &alice.address()), 2);
}

#[test]
fn a_reorg_restores_the_sponsor_and_the_frames_pay_once_on_the_surviving_branch() {
    let alice = user(0x33);
    let key = sponsor_key(0x5C);
    let payer = sponsor_address(&key);
    let f = fixture(&[alice.address(), payer]);
    let root_before = f.db.state_root().expect("root");

    let frames: Vec<Transaction> = (0..3)
        .map(|n| sponsored(inner(&alice, n, 1), &key, GENEROUS, V4 + 100))
        .collect();
    let owed = frames.iter().fold(U256::zero(), |s, t| s + required(t));
    let mut block = block_of(frames.clone());
    block.header.state_root = f.db.preview_root(&block, at(V4)).expect("preview");
    let id = [7u8; 32];
    f.db.apply_block_journaled(&block, &id, at(V4))
        .expect("branch A");
    assert_eq!(balance(&f, &payer), U256::from(FUNDS) - owed);

    f.db.revert_block(&id).expect("revert");
    assert_eq!(
        balance(&f, &payer),
        U256::from(FUNDS),
        "the debit is undone"
    );
    assert_eq!(nonce(&f, &alice.address()), 0);
    assert_eq!(f.db.state_root().expect("root"), root_before);

    // The same frames on the surviving branch pay once, and a replay of the
    // block after them pays nothing more.
    apply(&f, frames.clone(), V4).expect("branch B");
    assert_eq!(balance(&f, &payer), U256::from(FUNDS) - owed);
    let err = apply(&f, frames, V4 + 1).unwrap_err();
    assert!(matches!(err, NodeError::InvalidNonce { .. }), "{err:?}");
    assert_eq!(balance(&f, &payer), U256::from(FUNDS) - owed);
}

// ---- Threat model replay tests --------------------------------------------

// 1 (P-1)
#[test]
fn a_sponsorship_applied_to_a_second_inner_with_the_same_txid_fails_on_the_nonce() {
    let alice = user(0x34);
    let key = sponsor_key(0x5D);
    let payer = sponsor_address(&key);
    let f = fixture(&[alice.address(), payer]);
    let frame = sponsored(inner(&alice, 0, 1), &key, GENEROUS, V4 + 100);

    // Twice in one block: the second fails on the nonce, so the block does.
    let err = apply(&f, vec![frame.clone(), frame.clone()], V4).unwrap_err();
    assert!(matches!(err, NodeError::InvalidNonce { .. }), "{err:?}");

    apply(&f, vec![frame.clone()], V4).expect("once");
    let paid = U256::from(FUNDS) - balance(&f, &payer);
    assert_eq!(paid, required(&frame));

    // Re-signing the same inner is deterministic: the same txid, the same
    // nonce, refused.
    let again = sponsored(inner(&alice, 0, 1), &key, GENEROUS, V4 + 100);
    assert_eq!(again.inner_txid(), frame.inner_txid());
    let err = apply(&f, vec![again], V4 + 1).unwrap_err();
    assert!(matches!(err, NodeError::InvalidNonce { .. }), "{err:?}");

    // The sponsorship lifted onto a different inner does not verify.
    let mut lifted = inner(&alice, 1, 1);
    lifted.sponsor = frame.sponsor.clone();
    let err = apply(&f, vec![lifted], V4 + 1).unwrap_err();
    assert!(matches!(err, NodeError::SignatureSuite(_)), "{err:?}");
    assert_eq!(U256::from(FUNDS) - balance(&f, &payer), paid, "paid once");
}

// 2 (P-2)
#[test]
fn a_sponsorship_cannot_pay_more_than_max_fee_for_a_padded_outer_frame() {
    let alice = user(0x35);
    let key = sponsor_key(0x5E);
    let payer = sponsor_address(&key);
    let f = fixture(&[alice.address(), payer]);

    // The outer frame's size does not depend on the value of max_fee.
    let exact = required(&sponsored(inner(&alice, 0, 1), &key, 0, V4 + 100));
    let at_max = sponsor_on(
        inner(&alice, 0, 1),
        &key,
        exact,
        V4 + 100,
        &common::test_chain(),
    );
    apply(&f, vec![at_max], V4).expect("required == max_fee applies");
    assert_eq!(balance(&f, &payer), U256::from(FUNDS) - exact);

    // A padded inner (four more outputs) costs more than the same ceiling.
    let mut padded = Transaction::new(
        vec![],
        (0..5)
            .map(|_| TxOutput {
                amount: U256::one(),
                recipient: PAYEE,
            })
            .collect(),
        1,
    );
    padded.sign(&alice, &common::test_chain()).expect("sign");
    let padded = sponsor_on(padded, &key, exact, V4 + 100, &common::test_chain());
    assert!(required(&padded) > exact);
    let err = apply(&f, vec![padded], V4 + 1).unwrap_err();
    assert!(refusal(&err).contains("exceeds max_fee"), "{err}");

    // One under the requirement is refused; nothing moved.
    let tight = sponsored(inner(&alice, 1, 1), &key, 0, V4 + 100);
    let tight = sponsor_on(
        inner(&alice, 1, 1),
        &key,
        required(&tight) - U256::one(),
        V4 + 100,
        &common::test_chain(),
    );
    let err = apply(&f, vec![tight], V4 + 1).unwrap_err();
    assert!(refusal(&err).contains("exceeds max_fee"), "{err}");
    assert_eq!(balance(&f, &payer), U256::from(FUNDS) - exact);
    assert_eq!(nonce(&f, &alice.address()), 1);
}

// 3 (P-3)
#[test]
fn a_sponsorship_signed_under_another_chain_tag_is_refused() {
    let alice = user(0x36);
    let key = sponsor_key(0x5F);
    let payer = sponsor_address(&key);
    let f = fixture(&[alice.address(), payer]);
    let foreign = ChainTag::from_genesis([0xEE; 32]);
    let frame = sponsor_on(
        inner(&alice, 0, 1),
        &key,
        U256::from(GENEROUS),
        V4 + 100,
        &foreign,
    );
    let err = apply(&f, vec![frame], V4).unwrap_err();
    assert!(
        matches!(&err, NodeError::SignatureSuite(m) if m.contains("sponsor")),
        "{err:?}"
    );
    assert_eq!(balance(&f, &payer), U256::from(FUNDS));
    assert_eq!(nonce(&f, &alice.address()), 0);
}

// 4 (P-4)
#[test]
fn a_stripped_outer_frame_leaves_an_unfunded_inner_that_is_refused_without_a_nonce_burn() {
    let alice = user(0x37);
    let key = sponsor_key(0x60);
    let payer = sponsor_address(&key);
    let f = fixture(&[alice.address(), payer]);
    let frame = sponsored(inner(&alice, 0, 1), &key, GENEROUS, V4 + 100);

    let mut stripped = frame.clone();
    stripped.sponsor = None;
    assert_eq!(stripped.txid(), frame.inner_txid());
    let err = apply(&f, vec![stripped], V4).unwrap_err();
    assert!(matches!(err, NodeError::FeeTooLow { .. }), "{err:?}");
    assert_eq!(nonce(&f, &alice.address()), 0, "the nonce survives");

    apply(&f, vec![frame], V4).expect("the sponsored frame still applies");
    assert_eq!(nonce(&f, &alice.address()), 1);
}

// 5 (P-5)
#[test]
fn wrapping_a_fee_paying_inner_in_a_sponsorship_is_refused() {
    let alice = user(0x38);
    let key = sponsor_key(0x61);
    let payer = sponsor_address(&key);
    let f = fixture(&[alice.address(), payer]);
    let mut paying = Transaction::new(
        vec![],
        vec![
            TxOutput {
                amount: U256::one(),
                recipient: PAYEE,
            },
            TxOutput {
                amount: U256::from(GENEROUS),
                recipient: FEE_COLLECTOR,
            },
        ],
        0,
    );
    paying.sign(&alice, &common::test_chain()).expect("sign");
    let wrapped = sponsored(paying, &key, GENEROUS, V4 + 100);
    let err = apply(&f, vec![wrapped], V4).unwrap_err();
    assert!(refusal(&err).contains("pays the fee collector"), "{err}");
    assert_eq!(balance(&f, &payer), U256::from(FUNDS));
}

// 7 (P-7)
#[test]
fn a_sponsor_that_spends_to_zero_voids_its_frames_at_apply_without_consuming_the_senders_nonce() {
    let alice = user(0x39);
    let (rug, honest) = (sponsor_key(0x62), sponsor_key(0x63));
    let f = fixture(&[alice.address(), sponsor_address(&honest)]);
    // The rug sponsor signed while funded, then emptied its account.
    fund(&f.db, &sponsor_address(&rug), U256::zero());
    let frame = sponsored(inner(&alice, 0, 1), &rug, GENEROUS, V4 + 100);
    let err = apply(&f, vec![frame], V4).unwrap_err();
    assert!(
        matches!(err, NodeError::InsufficientBalance { .. }),
        "{err:?}"
    );
    assert_eq!(nonce(&f, &alice.address()), 0);

    // The same inner, re-sponsored by someone who can pay.
    let rescued = sponsored(inner(&alice, 0, 1), &honest, GENEROUS, V4 + 100);
    apply(&f, vec![rescued], V4).expect("re-sponsored");
    assert_eq!(nonce(&f, &alice.address()), 1);
}

// 8 (P-8)
#[test]
fn a_sponsorship_past_valid_until_or_beyond_the_window_is_refused() {
    let alice = user(0x3A);
    let key = sponsor_key(0x64);
    let f = fixture(&[alice.address(), sponsor_address(&key)]);

    let expires_now = sponsored(inner(&alice, 0, 1), &key, GENEROUS, V4);
    let err = apply(&f, vec![expires_now.clone()], V4 + 1).unwrap_err();
    assert!(refusal(&err).contains("expired"), "{err}");
    apply(&f, vec![expires_now], V4).expect("height == valid_until applies");

    // The window is admission policy (Eric O-4): a block may carry a longer one.
    let long = sponsored(
        inner(&alice, 1, 1),
        &key,
        GENEROUS,
        V4 + 1 + MAX_SPONSOR_WINDOW + 5,
    );
    apply(&f, vec![long], V4 + 1).expect("valid in a block");

    // At admission it is refused one block past the window, accepted at it.
    let dir = TempDir::new().expect("dir");
    let (c, height) = live_chain(&dir, &[alice.address(), sponsor_address(&key)]);
    let pool = Mempool::new(Arc::clone(c.state()));
    let too_long = sponsored(
        inner(&alice, 0, 1),
        &key,
        GENEROUS,
        height + MAX_SPONSOR_WINDOW + 1,
    );
    let err = pool.insert(too_long).unwrap_err();
    assert!(refusal(&err).contains("MAX_SPONSOR_WINDOW"), "{err}");
    let in_window = sponsored(
        inner(&alice, 0, 1),
        &key,
        GENEROUS,
        height + MAX_SPONSOR_WINDOW,
    );
    assert!(pool.insert(in_window).expect("admitted"));
}

// 9 (P-9)
#[test]
fn a_v13_frame_inside_a_v13_frame_is_refused_at_decode() {
    let key = sponsor_key(0x65);
    let frame = sponsored(inner(&user(0x3B), 0, 1), &key, GENEROUS, V4 + 100);
    let bytes = frame.to_bytes();
    assert_eq!(bytes[0], 0x0D);
    assert_eq!(Transaction::from_bytes(&bytes).expect("round trip"), frame);

    let mut nested = vec![0x0D];
    nested.extend_from_slice(&u32::try_from(bytes.len()).unwrap().to_le_bytes());
    nested.extend_from_slice(&bytes);
    nested.extend_from_slice(&bytes[bytes.len() - 64..]);
    let err = Transaction::from_bytes(&nested).unwrap_err();
    assert!(
        matches!(&err, NodeError::Decode(m) if m.contains("depth 1")),
        "{err:?}"
    );
}

/// A v7 frame from the hybrid-suite `key`, paying its own fee: what a
/// sponsor sends to configure its own account.
fn own_paid(kind: TxKind, nonce: u64, key: &SponsorKey) -> Transaction {
    let build = |fee: U256| {
        let mut tx = Transaction::with_kind(kind.clone(), nonce);
        tx.outputs = vec![TxOutput {
            amount: fee,
            recipient: FEE_COLLECTOR,
        }];
        tx.sign_with_suite::<HybridMlDsa65SlhDsa128s>(key, &common::test_chain())
            .expect("sign");
        tx
    };
    // A fee's value does not change the frame's size.
    build(required(&build(U256::zero())))
}

fn vault(limit: U256) -> TxKind {
    TxKind::Vault(Box::new(VaultAction::Configure(VaultConfig {
        delay_blocks: 100,
        limit,
        guardians: vec![[0x99; 32]],
    })))
}

// 10 (P-10)
#[test]
fn a_sponsor_vault_counts_the_fee_toward_its_window() {
    let alice = user(0x3C);
    let key = sponsor_key(0x66);
    let payer = sponsor_address(&key);
    let f = fixture(&[alice.address(), payer]);

    let first = sponsored(inner(&alice, 0, 1), &key, GENEROUS, V4 + 100);
    let one_fee = required(&first);
    // Room for one sponsored fee in the window, not two.
    let configure = own_paid(vault(one_fee + one_fee / 2), 0, &key);
    apply(&f, vec![configure], V4).expect("the sponsor opts into a vault");

    apply(&f, vec![first], V4 + 1).expect("one fee fits the window");
    let second = sponsored(inner(&alice, 1, 1), &key, GENEROUS, V4 + 100);
    let err = apply(&f, vec![second], V4 + 2).unwrap_err();
    assert!(matches!(err, NodeError::Vault(_)), "{err:?}");
    assert_eq!(nonce(&f, &alice.address()), 1);
}

// 11 (P-11)
#[test]
fn base_fee_burn_and_tip_split_are_unchanged_for_a_sponsored_frame() {
    let alice = user(0x3D);
    let key = sponsor_key(0x67);
    let payer = sponsor_address(&key);
    let f = fixture(&[alice.address(), payer]);

    let frame = sponsored(inner(&alice, 0, 1), &key, GENEROUS, V4 + 100);
    let owed = required(&frame);
    apply(&f, vec![frame], V4).expect("sponsored");
    assert_eq!(balance(&f, &FEE_SINK), owed, "the base fee burns");
    assert_eq!(balance(&f, &FEE_COLLECTOR), U256::zero(), "no tip");

    // The same split as an unsponsored frame paying the base fee and a tip.
    let tip = U256::from(500u64);
    let mut plain = Transaction::new(
        vec![],
        vec![
            TxOutput {
                amount: U256::one(),
                recipient: PAYEE,
            },
            TxOutput {
                amount: U256::zero(),
                recipient: FEE_COLLECTOR,
            },
        ],
        1,
    );
    // Measured signed: an amount's value does not change the frame's size.
    plain.sign(&alice, &common::test_chain()).expect("sign");
    let base = required(&plain);
    plain.outputs[1].amount = base + tip;
    plain.sign(&alice, &common::test_chain()).expect("sign");
    apply(&f, vec![plain], V4 + 1).expect("unsponsored");
    assert_eq!(balance(&f, &FEE_SINK), owed + base);
    assert_eq!(balance(&f, &FEE_COLLECTOR), tip);
}

// 12 (P-12)
#[test]
fn two_sponsors_of_one_inner_pay_once() {
    let alice = user(0x3E);
    let (a, b) = (sponsor_key(0x68), sponsor_key(0x69));
    let f = fixture(&[alice.address(), sponsor_address(&a), sponsor_address(&b)]);
    let by_a = sponsored(inner(&alice, 0, 1), &a, GENEROUS, V4 + 100);
    let by_b = sponsored(inner(&alice, 0, 1), &b, GENEROUS, V4 + 100);
    assert_ne!(by_a.txid(), by_b.txid(), "two outer ids");
    assert_eq!(by_a.inner_txid(), by_b.inner_txid(), "one inner");

    let err = apply(&f, vec![by_a.clone(), by_b.clone()], V4).unwrap_err();
    assert!(matches!(err, NodeError::InvalidNonce { .. }), "{err:?}");
    apply(&f, vec![by_a.clone()], V4).expect("A pays");
    let err = apply(&f, vec![by_b], V4 + 1).unwrap_err();
    assert!(matches!(err, NodeError::InvalidNonce { .. }), "{err:?}");
    assert_eq!(
        balance(&f, &sponsor_address(&a)),
        U256::from(FUNDS) - required(&by_a)
    );
    assert_eq!(balance(&f, &sponsor_address(&b)), U256::from(FUNDS));
}

// 12, mempool half (P-12)
#[test]
fn the_mempool_keeps_one_sponsored_frame_per_inner() {
    let alice = user(0x3F);
    let (a, b) = (sponsor_key(0x6A), sponsor_key(0x6B));
    let dir = TempDir::new().expect("dir");
    let (c, height) = live_chain(
        &dir,
        &[alice.address(), sponsor_address(&a), sponsor_address(&b)],
    );
    let pool = Mempool::new(Arc::clone(c.state()));
    let by_a = sponsored(inner(&alice, 0, 1), &a, GENEROUS, height + 100);
    let by_b = sponsored(inner(&alice, 0, 1), &b, GENEROUS, height + 100);
    assert!(pool.insert(by_a.clone()).expect("A admitted"));
    assert!(
        !pool.insert(by_b).expect("B is not an error"),
        "an equal-size second sponsorship of the pooled inner is not kept"
    );
    assert_eq!(pool.len(), 1);
    assert!(pool.contains(&by_a.txid()));
}

/// A two-key vault for `owner`'s own sends, with an instant limit of
/// `limit`, configured through a real fee-paying hybrid transaction.
fn hybrid_vault(f: &Fixture, owner: &HybridSigningKey, limit: u64, height: u64) {
    let build = |fee: U256| {
        let mut tx = Transaction::with_kind(vault(U256::from(limit)), 0);
        tx.outputs = vec![TxOutput {
            amount: fee,
            recipient: FEE_COLLECTOR,
        }];
        tx.sign(owner, &common::test_chain()).expect("sign");
        tx
    };
    let configure = build(required(&build(U256::zero())));
    apply(f, vec![configure], height).expect("vault configured");
}

// 13 (P-13). The threat model names a v9 session key; wire v9 (#447) is not
// on this branch, so the inner's own policy is a vault window instead: the
// sponsorship changes who pays, not what the inner may do.
#[test]
fn a_sponsored_frame_still_obeys_the_inner_senders_own_rules() {
    let alice = user(0x40);
    let key = sponsor_key(0x6C);
    let f = fixture(&[alice.address(), sponsor_address(&key)]);
    hybrid_vault(&f, &alice, 10, V4);

    let over = sponsored(inner(&alice, 1, 11), &key, GENEROUS, V4 + 100);
    let err = apply(&f, vec![over], V4 + 1).unwrap_err();
    assert!(matches!(err, NodeError::Vault(_)), "{err:?}");
    assert_eq!(
        balance(&f, &sponsor_address(&key)),
        U256::from(FUNDS),
        "the refused frame cost the sponsor nothing"
    );
    let within = sponsored(inner(&alice, 1, 10), &key, GENEROUS, V4 + 100);
    apply(&f, vec![within], V4 + 1).expect("within the window");
}

// 14
#[test]
fn a_v13_frame_is_refused_by_verify_and_before_activation_by_verify_at() {
    let alice = user(0x41);
    let key = sponsor_key(0x6D);
    let f = fixture(&[alice.address(), sponsor_address(&key)]);
    let frame = sponsored(inner(&alice, 0, 1), &key, GENEROUS, u64::MAX);

    let err = frame.verify(&common::test_chain()).unwrap_err();
    assert!(
        matches!(&err, NodeError::SignatureSuite(m) if m.contains("v13")),
        "{err:?}"
    );
    // verify_at checks both signatures; the activation height is the shared
    // `require_active` both admission and apply call, since verify_at has
    // no schedule to read.
    assert_eq!(
        apply(&f, vec![frame.clone()], V4 - 1).unwrap_err(),
        NodeError::SponsoredFeesInactive {
            height: V4 - 1,
            activation: V4,
        }
    );
    assert!(matches!(
        f.db.apply_block(
            &block_of(vec![frame.clone()]),
            BlockContext::at_height(u64::MAX - 1)
        )
        .unwrap_err(),
        NodeError::SponsoredFeesInactive { .. }
    ));
    // Admission refuses it too: no schedule names bit 44 here.
    let pool = Mempool::new(Arc::clone(&f.db));
    assert!(matches!(
        pool.insert(frame.clone()).unwrap_err(),
        NodeError::SponsoredFeesInactive { .. }
    ));
    apply(&f, vec![frame], V4).expect("at activation");
}

#[test]
fn a_node_with_accept_sponsored_off_refuses_v13_frames() {
    let alice = user(0x42);
    let key = sponsor_key(0x6E);
    let dir = TempDir::new().expect("dir");
    let (c, height) = live_chain(&dir, &[alice.address(), sponsor_address(&key)]);
    let pool = Mempool::new(Arc::clone(c.state())).with_accept_sponsored(false);
    let frame = sponsored(inner(&alice, 0, 1), &key, GENEROUS, height + 100);
    assert!(matches!(
        pool.insert(frame).unwrap_err(),
        NodeError::MempoolRejected(_)
    ));
}

// ---- Mempool policy on a live chain -------------------------------------

/// A chain whose genesis runs the fee market and schedules bit 44 from
/// height 1, with `funded` holding [`FUNDS`]. Returns the chain and the
/// height its mempool admits at.
fn live_chain(dir: &TempDir, funded: &[Address]) -> (Chain, u64) {
    let validator = keys::signing_key_from_seed(&[1; 32]).expect("validator");
    let config = GenesisConfig {
        chain_id: "sponsored-fees".to_string(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: funded
            .iter()
            .map(|a| Allocation {
                address: hex::encode(a),
                balance: U256::from(FUNDS),
            })
            .collect(),
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: vec![ProtocolUpgrade {
            version: 2,
            activation_height: 1,
            features: FeatureSet(FeatureSet::SPONSORED_FEES),
        }],
        suite_deprecations: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: Some(BftGenesis {
            validators: vec![hex::encode(validator.verifying_key().to_bytes())],
            anchor_timeout_ms: 1_000,
            batch_size: 100,
            round_interval_ms: 500,
            mining: None,
            staking: None,
            fees: Some(FeesGenesis {
                initial_base_fee: U256::from(BASE_FEE),
                min_base_fee: U256::from(BASE_FEE),
                target_block_bytes: 1_000_000,
                change_denominator: 8,
            }),
        }),
    };
    let state = Arc::new(StateDB::open(dir.path()).expect("open"));
    config.seed_state(&state).expect("seed");
    let chain = common::open_chain(
        state,
        config.genesis_block().expect("genesis"),
        ChainConfig::without_pow_verification(),
    )
    .expect("chain");
    (chain, 1)
}

// 6 (P-6)
#[test]
fn the_sixty_fifth_pending_frame_per_sponsor_is_dropped_at_admission_but_valid_in_a_block() {
    let sponsor = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([0x6F; 32]));
    let payer = suite_address(SuiteId::MlDsa87, &MlDsa87::public_key(&sponsor));
    let dir = TempDir::new().expect("dir");
    let (mut c, height) = live_chain(&dir, &[payer]);
    let pool = Mempool::new(Arc::clone(c.state()));

    // Sixty-five users who hold nothing: the sponsor is what admits them.
    let count = MAX_SPONSORED_PENDING_PER_SPONSOR + 1;
    let frames: Vec<Transaction> = (0..count)
        .map(|i| {
            let seed = [u8::try_from(i).unwrap().wrapping_add(0x80); 32];
            let k = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes(seed));
            let mut tx = Transaction::new(vec![], vec![], 0);
            tx.sign_with_suite::<MlDsa87>(&k, &common::test_chain())
                .expect("sign");
            tx.sponsor_with::<MlDsa87>(
                &sponsor,
                U256::from(GENEROUS),
                height + 100,
                &common::test_chain(),
            )
            .expect("sponsor");
            tx
        })
        .collect();
    for frame in &frames[..MAX_SPONSORED_PENDING_PER_SPONSOR] {
        assert!(pool.insert(frame.clone()).expect("admitted"));
    }
    let err = pool
        .insert(frames[MAX_SPONSORED_PENDING_PER_SPONSOR].clone())
        .unwrap_err();
    assert!(
        matches!(&err, NodeError::MempoolRejected(m) if m.contains("sponsored frames")),
        "{err:?}"
    );
    assert_eq!(pool.len(), MAX_SPONSORED_PENDING_PER_SPONSOR);

    // A block holding all sixty-five is valid: the cap is admission policy.
    let owed = frames.iter().fold(U256::zero(), |s, t| s + required(t));
    let block = c.candidate_block(1_000_001, frames).expect("all apply");
    assert_eq!(block.transactions.len(), count);
    c.insert_block(block).expect("valid in a block");
    assert_eq!(
        c.state().get_account(&payer).expect("read").balance,
        U256::from(FUNDS) - owed
    );
}

// 6, the Σ max_fee half (P-6)
#[test]
fn pending_max_fees_past_the_sponsor_balance_are_refused_at_admission() {
    let alice = user(0x43);
    let key = sponsor_key(0x70);
    let payer = sponsor_address(&key);
    let dir = TempDir::new().expect("dir");
    let (c, height) = live_chain(&dir, &[alice.address(), payer]);
    let pool = Mempool::new(Arc::clone(c.state()));

    // Each frame asks for half the sponsor's balance: two fit, three do not.
    let half = FUNDS / 2;
    for n in 0..2 {
        let frame = sponsored(inner(&alice, n, 1), &key, half, height + 100);
        assert!(pool.insert(frame).expect("admitted"));
    }
    let third = sponsored(inner(&alice, 2, 1), &key, half, height + 100);
    assert!(matches!(
        pool.insert(third).unwrap_err(),
        NodeError::InsufficientBalance { .. }
    ));
    // A max_fee whose sum wraps past 2^256 is "cannot cover", not a small sum.
    let wrap = sponsor_on(
        inner(&alice, 2, 1),
        &key,
        U256::MAX,
        height + 100,
        &common::test_chain(),
    );
    assert!(matches!(
        pool.insert(wrap).unwrap_err(),
        NodeError::InsufficientBalance { .. }
    ));
}
