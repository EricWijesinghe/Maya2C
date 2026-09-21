//! m-of-n multisig on the node (wire version 8): a 3-of-5 ML-DSA account that
//! spends with two members offline, and the ways a quorum cannot be faked.
//!
//! Dark on the same gate as the v7 envelope, so every accepting check runs at
//! `SUITE_ENVELOPE_ACTIVATION_HEIGHT` and every height below it refuses.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::multisig_tx::{MultisigAuth, multisig_address};
use custom_l1_node::core::transaction::{Transaction, TxInput, TxOutput};
use custom_l1_node::crypto::suites::suite_address;
use custom_l1_node::state::context::SUITE_ENVELOPE_ACTIVATION_HEIGHT;
use maya_crypto_pq::agility::{Network, SuitePolicy};
use maya_crypto_pq::multisig::{Approval, MultisigPolicy, PolicyKey};
use maya_crypto_pq::suite::{MasterSeed, MlDsa87, SignatureSuite};

const AT: u64 = SUITE_ENVELOPE_ACTIVATION_HEIGHT;

type Key = <MlDsa87 as SignatureSuite>::SigningKey;

fn members() -> Vec<Key> {
    (1..=5u8)
        .map(|i| MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([i; 32])))
        .collect()
}

fn policy(keys: &[Key]) -> MultisigPolicy {
    let listed = keys
        .iter()
        .map(|k| PolicyKey {
            suite: MlDsa87::ID,
            public_key: MlDsa87::public_key(k),
        })
        .collect();
    MultisigPolicy::new(3, listed).expect("3-of-5")
}

/// An unsigned spend from the policy's account.
fn draft(policy: MultisigPolicy) -> Transaction {
    let mut tx = Transaction::new(
        vec![TxInput {
            prev_tx: [7; 32],
            index: 0,
        }],
        vec![TxOutput {
            amount: 40,
            recipient: [9; 32],
        }],
        3,
    );
    tx.multisig = Some(Box::new(MultisigAuth::unsigned(policy)));
    tx
}

/// Collects approvals from `signers` over the draft's signing bytes.
fn approve(mut tx: Transaction, keys: &[Key], signers: &[u8]) -> Transaction {
    let message = tx.signing_bytes();
    let approvals = signers
        .iter()
        .map(|&i| Approval {
            index: i,
            signature: MlDsa87::sign(&keys[usize::from(i)], &message).expect("sign"),
        })
        .collect();
    let policy = tx.multisig.as_ref().expect("multisig").policy().clone();
    tx.multisig = Some(Box::new(
        MultisigAuth::new(policy, approvals).expect("shape"),
    ));
    tx
}

#[test]
fn three_of_five_spends_with_two_members_offline() {
    let keys = members();
    let tx = approve(draft(policy(&keys)), &keys, &[0, 2, 4]);
    let genesis = SuitePolicy::genesis(Network::Mainnet);
    assert_eq!(tx.verify_at(AT, &genesis), Ok(()));

    // The wire round-trips exactly, and the decoded frame verifies too.
    let decoded = Transaction::from_bytes(&tx.to_bytes()).expect("decode");
    assert_eq!(decoded, tx);
    assert_eq!(decoded.txid(), tx.txid());
    assert_eq!(decoded.verify_at(AT, &genesis), Ok(()));
}

#[test]
fn the_account_is_the_policy() {
    let keys = members();
    let p = policy(&keys);
    let tx = draft(p.clone());
    assert_eq!(tx.sender(), multisig_address(&p));
    // No member's own account is the multisig account.
    for key in &keys {
        assert_ne!(
            tx.sender(),
            suite_address(MlDsa87::ID, &MlDsa87::public_key(key))
        );
    }
    // A 2-of-5 over the same keys is a different account.
    let looser = MultisigPolicy::new(2, p.keys().to_vec()).expect("2-of-5");
    assert_ne!(multisig_address(&looser), multisig_address(&p));
}

#[test]
fn it_is_dark_before_activation_and_through_verify() {
    let keys = members();
    let tx = approve(draft(policy(&keys)), &keys, &[0, 1, 2]);
    let genesis = SuitePolicy::genesis(Network::Mainnet);
    assert!(
        tx.verify().is_err(),
        "verify() has no height and refuses v8"
    );
    for height in [0, 1, AT - 1] {
        assert!(tx.verify_at(height, &genesis).is_err(), "height {height}");
    }
}

#[test]
fn two_approvals_do_not_spend_a_three_of_five() {
    let keys = members();
    let tx = approve(draft(policy(&keys)), &keys, &[1, 3]);
    assert!(
        tx.verify_at(AT, &SuitePolicy::genesis(Network::Mainnet))
            .is_err()
    );
}

#[test]
fn a_changed_output_invalidates_every_approval() {
    let keys = members();
    let mut tx = approve(draft(policy(&keys)), &keys, &[0, 1, 2]);
    tx.outputs[0].amount = 4_000;
    assert!(
        tx.verify_at(AT, &SuitePolicy::genesis(Network::Mainnet))
            .is_err()
    );
}

#[test]
fn swapping_in_a_different_policy_is_a_different_sender_and_fails() {
    // Approvals by 0,1,2 re-presented under a policy where those three keys
    // are listed in another order: the signed bytes commit to the policy.
    let keys = members();
    let tx = approve(draft(policy(&keys)), &keys, &[0, 1, 2]);
    let mut reordered: Vec<Key> = members();
    reordered.swap(0, 3);
    let mut forged = tx.clone();
    let approvals = tx.multisig.as_ref().expect("multisig").approvals().to_vec();
    forged.multisig = Some(Box::new(
        MultisigAuth::new(policy(&reordered), approvals).expect("shape"),
    ));
    assert_ne!(forged.sender(), tx.sender());
    assert!(
        forged
            .verify_at(AT, &SuitePolicy::genesis(Network::Mainnet))
            .is_err()
    );
}

#[test]
fn hostile_frames_are_refused_before_any_signature_is_read() {
    let keys = members();
    let tx = approve(draft(policy(&keys)), &keys, &[0, 1, 2]);
    let bytes = tx.to_bytes();

    // Truncated anywhere, and one trailing byte.
    for cut in [1, bytes.len() / 2, bytes.len() - 1] {
        assert!(
            Transaction::from_bytes(&bytes[..cut]).is_err(),
            "cut at {cut}"
        );
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(Transaction::from_bytes(&trailing).is_err());

    // Locate the approval count: after version, io, policy and nonce.
    let io = 8 + 36 + 8 + 40;
    let policy_len = tx
        .multisig
        .as_ref()
        .expect("multisig")
        .policy()
        .encode()
        .len();
    let count_at = 1 + io + policy_len + 8;
    assert_eq!(bytes[count_at], 3);

    let mut too_many = bytes.clone();
    too_many[count_at] = 6;
    assert!(
        Transaction::from_bytes(&too_many).is_err(),
        "more approvals than keys"
    );

    let mut bad_index = bytes.clone();
    bad_index[count_at + 1] = 9;
    assert!(
        Transaction::from_bytes(&bad_index).is_err(),
        "index outside the policy"
    );

    let mut bad_len = bytes;
    bad_len[count_at + 2] ^= 0xff;
    assert!(
        Transaction::from_bytes(&bad_len).is_err(),
        "signature length"
    );
}

#[test]
fn every_quorum_of_one_spend_has_one_txid() {
    // Two different quorums, and a superset, authorize the same spend. If the
    // txid hashed approvals, anyone holding a spare one could re-encode a
    // broadcast transaction under a new id and orphan its children.
    let keys = members();
    let genesis = SuitePolicy::genesis(Network::Mainnet);
    let a = approve(draft(policy(&keys)), &keys, &[0, 1, 2]);
    let b = approve(draft(policy(&keys)), &keys, &[2, 3, 4]);
    let all = approve(draft(policy(&keys)), &keys, &[0, 1, 2, 3, 4]);
    for tx in [&a, &b, &all] {
        assert_eq!(tx.verify_at(AT, &genesis), Ok(()));
    }
    assert_ne!(
        a.to_bytes(),
        b.to_bytes(),
        "different witnesses on the wire"
    );
    assert_eq!(a.txid(), b.txid());
    assert_eq!(a.txid(), all.txid());
}

#[test]
fn an_auth_cannot_be_built_in_a_shape_no_node_decodes() {
    let keys = members();
    let approvals = approve(draft(policy(&keys)), &keys, &[0, 1, 2])
        .multisig
        .expect("multisig")
        .approvals()
        .to_vec();
    let mut reversed = approvals.clone();
    reversed.reverse();
    assert!(MultisigAuth::new(policy(&keys), reversed).is_err());
    let mut repeated = approvals.clone();
    repeated[1] = repeated[0].clone();
    assert!(MultisigAuth::new(policy(&keys), repeated).is_err());
    let mut outside = approvals;
    outside[2].index = 5;
    assert!(MultisigAuth::new(policy(&keys), outside).is_err());
}
