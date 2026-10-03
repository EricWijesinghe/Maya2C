//! The suite envelope (v7) and multisig (v8) through the real apply path and
//! the real mempool, at the heights the node actually builds — ADR-013.
//!
//! `suite_parity_tests` and `multisig_tests` check `verify_at` directly. These
//! check what that is for: that value actually moves. A verifier that accepts a
//! frame the executor never reaches would be a rule with nothing behind it, and
//! that is exactly the state this crate was in while the envelope was dark.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use tempfile::TempDir;

use custom_l1_node::core::multisig_tx::{MultisigAuth, multisig_address};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::core::{Block, BlockHeader};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::crypto::suites::suite_address;
use custom_l1_node::network::mempool::Mempool;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_crypto_pq::multisig::{Approval, MultisigPolicy, PolicyKey};
use maya_crypto_pq::suite::{Ed25519, MasterSeed, MlDsa65, MlDsa87, SignatureSuite, SuiteId};

/// The chain test signatures commit to (ADR-036).
const CHAIN: custom_l1_node::core::ChainTag =
    custom_l1_node::core::ChainTag::from_genesis([42; 32]);

const FUNDS: u64 = 1_000;
const RECIPIENT: Address = [0x5a; 32];

type Key87 = <MlDsa87 as SignatureSuite>::SigningKey;

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_789_200_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn open_db(funded: &[Address]) -> (StateDB, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    db.bind_chain(CHAIN).expect("bind the test chain");
    for address in funded {
        db.put_account(
            address,
            &Account {
                balance: FUNDS,
                nonce: 0,
            },
        )
        .expect("fund");
    }
    (db, dir)
}

fn pay(amount: u64, nonce: u64) -> Transaction {
    Transaction::new(
        Vec::new(),
        vec![TxOutput {
            amount,
            recipient: RECIPIENT,
        }],
        nonce,
    )
}

fn v7<S: SignatureSuite>(key: &S::SigningKey, amount: u64, nonce: u64) -> Transaction {
    let mut tx = pay(amount, nonce);
    tx.sign_with_suite::<S>(key, &CHAIN).expect("sign");
    tx
}

fn members() -> Vec<Key87> {
    (1..=5u8)
        .map(|i| MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([0x40 + i; 32])))
        .collect()
}

fn three_of_five(keys: &[Key87]) -> MultisigPolicy {
    let listed = keys
        .iter()
        .map(|k| PolicyKey {
            suite: SuiteId::MlDsa87,
            public_key: MlDsa87::public_key(k),
        })
        .collect();
    MultisigPolicy::new(3, listed).expect("3-of-5")
}

fn v8(keys: &[Key87], signers: &[u8], amount: u64) -> Transaction {
    let policy = three_of_five(keys);
    let mut tx = pay(amount, 0);
    tx.multisig = Some(Box::new(MultisigAuth::unsigned(policy.clone())));
    let message = tx.signing_bytes(&CHAIN);
    let approvals = signers
        .iter()
        .map(|&i| Approval {
            index: i,
            signature: MlDsa87::sign(&keys[usize::from(i)], &message).expect("sign"),
        })
        .collect();
    tx.multisig = Some(Box::new(
        MultisigAuth::new(policy, approvals).expect("shape"),
    ));
    tx
}

#[test]
fn a_v7_transfer_moves_value_at_the_production_context() {
    let key = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([0x11; 32]));
    let sender = suite_address(SuiteId::MlDsa87, &MlDsa87::public_key(&key));
    let (db, _dir) = open_db(&[sender]);

    db.apply_block(
        &block_of(vec![v7::<MlDsa87>(&key, 250, 0)]),
        BlockContext::at_height(1),
    )
    .expect("a v7 transfer applies");

    let after = db.get_account(&sender).expect("sender");
    assert_eq!((after.balance, after.nonce), (FUNDS - 250, 1));
    assert_eq!(db.get_account(&RECIPIENT).expect("recipient").balance, 250);
}

#[test]
fn a_tampered_v7_transfer_is_refused_and_moves_nothing() {
    let key = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([0x12; 32]));
    let sender = suite_address(SuiteId::MlDsa87, &MlDsa87::public_key(&key));
    let (db, _dir) = open_db(&[sender]);

    let mut forged = v7::<MlDsa87>(&key, 250, 0);
    forged.outputs[0].amount = FUNDS;
    assert!(
        db.apply_block(&block_of(vec![forged]), BlockContext::at_height(1))
            .is_err()
    );
    assert_eq!(db.get_account(&sender).expect("sender").balance, FUNDS);
    assert_eq!(db.get_account(&RECIPIENT).expect("recipient").balance, 0);
}

#[test]
fn a_three_of_five_multisig_spends_through_the_apply_path() {
    let keys = members();
    let account = multisig_address(&three_of_five(&keys));
    let (db, _dir) = open_db(&[account]);

    // Members 1 and 3 offline: 0, 2 and 4 are the quorum.
    db.apply_block(
        &block_of(vec![v8(&keys, &[0, 2, 4], 400)]),
        BlockContext::at_height(1),
    )
    .expect("a 3-of-5 quorum spends");
    assert_eq!(
        db.get_account(&account).expect("account").balance,
        FUNDS - 400
    );
    assert_eq!(db.get_account(&RECIPIENT).expect("recipient").balance, 400);
}

#[test]
fn two_approvals_cannot_spend_a_three_of_five_through_the_apply_path() {
    let keys = members();
    let account = multisig_address(&three_of_five(&keys));
    let (db, _dir) = open_db(&[account]);

    assert!(
        db.apply_block(
            &block_of(vec![v8(&keys, &[1, 3], 400)]),
            BlockContext::at_height(1)
        )
        .is_err()
    );
    assert_eq!(db.get_account(&account).expect("account").balance, FUNDS);
}

#[test]
fn the_mempool_admits_a_v7_transfer_and_refuses_ed25519() {
    let pq = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([0x13; 32]));
    let pq_sender = suite_address(SuiteId::MlDsa65, &MlDsa65::public_key(&pq));
    let classical = Ed25519::signing_key_from_seed(&MasterSeed::from_bytes([0x14; 32]));
    let classical_sender = suite_address(SuiteId::Ed25519, &Ed25519::public_key(&classical));
    let (db, _dir) = open_db(&[pq_sender, classical_sender]);
    let mempool = Mempool::new(Arc::new(db));

    assert_eq!(mempool.validate(&v7::<MlDsa65>(&pq, 10, 0)), Ok(()));
    // Ed25519 has no post-quantum security. Mainnet rules refuse it whatever
    // the node thinks its network is, and so does the admission path.
    assert!(mempool.validate(&v7::<Ed25519>(&classical, 10, 0)).is_err());
}

#[test]
fn the_height_less_verify_still_refuses_both_frames() {
    let key = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([0x15; 32]));
    assert!(v7::<MlDsa87>(&key, 1, 0).verify(&CHAIN).is_err());
    assert!(v8(&members(), &[0, 1, 2], 1).verify(&CHAIN).is_err());
}

#[test]
fn a_v7_transfer_with_a_stale_nonce_is_refused_through_the_apply_path() {
    let key = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([0x16; 32]));
    let sender = suite_address(SuiteId::MlDsa87, &MlDsa87::public_key(&key));
    let (db, _dir) = open_db(&[sender]);
    db.apply_block(
        &block_of(vec![v7::<MlDsa87>(&key, 100, 0)]),
        BlockContext::at_height(1),
    )
    .expect("first spend");

    // A validly signed replay of nonce 0, and a jump to nonce 5.
    for nonce in [0, 5] {
        assert!(
            db.apply_block(
                &block_of(vec![v7::<MlDsa87>(&key, 100, nonce)]),
                BlockContext::at_height(2)
            )
            .is_err(),
            "nonce {nonce}"
        );
    }
    assert_eq!(
        db.get_account(&sender).expect("sender").balance,
        FUNDS - 100
    );
}

#[test]
fn a_multisig_policy_naming_ed25519_cannot_spend_even_with_a_pq_quorum() {
    // Four ML-DSA-87 members who all approve, plus one Ed25519 key in the
    // policy that never signs. Every *listed* key's suite must be admissible,
    // not just the approvers', or a classical key could sit in a policy the
    // chain treats as post-quantum.
    let pq = members();
    let classical = Ed25519::signing_key_from_seed(&MasterSeed::from_bytes([0x17; 32]));
    let mut listed: Vec<PolicyKey> = pq[..4]
        .iter()
        .map(|k| PolicyKey {
            suite: SuiteId::MlDsa87,
            public_key: MlDsa87::public_key(k),
        })
        .collect();
    listed.push(PolicyKey {
        suite: SuiteId::Ed25519,
        public_key: Ed25519::public_key(&classical),
    });
    let Ok(policy) = MultisigPolicy::new(3, listed) else {
        // Refused at construction is also a refusal; nothing can be funded.
        return;
    };
    let account = multisig_address(&policy);
    let (db, _dir) = open_db(&[account]);

    let mut tx = pay(400, 0);
    tx.multisig = Some(Box::new(MultisigAuth::unsigned(policy.clone())));
    let message = tx.signing_bytes(&CHAIN);
    let approvals = (0..3u8)
        .map(|i| Approval {
            index: i,
            signature: MlDsa87::sign(&pq[usize::from(i)], &message).expect("sign"),
        })
        .collect();
    tx.multisig = Some(Box::new(
        MultisigAuth::new(policy, approvals).expect("shape"),
    ));

    assert!(
        db.apply_block(&block_of(vec![tx]), BlockContext::at_height(1))
            .is_err()
    );
    assert_eq!(db.get_account(&account).expect("account").balance, FUNDS);
}
