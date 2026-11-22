//! Red team for sponsored fees (ADR-135 P-4, bit 44, wire v13; V4-V1).
//! Each test is one attack attempt against real block execution or the
//! real mempool, named for the attack, asserting the refusal and that the
//! sponsor paid nothing it did not sign for. This file is the entry point
//! for the V4-V1 classes; the threat-model replays (P-1..P-13) and the drain
//! tests are in `sponsored_fee_tests.rs`.
//!
//! Classes tried (V4-V1 brief):
//! - value creation: `a_block_of_sponsored_frames_creates_no_value` (a
//!   sponsorship moves the fee between two accounts and burns it).
//! - fee bypass: a relay editing the outer frame, a swapped sponsor key,
//!   M1 (padding, quorum swap); P-5 in `sponsored_fee_tests.rs`.
//! - replay: a sponsorship moved to a re-signed inner at the same nonce;
//!   a cached frame after its sponsor suite's sunset. Replay of an applied
//!   frame and under another chain tag are in `sponsored_fee_tests.rs`:
//!   `a_sponsorship_applied_to_a_second_inner_with_the_same_txid_fails_on_the_nonce`
//!   (P-1) and `a_sponsorship_signed_under_another_chain_tag_is_refused` (P-3).
//! - squatting: no namespace, but an inner's one pool slot can be squatted.
//!   `a_smaller_sponsorship_that_will_not_apply_cannot_displace_an_honest_frame`
//!   (first seen kept; a vault cannot pool more than its window takes) and
//!   the limit-0 vault test there. Residual: a race in which an attacker's
//!   frame reaches a node before the honest one (spec 02, Open residuals);
//!   the honest submitter gets "already sponsored", not a silent duplicate.
//! - unbounded per-block work: validity side,
//!   `frame_sizes_bound_sponsored_signature_checks_per_block_by_the_plain_frame_count`
//!   (a formula over the smallest frame sizes, not a count of verify calls:
//!   a sponsored frame carries both its signatures' bytes, so the byte cap
//!   allows no more checks than plain frames); builder side,
//!   `a_sponsor_cannot_pool_sponsorships_its_own_pooled_spend_leaves_unpaid`
//!   and M3, staged in
//!   place for sponsored transfers, pinned by `the_builder_undoes_*` there
//!   and measured by `examples/builder_copy_cost.rs`. M3 is still open for
//!   every other v4 form (BACKLOG, v4 activation blockers).
//! - X1 (sponsor around account authority, with #548): open, no test on
//!   this branch (its `ka:a:` / `ka:w:` records are on #548), a blocker for
//!   the postgen/v4 integration. NOT VERIFIED. Held in code: a schedule
//!   entry naming bit 44 is refused (`state::upgrades`, X1/M3).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use custom_l1_node::consensus::chain::Chain;
use custom_l1_node::core::multisig_tx::{MultisigAuth, multisig_address};
use custom_l1_node::core::transaction::TxOutput;
use custom_l1_node::core::vault_payload::{VaultAction, VaultConfig};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::network::mempool::Mempool;
use custom_l1_node::state::fees::{FEE_COLLECTOR, FEE_KEY, FeeRecord, MAX_BLOCK_BYTES};
use custom_l1_node::state::shielded::FEE_SINK;
use custom_l1_node::state::verified::VerifiedCache;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_crypto_pq::agility::{MIN_EMERGENCY_WINDOW, Network, SuitePolicy};
use maya_crypto_pq::multisig::{Approval, MultisigPolicy, PolicyKey};
use maya_crypto_pq::suite::{
    HybridMlDsa65SlhDsa128s, MasterSeed, MlDsa65, MlDsa87, SignatureSuite, SuiteId, suite_address,
};
use maya_dex::Amount;
use primitive_types::U256;
use tempfile::TempDir;

mod common;

const V4: u64 = 10;
const BASE_FEE: u64 = 2;
const FUNDS: u64 = 100_000_000;
const GENEROUS: u64 = 1_000_000;
const PAYEE: Address = [0x78; 32];

type SponsorKey = <HybridMlDsa65SlhDsa128s as SignatureSuite>::SigningKey;

struct Net {
    db: StateDB,
    _dir: TempDir,
}

fn net(funded: &[Address]) -> Net {
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    common::bind(&db);
    let fees = FeeRecord {
        base_fee: Amount::from_u64(BASE_FEE),
        min_base_fee: Amount::from_u64(BASE_FEE),
        target_block_bytes: 1_000_000_000,
        change_denominator: 8,
    };
    db.raw_put_for_test(FEE_KEY, &fees.encode()).unwrap();
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

fn apply(n: &Net, txs: Vec<Transaction>, height: u64) -> Result<[u8; 32], NodeError> {
    let block = Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        txs,
    );
    let context = BlockContext::at_height(height).with_sponsored_fees_activation(V4);
    n.db.apply_block(&block, context)
}

fn balance(n: &Net, who: &Address) -> U256 {
    n.db.get_account(who).unwrap().balance
}

fn nonce(n: &Net, who: &Address) -> u64 {
    n.db.get_account(who).unwrap().nonce
}

fn user(seed: u8) -> HybridSigningKey {
    signing_key_from_seed(&[seed; 32]).unwrap()
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

fn outputs(amount: u64) -> Vec<TxOutput> {
    vec![TxOutput {
        amount: U256::from(amount),
        recipient: PAYEE,
    }]
}

fn inner(key: &HybridSigningKey, nonce: u64, amount: u64) -> Transaction {
    let mut tx = Transaction::new(vec![], outputs(amount), nonce);
    tx.sign(key, &common::test_chain()).unwrap();
    tx
}

fn sponsored(mut tx: Transaction, key: &SponsorKey, max_fee: u64) -> Transaction {
    tx.sponsor_with::<HybridMlDsa65SlhDsa128s>(
        key,
        U256::from(max_fee),
        V4 + 100,
        &common::test_chain(),
    )
    .unwrap();
    tx
}

fn required(tx: &Transaction) -> U256 {
    U256::from(BASE_FEE) * U256::from(tx.to_bytes().len())
}

fn inner_taken(err: &NodeError) {
    assert!(
        matches!(err, NodeError::MempoolRejected(m) if m.contains("already sponsored")),
        "{err:?}"
    );
}

fn sponsor_signature_refused(err: &NodeError) {
    assert!(
        matches!(err, NodeError::SignatureSuite(m) if m.contains("sponsor")),
        "{err:?}"
    );
}

// Fee bypass by a relay: the sponsor signed max_fee = exactly this frame's
// fee; the relay raises max_fee (to fit a heavier frame later) or extends
// valid_until. Both are under the sponsor's signature, so both are refused,
// and nothing is charged.
#[test]
fn a_relay_that_raises_max_fee_or_extends_valid_until_is_refused() {
    let alice = user(0x51);
    let key = sponsor_key(0x61);
    let payer = sponsor_address(&key);
    let n = net(&[alice.address(), payer]);
    let honest = sponsored(inner(&alice, 0, 1), &key, GENEROUS);

    let mut raised = honest.clone();
    raised.sponsor.as_mut().unwrap().max_fee = U256::from(GENEROUS) * 2;
    sponsor_signature_refused(&apply(&n, vec![raised], V4).unwrap_err());

    let mut extended = honest.clone();
    extended.sponsor.as_mut().unwrap().valid_until = V4 + 1_000_000;
    sponsor_signature_refused(&apply(&n, vec![extended], V4).unwrap_err());

    assert_eq!(balance(&n, &payer), U256::from(FUNDS));
    assert_eq!(nonce(&n, &alice.address()), 0);
    apply(&n, vec![honest.clone()], V4).expect("the untouched frame");
    assert_eq!(balance(&n, &payer), U256::from(FUNDS) - required(&honest));
}

// Payer substitution: a relay keeps the signature and swaps in another
// funded sponsor's public key, so that account would pay. The payer is
// derived from the key, and the key no longer verifies the signature.
#[test]
fn a_swapped_sponsor_key_cannot_make_another_account_pay() {
    let alice = user(0x52);
    let (signer, victim) = (sponsor_key(0x62), sponsor_key(0x63));
    let victim_address = sponsor_address(&victim);
    let n = net(&[alice.address(), sponsor_address(&signer), victim_address]);
    let mut frame = sponsored(inner(&alice, 0, 1), &signer, GENEROUS);
    frame.sponsor.as_mut().unwrap().public_key = HybridMlDsa65SlhDsa128s::public_key(&victim);
    assert_eq!(frame.sponsor.as_ref().unwrap().address(), victim_address);
    sponsor_signature_refused(&apply(&n, vec![frame], V4).unwrap_err());
    assert_eq!(balance(&n, &victim_address), U256::from(FUNDS));
}

// Approval reuse: a sender holds a sponsorship for a 1-unit transfer at
// nonce 0, then re-signs nonce 0 as a larger transfer and staples the old
// sponsorship on. The sponsor signed the old inner's txid, not "nonce 0".
#[test]
fn a_sponsorship_for_one_inner_cannot_pay_for_a_replacement_at_the_same_nonce() {
    let alice = user(0x53);
    let key = sponsor_key(0x64);
    let payer = sponsor_address(&key);
    let n = net(&[alice.address(), payer]);
    let approved = sponsored(inner(&alice, 0, 1), &key, GENEROUS);
    let mut swapped = inner(&alice, 0, 5_000);
    swapped.sponsor = approved.sponsor.clone();
    sponsor_signature_refused(&apply(&n, vec![swapped], V4).unwrap_err());
    assert_eq!(balance(&n, &payer), U256::from(FUNDS));
    assert_eq!(nonce(&n, &alice.address()), 0);
}

// M1 (ADR-135 amendment, #501), tried two ways. As written there ("spare
// approvals can pad the inner") it does not hold: the verifier takes exactly
// `m` approvals, so a third one is refused. The narrower attack: approvals are
// outside the txid, so a relay holding a spare approval can swap *which*
// members signed. With a mixed-suite policy that changes the size: an
// ML-DSA-87 approval (4,627 bytes) for an ML-DSA-65 one (3,309), and the
// sponsor would pay for bytes it never saw. Closed by SP-2 signing
// `inner_len`: the swapped frame is longer, so the sponsor's signature no
// longer verifies and nothing is charged.
#[test]
fn m1_padding_and_a_swapped_mixed_suite_quorum_are_both_refused() {
    let small = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([0x71; 32]));
    let large: Vec<_> = (2..=3u8)
        .map(|i| MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([0x70 + i; 32])))
        .collect();
    let mut keys = vec![PolicyKey {
        suite: MlDsa65::ID,
        public_key: MlDsa65::public_key(&small),
    }];
    keys.extend(large.iter().map(|k| PolicyKey {
        suite: MlDsa87::ID,
        public_key: MlDsa87::public_key(k),
    }));
    let policy = MultisigPolicy::new(2, keys).unwrap();
    let account = multisig_address(&policy);
    let key = sponsor_key(0x65);
    let payer = sponsor_address(&key);
    let n = net(&[account, payer]);

    let mut tx = Transaction::new(vec![], outputs(1), 0);
    tx.multisig = Some(Box::new(MultisigAuth::unsigned(policy.clone())));
    let message = tx.signing_bytes(&common::test_chain());
    let approve = |i: u8| Approval {
        index: i,
        signature: match i {
            0 => MlDsa65::sign(&small, &message).unwrap(),
            _ => MlDsa87::sign(&large[usize::from(i) - 1], &message).unwrap(),
        },
    };
    let with = |approvals: Vec<Approval>| {
        let mut t = tx.clone();
        t.multisig = Some(Box::new(
            MultisigAuth::new(policy.clone(), approvals).unwrap(),
        ));
        t
    };
    let seen = sponsored(with(vec![approve(0), approve(1)]), &key, GENEROUS);

    // Padding with a third approval: refused, nothing charged.
    let mut padded = with(vec![approve(0), approve(1), approve(2)]);
    assert_eq!(padded.inner_txid(), seen.inner_txid());
    padded.sponsor = seen.sponsor.clone();
    let err = apply(&n, vec![padded], V4).unwrap_err();
    assert!(
        matches!(&err, NodeError::SignatureSuite(m) if m.contains("exactly 2")),
        "{err:?}"
    );
    assert_eq!(balance(&n, &payer), U256::from(FUNDS));

    // Swapping the small approval for a large one keeps the txid.
    let mut swapped = with(vec![approve(1), approve(2)]);
    assert_eq!(swapped.inner_txid(), seen.inner_txid());
    swapped.sponsor = seen.sponsor.clone();
    assert!(required(&swapped) > required(&seen));

    // Under any ceiling, even a generous one, the swapped frame is refused:
    // the sponsor signed the inner's length, and the swap changed it.
    let err = apply(&n, vec![swapped], V4).unwrap_err();
    sponsor_signature_refused(&err);
    assert_eq!(balance(&n, &payer), U256::from(FUNDS));

    // Control: the frame the sponsor saw applies and costs what it saw.
    apply(&n, vec![seen.clone()], V4).expect("the frame the sponsor signed");
    assert_eq!(balance(&n, &payer), U256::from(FUNDS) - required(&seen));
}

// ---- Mempool, value, per-block work, sunset (V4-V1 review) --------------

type Ml87Key = <MlDsa87 as SignatureSuite>::SigningKey;

fn live_chain(dir: &TempDir, funded: &[Address]) -> (Chain, u64) {
    common::sponsored_chain(dir, funded, U256::from(FUNDS), U256::from(BASE_FEE))
}

fn ml87(seed: u8) -> (Ml87Key, Address) {
    let key = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([seed; 32]));
    let address = suite_address(SuiteId::MlDsa87, &MlDsa87::public_key(&key));
    (key, address)
}

/// `tx` sponsored by the ML-DSA-87 `key`: a smaller outer frame than the
/// hybrid sponsorship [`sponsored`] makes.
fn sponsored_87(mut tx: Transaction, key: &Ml87Key, valid_until: u64) -> Transaction {
    tx.sponsor_with::<MlDsa87>(
        key,
        U256::from(GENEROUS),
        valid_until,
        &common::test_chain(),
    )
    .unwrap();
    tx
}

/// A v7 frame from `key`, paying its own fee, that gives its account a vault
/// with an instant limit of `limit` per window.
fn vault_87(key: &Ml87Key, limit: U256) -> Transaction {
    let build = |fee: U256| {
        let config = VaultConfig {
            delay_blocks: 100,
            limit,
            guardians: vec![[0x99; 32]],
        };
        let kind = TxKind::Vault(Box::new(VaultAction::Configure(config)));
        let mut tx = Transaction::with_kind(kind, 0);
        tx.outputs = vec![TxOutput {
            amount: fee,
            recipient: FEE_COLLECTOR,
        }];
        tx.sign_with_suite::<MlDsa87>(key, &common::test_chain())
            .unwrap();
        tx
    };
    // A fee's value does not change the frame's size.
    build(required(&build(U256::zero())))
}

// Squatting through the old P-12 rule ("the smaller outer frame replaces"):
// a sponsor on a smaller suite displaces an honest hybrid sponsorship, then
// makes sure its own frame never applies. Two of the three variants the
// review named: (a) `valid_until` = the admission height, so it expires if
// the next block leaves it out; (c) a vault window with room for one fee,
// which the per-frame dry-run passed for each of 64 frames. (b), a sponsor
// that spends to zero after displacing, needs the displacement this refuses.
#[test]
fn a_smaller_sponsorship_that_will_not_apply_cannot_displace_an_honest_frame() {
    let (alice, bob) = (user(0xA0), user(0xA1));
    let honest = sponsor_key(0xA2);
    let (expiring, expiring_payer) = ml87(0xA3);
    let (vaulted, vaulted_payer) = ml87(0xA4);
    let dir = TempDir::new().unwrap();
    let (mut c, height) = live_chain(
        &dir,
        &[
            alice.address(),
            bob.address(),
            sponsor_address(&honest),
            expiring_payer,
            vaulted_payer,
        ],
    );
    // Signed only once the chain is open: `test_chain` is its genesis.
    let one_fee = required(&sponsored_87(inner(&alice, 0, 1), &vaulted, height + 100));
    let block = c
        .candidate_block(1_000_001, vec![vault_87(&vaulted, one_fee)])
        .unwrap();
    c.insert_block(block).expect("the vault sponsor configures");
    let admit_at = height + 1;

    let pool = Mempool::new(Arc::clone(c.state()));
    let by_honest = sponsored(inner(&alice, 0, 1), &honest, GENEROUS);
    assert!(pool.insert(by_honest.clone()).expect("honest admitted"));
    let squats = [
        sponsored_87(inner(&alice, 0, 1), &expiring, admit_at),
        sponsored_87(inner(&alice, 0, 1), &vaulted, admit_at + 100),
    ];
    for squat in squats {
        assert_eq!(squat.inner_txid(), by_honest.inner_txid(), "one inner");
        assert!(squat.to_bytes().len() < by_honest.to_bytes().len());
        // Refused by name, not Ok(false) like a duplicate, so the
        // submitter knows to resubmit after the pooled frame is evicted.
        inner_taken(&pool.insert(squat).unwrap_err());
    }
    assert_eq!(pool.len(), 1);
    assert!(pool.contains(&by_honest.txid()), "the honest frame stays");

    // (c) cannot fill a pool either: the window must take every pooled
    // max_fee of this sponsor plus this frame's fee.
    let fresh = Mempool::new(Arc::clone(c.state()));
    let first = sponsored_87(inner(&alice, 0, 1), &vaulted, admit_at + 100);
    assert!(fresh.insert(first).expect("one fee fits the window"));
    let second = sponsored_87(inner(&bob, 0, 1), &vaulted, admit_at + 100);
    let err = fresh.insert(second).unwrap_err();
    assert!(matches!(err, NodeError::Vault(_)), "{err:?}");
    assert_eq!(fresh.len(), 1);
}

// Builder work through the pool: a sponsor pools its own spend of almost
// all its balance, then sponsorships whose max_fee only the spent balance
// covered. Counted apart, both passed and the builder verified and staged
// frames that fail at apply. The sponsor's pooled spends count against its
// sponsored max_fees.
#[test]
fn a_sponsor_cannot_pool_sponsorships_its_own_pooled_spend_leaves_unpaid() {
    let alice = user(0xAC);
    let (key, payer) = ml87(0xAD);
    let dir = TempDir::new().unwrap();
    let (c, height) = live_chain(&dir, &[alice.address(), payer]);
    let spend = |fee: U256| {
        let mut tx = Transaction::new(vec![], outputs(FUNDS - GENEROUS), 0);
        tx.outputs.push(TxOutput {
            amount: fee,
            recipient: FEE_COLLECTOR,
        });
        tx.sign_with_suite::<MlDsa87>(&key, &common::test_chain())
            .unwrap();
        tx
    };
    let drain = spend(required(&spend(U256::zero())));

    let pool = Mempool::new(Arc::clone(c.state()));
    assert!(pool.insert(drain).expect("the sponsor's own spend"));
    let frame = sponsored_87(inner(&alice, 0, 1), &key, height + 100);
    let err = pool.insert(frame).unwrap_err();
    assert!(
        matches!(err, NodeError::InsufficientBalance { .. }),
        "{err:?}"
    );
    assert_eq!(pool.len(), 1);
}

// Value creation: a block of sponsored frames from two senders, one
// sponsor. Every unit is accounted for: the transfers reach the payee, the
// fees leave the sponsor and burn (P-11), nothing else moves.
#[test]
fn a_block_of_sponsored_frames_creates_no_value() {
    let (alice, bob) = (user(0xA5), user(0xA6));
    let key = sponsor_key(0xA7);
    let payer = sponsor_address(&key);
    let n = net(&[alice.address(), bob.address(), payer]);
    let everyone = [
        alice.address(),
        bob.address(),
        payer,
        PAYEE,
        FEE_COLLECTOR,
        FEE_SINK,
    ];
    let total = |n: &Net| everyone.iter().fold(U256::zero(), |s, a| s + balance(n, a));
    let before = total(&n);

    let frames = vec![
        sponsored(inner(&alice, 0, 5), &key, GENEROUS),
        sponsored(inner(&bob, 0, 7), &key, GENEROUS),
        sponsored(inner(&alice, 1, 9), &key, GENEROUS),
    ];
    let owed = frames.iter().fold(U256::zero(), |s, t| s + required(t));
    apply(&n, frames, V4).expect("all three apply");
    assert_eq!(total(&n), before, "no value created or destroyed");
    assert_eq!(balance(&n, &payer), U256::from(FUNDS) - owed);
    assert_eq!(balance(&n, &FEE_SINK), owed, "the fees burn");
    assert_eq!(balance(&n, &PAYEE), U256::from(21u64));
}

/// FIPS 204 ML-DSA-65 signature bytes: the smallest signature the consensus
/// suite policy admits (ML-DSA-87 4,627, SLH-DSA-128s 7,856; Ed25519 refused).
const ML_DSA_65_SIGNATURE_LEN: usize = 3_309;

// Unbounded per-block work, validity side: a block full of sponsored frames,
// each asking for two signature checks. Each check is carried by at least
// one signature's bytes, so a block of MAX_BLOCK_BYTES asks for at most
// MAX_BLOCK_BYTES / 3,309 checks whether or not its frames are sponsored
// (5 MiB / 3,309 = 1,584). Measured 2026-11-22: the smallest sponsored
// frame is 10,612 B (988 checks per 5 MiB block), the smallest plain one
// 5,297 B (989 checks). A count bound, not a time bound: an SLH-DSA check
// costs more than an ML-DSA one, sponsored or not. The cap is a governance
// parameter (5 MiB at genesis); the bound scales with it.
#[test]
fn frame_sizes_bound_sponsored_signature_checks_per_block_by_the_plain_frame_count() {
    let user_key = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([0xA8; 32]));
    let sponsor = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([0xA9; 32]));
    // The smallest frames: an empty ML-DSA-65 inner, an ML-DSA-65 sponsor.
    let mut plain = Transaction::new(vec![], vec![], 0);
    plain
        .sign_with_suite::<MlDsa65>(&user_key, &common::test_chain())
        .unwrap();
    let mut frame = plain.clone();
    frame
        .sponsor_with::<MlDsa65>(
            &sponsor,
            U256::from(GENEROUS),
            V4 + 100,
            &common::test_chain(),
        )
        .unwrap();
    let (plain_len, frame_len) = (plain.to_bytes().len(), frame.to_bytes().len());
    assert!(
        frame_len >= 2 * ML_DSA_65_SIGNATURE_LEN,
        "both signatures ride in the frame: {frame_len} bytes"
    );

    let cap = usize::try_from(MAX_BLOCK_BYTES).unwrap();
    let ceiling = cap / ML_DSA_65_SIGNATURE_LEN;
    let sponsored_checks = 2 * (cap / frame_len);
    let plain_checks = cap / plain_len;
    assert!(
        sponsored_checks <= ceiling,
        "{sponsored_checks} > {ceiling}"
    );
    assert!(plain_checks <= ceiling, "{plain_checks} > {ceiling}");
    assert!(
        sponsored_checks <= plain_checks,
        "sponsoring raises per-block checks: {sponsored_checks} > {plain_checks}"
    );
    println!(
        "block cap {cap} B: sponsored frame {frame_len} B -> {sponsored_checks} checks; \
         plain frame {plain_len} B -> {plain_checks} checks; ceiling {ceiling}"
    );
}

// Replay across a sunset: a v13 frame verified, and cached, while its
// sponsor suite may still sign, then offered again after the sunset. A
// warm-cache node that answered from the cache would accept what a
// cold-cache node refuses: a state-root split. Both must refuse.
#[test]
fn a_cached_v13_frame_is_refused_once_its_sponsor_suite_sunsets() {
    let alice = user(0xAA);
    let (sponsor, _) = ml87(0xAB);
    let frame = sponsored_87(inner(&alice, 0, 1), &sponsor, u64::MAX);
    // Deprecated at 0 with the emergency window: signs before it, not after.
    let sunset_at = MIN_EMERGENCY_WINDOW;
    let policy = SuitePolicy::genesis(Network::Mainnet)
        .with_default(SuiteId::MlDsa65)
        .and_then(|p| p.with_deprecation(SuiteId::MlDsa87, 0, MIN_EMERGENCY_WINDOW, true))
        .unwrap();
    let refused = |r: Result<(), NodeError>| {
        assert!(
            matches!(&r, Err(NodeError::SignatureSuite(m)) if m.contains("MlDsa87")),
            "{r:?}"
        );
    };

    let cache = VerifiedCache::default();
    cache
        .verify(&frame, sunset_at - 1, &policy, &common::test_chain())
        .expect("inside the window");
    assert_eq!(cache.len(), 1, "cached");
    refused(cache.verify(&frame, sunset_at, &policy, &common::test_chain()));
    refused(frame.verify_at(sunset_at, &policy, &common::test_chain()));
}
