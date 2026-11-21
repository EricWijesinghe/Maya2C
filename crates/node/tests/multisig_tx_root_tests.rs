//! A v8 multisig transaction's id leaves its approvals out (one spend, one
//! id, whichever quorum signed). The block's `tx_root` must still commit to
//! them, or a relay can file a body with altered approvals under an honest
//! block id (invariant 24).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::multisig_tx::MultisigAuth;
use custom_l1_node::core::transaction::{Transaction, TxInput, TxOutput};
use custom_l1_node::core::{Block, BlockHeader};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use maya_crypto_pq::multisig::{Approval, MultisigPolicy, PolicyKey};
use maya_crypto_pq::suite::{MasterSeed, MlDsa87, SignatureSuite};

const CHAIN: custom_l1_node::core::ChainTag =
    custom_l1_node::core::ChainTag::from_genesis([42; 32]);

type Key = <MlDsa87 as SignatureSuite>::SigningKey;

fn members() -> Vec<Key> {
    (1..=3u8)
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
    MultisigPolicy::new(2, listed).expect("2-of-3")
}

/// A 2-of-3 spend whose first approval signs `first_message` (the honest
/// signing bytes when `None`).
fn spend(keys: &[Key], first_message: Option<&[u8]>) -> Transaction {
    let mut tx = Transaction::new(
        vec![TxInput {
            prev_tx: [7; 32],
            index: 0,
        }],
        vec![TxOutput {
            amount: primitive_types::U256::from(40),
            recipient: [9; 32],
        }],
        3,
    );
    let policy = policy(keys);
    tx.multisig = Some(Box::new(MultisigAuth::unsigned(policy.clone())));
    let honest = tx.signing_bytes(&CHAIN);
    let approvals = vec![
        Approval {
            index: 0,
            signature: MlDsa87::sign(&keys[0], first_message.unwrap_or(&honest)).expect("sign"),
        },
        Approval {
            index: 1,
            signature: MlDsa87::sign(&keys[1], &honest).expect("sign"),
        },
    ];
    tx.multisig = Some(Box::new(
        MultisigAuth::new(policy, approvals).expect("shape"),
    ));
    tx
}

fn header() -> BlockHeader {
    BlockHeader {
        version: 0,
        prev_hash: [3; 32],
        state_root: [4; 32],
        timestamp: 1_756_252_800,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    }
}

#[test]
fn altered_approvals_under_an_honest_header_fail_the_tx_root_check() {
    let keys = members();
    let honest = Block::new(header(), vec![spend(&keys, None)]);
    let altered_tx = spend(&keys, Some(b"not the transaction"));

    // The id does not see the approvals, so it cannot tell the bodies apart.
    assert_eq!(altered_tx.txid(), honest.transactions[0].txid());
    assert_ne!(altered_tx.to_bytes(), honest.transactions[0].to_bytes());

    // The same header carrying the altered body: the root must notice.
    let altered = Block {
        header: honest.header.clone(),
        transactions: vec![altered_tx],
    };
    assert!(
        altered.check_tx_root().is_err(),
        "a v8 body with different approvals passed the header's tx_root"
    );
}

#[test]
fn the_honest_multisig_block_still_checks() {
    let keys = members();
    let block = Block::new(header(), vec![spend(&keys, None)]);
    block.check_tx_root().expect("honest root");
}

#[test]
fn a_multisig_inclusion_proof_needs_the_whole_frame() {
    use custom_l1_node::core::{block_leaf, transaction_leaf};
    use custom_l1_node::state::merkle::verify_path;

    let keys = members();
    let tx = spend(&keys, None);
    let block = Block::new(header(), vec![tx.clone(), spend(&keys, Some(b"other"))]);
    let path = block.tx_inclusion_path(0).expect("in range");
    assert_eq!(verify_path(&block_leaf(&tx), &path), block.header.tx_root);
    // The id alone no longer proves a v8 frame.
    assert_ne!(
        verify_path(&transaction_leaf(&tx.txid()), &path),
        block.header.tx_root
    );
}
