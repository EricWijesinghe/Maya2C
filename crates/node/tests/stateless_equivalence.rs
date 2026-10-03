//! Stateless verification against the full node: the same roots, the same
//! refusals, and a different answer only where a witness cannot decide.
//!
//! Every block here is built by a real `StateDB`, which is the authority: its
//! root is the one a block declares, and `verify_block` has to agree with it
//! from a witness alone.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::{
    Account, Address, BlockContext, LayerDigest, StateDB, StateLayer, StateWitness, StatelessError,
    verify_block,
};
use maya_stateless_core::AccountState;
use tempfile::TempDir;

mod common;

const ACTIVATION: u64 = 1;
const FILLER_ACCOUNTS: u8 = 64;

fn context(height: u64) -> BlockContext {
    BlockContext::at_height(height).with_stateless_activation(ACTIVATION)
}

fn key(seed: u8) -> HybridSigningKey {
    signing_key_from_seed(&[seed; 32]).expect("derive")
}

fn filler(index: u8) -> Address {
    *blake3::hash(&[0xF1, index]).as_bytes()
}

fn transfer(sender: &HybridSigningKey, nonce: u64, outputs: &[(Address, u64)]) -> Transaction {
    let outputs = outputs
        .iter()
        .map(|(recipient, amount)| TxOutput {
            amount: *amount,
            recipient: *recipient,
        })
        .collect();
    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.sign(sender, &common::test_chain()).expect("sign");
    tx
}

struct Node {
    _dir: TempDir,
    db: StateDB,
    tip: BlockHeader,
    height: u64,
}

impl Node {
    /// Funds `accounts` plus the fillers that keep the tree deep enough for a
    /// witness to leave something opaque.
    fn new(accounts: &[Address]) -> Self {
        let dir = TempDir::new().expect("tempdir");
        let db = StateDB::open(dir.path()).expect("open");
        common::bind(&db);
        let fillers = (0..FILLER_ACCOUNTS).map(filler);
        for address in accounts.iter().copied().chain(fillers) {
            let account = Account {
                balance: 1_000,
                nonce: 0,
            };
            db.put_account(&address, &account).expect("fund");
        }
        let tip = BlockHeader {
            prev_hash: [0; 32],
            state_root: db.state_root().expect("root"),
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        };
        Self {
            _dir: dir,
            db,
            tip,
            height: 0,
        }
    }

    fn activated(accounts: &[Address]) -> Self {
        let mut node = Self::new(accounts);
        let block = node.build(vec![]);
        node.commit(&block);
        node
    }

    fn draft(&self, transactions: Vec<Transaction>) -> Block {
        let header = BlockHeader {
            prev_hash: self.tip.id(),
            state_root: [0; 32],
            timestamp: self.tip.timestamp + 1,
            nonce: 0,
            difficulty_target: self.tip.difficulty_target,
            tx_root: [0; 32],
        };
        Block::new(header, transactions)
    }

    /// A block on the tip declaring the root the full node computes for it.
    fn build(&self, transactions: Vec<Transaction>) -> Block {
        let draft = self.draft(transactions);
        let root = self
            .db
            .preview_root(&draft, context(self.height + 1))
            .expect("preview");
        let header = BlockHeader {
            state_root: root,
            ..draft.header
        };
        Block::new(header, draft.transactions)
    }

    fn commit(&mut self, block: &Block) {
        self.db
            .apply_block_journaled(block, &block.header.id(), context(self.height + 1))
            .expect("apply");
        self.tip = block.header.clone();
        self.height += 1;
    }

    fn witness(&self, block: &Block) -> StateWitness {
        let witness = self.db.block_witness(block).expect("witness");
        let decoded = StateWitness::decode(&witness.encode()).expect("decode");
        assert_eq!(decoded, witness);
        decoded
    }
}

fn unverifiable(result: Result<(), StatelessError>) -> String {
    match result {
        Err(StatelessError::Unverifiable(reason)) => reason,
        other => panic!("expected unverifiable, got {other:?}"),
    }
}

fn invalid(result: Result<(), StatelessError>) -> String {
    match result {
        Err(StatelessError::Invalid(reason)) => reason,
        other => panic!("expected invalid, got {other:?}"),
    }
}

#[test]
fn the_account_encoding_is_the_stateless_leaf_value() {
    let account = Account {
        balance: 5,
        nonce: 9,
    };
    assert_eq!(
        account.encode(),
        AccountState {
            balance: 5,
            nonce: 9
        }
        .encode()
    );
}

#[test]
fn nothing_changes_before_activation_and_the_activation_block_switches_the_root() {
    let alice = key(1).address();
    let mut node = Node::new(&[alice]);
    let dense = node.db.state_root().expect("root");
    assert!(node.db.account_proof(&alice).expect("proof").is_some());

    let empty = node.draft(vec![]);
    let dormant = node
        .db
        .preview_root(&empty, BlockContext::at_height(1))
        .expect("preview");
    assert_eq!(dormant, dense, "an inactive context leaves the root alone");
    assert!(
        node.db.block_witness(&empty).is_err(),
        "no sparse root to open yet"
    );

    let activation = node.build(vec![]);
    assert_ne!(activation.header.state_root, dense);
    node.commit(&activation);
    assert!(node.db.account_proof(&alice).is_err());
    assert!(node.db.block_witness(&node.draft(vec![])).is_ok());
}

#[test]
fn reverting_the_activation_block_restores_the_dense_root() {
    let alice = key(1).address();
    let mut node = Node::new(&[alice]);
    let dense = node.db.state_root().expect("root");
    let activation = node.build(vec![]);
    node.commit(&activation);

    node.db
        .revert_block(&activation.header.id())
        .expect("revert");
    assert_eq!(node.db.state_root().expect("root"), dense);
    assert!(node.db.account_proof(&alice).expect("proof").is_some());
}

#[test]
fn stateless_verification_agrees_with_the_full_node_on_transfer_blocks() {
    let (alice, bob) = (key(1), key(2));
    let mut node = Node::activated(&[alice.address(), bob.address()]);
    let (fresh, other_fresh) = (filler(200), filler(201));

    let blocks = [
        vec![
            transfer(&alice, 0, &[(bob.address(), 10)]),
            transfer(&alice, 1, &[(fresh, 5), (bob.address(), 1)]),
            transfer(&bob, 0, &[(bob.address(), 7)]),
            transfer(&alice, 2, &[]),
        ],
        vec![
            transfer(&bob, 1, &[(alice.address(), 3), (other_fresh, 2)]),
            transfer(&alice, 3, &[(fresh, 1), (filler(3), 4)]),
        ],
    ];
    for transactions in blocks {
        let parent = node.db.state_root().expect("root");
        let block = node.build(transactions);
        let witness = node.witness(&block);
        assert_eq!(
            verify_block(
                &parent,
                node.height + 1,
                &block,
                witness,
                &common::test_chain()
            ),
            Ok(())
        );
        node.commit(&block);
    }
}

#[test]
fn a_broken_transfer_is_invalid_statelessly_as_it_is_refused_statefully() {
    let (alice, bob) = (key(1), key(2));
    let node = Node::activated(&[alice.address()]);
    let parent = node.db.state_root().expect("root");

    for tx in [
        transfer(&alice, 5, &[(bob.address(), 1)]),
        transfer(&alice, 0, &[(bob.address(), 1_001)]),
    ] {
        let block = node.draft(vec![tx]);
        assert!(node.db.preview_root(&block, context(2)).is_err());
        invalid(verify_block(
            &parent,
            node.height + 1,
            &block,
            node.witness(&block),
            &common::test_chain(),
        ));
    }
}

#[test]
fn a_declared_root_the_transfers_do_not_produce_is_invalid() {
    let (alice, bob) = (key(1), key(2));
    let node = Node::activated(&[alice.address()]);
    let parent = node.db.state_root().expect("root");
    let honest = node.build(vec![transfer(&alice, 0, &[(bob.address(), 1)])]);
    let witness = node.witness(&honest);

    let mut lying = honest.clone();
    lying.header.state_root = parent;
    let reason = invalid(verify_block(
        &parent,
        node.height + 1,
        &lying,
        witness,
        &common::test_chain(),
    ));
    assert!(reason.contains("declares"), "{reason}");
}

#[test]
fn a_substituted_body_is_unverifiable_not_invalid() {
    let (alice, bob) = (key(1), key(2));
    let node = Node::activated(&[alice.address()]);
    let parent = node.db.state_root().expect("root");
    let mut block = node.build(vec![transfer(&alice, 0, &[(bob.address(), 1)])]);
    let witness = node.witness(&block);

    block.transactions = vec![transfer(&alice, 0, &[(bob.address(), 2)])];
    unverifiable(verify_block(
        &parent,
        node.height + 1,
        &block,
        witness,
        &common::test_chain(),
    ));
}

#[test]
fn a_stale_or_foreign_witness_is_unverifiable_not_invalid() {
    let (alice, bob) = (key(1), key(2));
    let mut node = Node::activated(&[alice.address(), bob.address()]);

    let first = node.build(vec![transfer(&bob, 0, &[(alice.address(), 1)])]);
    let stale = node.witness(&node.draft(vec![transfer(&alice, 0, &[(bob.address(), 1)])]));
    node.commit(&first);

    let parent = node.db.state_root().expect("root");
    let next = node.build(vec![transfer(&alice, 0, &[(bob.address(), 1)])]);
    let reason = unverifiable(verify_block(
        &parent,
        node.height + 1,
        &next,
        stale,
        &common::test_chain(),
    ));
    assert!(reason.contains("pre-state root"), "{reason}");

    let elsewhere = node.build(vec![transfer(&alice, 0, &[(filler(250), 1)])]);
    let foreign = node.witness(&next);
    let reason = unverifiable(verify_block(
        &parent,
        node.height + 1,
        &elsewhere,
        foreign,
        &common::test_chain(),
    ));
    assert!(reason.contains("does not open"), "{reason}");
}

#[test]
fn a_pass_layer_or_a_dense_parent_makes_a_block_unverifiable() {
    let (alice, bob) = (key(1), key(2));
    let node = Node::activated(&[alice.address()]);
    let parent = node.db.state_root().expect("root");
    let block = node.build(vec![transfer(&alice, 0, &[(bob.address(), 1)])]);

    let mut governed = node.witness(&block);
    governed.layers.push(LayerDigest {
        layer: StateLayer::Governance,
        root: [7; 32],
    });
    let reason = unverifiable(verify_block(
        &parent,
        node.height + 1,
        &block,
        governed,
        &common::test_chain(),
    ));
    assert!(reason.contains("governance"), "{reason}");

    let mut dense = node.witness(&block);
    dense
        .layers
        .retain(|digest| digest.layer != StateLayer::Stateless);
    unverifiable(verify_block(
        &parent,
        node.height + 1,
        &block,
        dense,
        &common::test_chain(),
    ));
}

#[test]
fn a_transaction_witness_decides_a_block_of_that_transaction() {
    let (alice, bob) = (key(1), key(2));
    let node = Node::activated(&[alice.address()]);
    let parent = node.db.state_root().expect("root");
    let tx = transfer(&alice, 0, &[(bob.address(), 9)]);
    let witness = node.db.transaction_witness(&tx).expect("witness");
    assert!(
        witness.encode().len() < 1_024,
        "{} bytes",
        witness.encode().len()
    );

    let block = node.build(vec![tx]);
    assert_eq!(
        verify_block(
            &parent,
            node.height + 1,
            &block,
            witness,
            &common::test_chain()
        ),
        Ok(())
    );
}

#[test]
fn witness_layers_out_of_fold_order_are_refused() {
    let alice = key(1);
    let node = Node::activated(&[alice.address()]);
    let mut witness = node.witness(&node.draft(vec![]));
    witness.layers.push(LayerDigest {
        layer: StateLayer::Channels,
        root: [1; 32],
    });
    assert!(StateWitness::decode(&witness.encode()).is_err());
}
