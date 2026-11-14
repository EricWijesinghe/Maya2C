//! Red team for sponsored fees (ADR-135 P-4, bit 44, wire v13; V4-V1).
//! Each test is one attack attempt against real block execution, named for
//! the attack, asserting the refusal and that the sponsor paid nothing it
//! did not sign for. The threat-model replays (P-1..P-12) and the drain
//! tests are in `sponsored_fee_tests.rs`; this file is a relay that edits
//! the outer frame after the sponsor signed it, and M1 tried two ways.
//!
//! Classes tried (V4-V1 brief): value creation (none: a sponsorship moves
//! the fee between two accounts, `sponsored_fee_tests` checks the total),
//! fee bypass (here, M1, and P-5 there), unbounded per-block work (M3, open:
//! the builder's per-frame overlay copy, not measured here), replay (P-1
//! there), squatting (no namespace).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use custom_l1_node::core::multisig_tx::{MultisigAuth, multisig_address};
use custom_l1_node::core::transaction::TxOutput;
use custom_l1_node::core::{Block, BlockHeader, Transaction};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::fees::{FEE_KEY, FeeRecord};
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
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
// `m` approvals, so a third one is refused. What remains open is narrower:
// approvals are outside the txid, so a relay holding a spare approval can
// swap *which* members signed. With a mixed-suite policy that changes the
// size: an ML-DSA-87 approval (4,627 bytes) for an ML-DSA-65 one (3,309), and
// the sponsor pays for bytes it never saw, up to max_fee and never past it.
// When the fix lands (the sponsor signs `inner_len` or the approvals), the
// swapped frame must be refused and the last assertion block flips.
#[test]
fn m1_padding_is_refused_but_a_swapped_mixed_suite_quorum_still_costs_the_sponsor_more() {
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

    // A ceiling at the fee the sponsor saw still binds.
    let tight = sponsored(
        with(vec![approve(0), approve(1)]),
        &key,
        required(&seen).as_u64(),
    );
    let mut tight_swapped = with(vec![approve(1), approve(2)]);
    tight_swapped.sponsor = tight.sponsor.clone();
    let err = apply(&n, vec![tight_swapped], V4).unwrap_err();
    assert!(
        matches!(&err, NodeError::SponsorshipRefused { reason } if reason.contains("max_fee")),
        "{err:?}"
    );

    // The open residual: under a generous ceiling the swapped frame applies
    // and the sponsor pays for the larger frame.
    apply(&n, vec![swapped.clone()], V4).expect("M1 residual: the swapped quorum applies");
    assert_eq!(balance(&n, &payer), U256::from(FUNDS) - required(&swapped));
    assert!(required(&swapped) <= U256::from(GENEROUS));
}
