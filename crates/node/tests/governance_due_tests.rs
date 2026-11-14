//! The settlement index's per-height cap, through real transactions.
//!
//! The end-of-block pass settles only proposals due at its height (ADR-082
//! amendment 2026-11-13). The cap is what keeps "due at one height" small:
//! without it, 32 proposals a block for 17 days could all come due together.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use custom_l1_node::core::governance_payload::ProposalSubmission;
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::governance::derive_proposal_id;
use custom_l1_node::state::governance_exec::{
    MAX_GOVERNANCE_TX_PER_BLOCK, MAX_PROPOSALS_DUE_PER_HEIGHT,
};
use custom_l1_node::state::{Account, BlockContext, StateDB};
use maya_governance::limits::{MIN_TIMELOCK_BLOCKS, MIN_VOTING_BLOCKS};
use primitive_types::U256;
use tempfile::TempDir;

/// Long enough that every proposal below closes at one height from a
/// different opening height.
const VOTING: u64 = MIN_VOTING_BLOCKS + 10;

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(4),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn open() -> (StateDB, HybridSigningKey, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    common::bind(&db);
    let key = generate_signing_key().expect("key");
    // 10,000 coins at 24 decimals: every deposit below, with room to spare.
    db.put_account(
        &key.address(),
        &Account {
            balance: U256::exp10(28),
            nonce: 0,
        },
    )
    .expect("fund");
    (db, key, dir)
}

/// A proposal that changes nothing, opened at `height` so that its vote
/// closes at `VOTING + 1` whatever the height.
fn proposal(nonce: u64, height: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(
        TxKind::Propose(Box::new(ProposalSubmission {
            voting_blocks: VOTING + 1 - height,
            timelock_blocks: MIN_TIMELOCK_BLOCKS,
            upgrade: None,
            changes: Vec::new(),
        })),
        nonce,
    );
    tx.sign(key, &common::test_chain()).expect("sign");
    tx
}

/// Fills one height's due slots from successive blocks, then shows the next
/// proposal for that height refused and one for the following height taken.
#[test]
fn a_height_takes_at_most_the_capped_number_of_due_proposals() {
    let (db, key, _dir) = open();
    let per_block = MAX_GOVERNANCE_TX_PER_BLOCK as u64;
    let mut nonce = 0;
    let mut height = 1;
    while nonce < MAX_PROPOSALS_DUE_PER_HEIGHT as u64 {
        let count = per_block.min(MAX_PROPOSALS_DUE_PER_HEIGHT as u64 - nonce);
        let txs = (nonce..nonce + count)
            .map(|n| proposal(n, height, &key))
            .collect();
        db.apply_block(&block_of(txs), BlockContext::at_height(height))
            .expect("a proposal while the height has room");
        nonce += count;
        height += 1;
    }

    let refused = db.apply_block(
        &block_of(vec![proposal(nonce, height, &key)]),
        BlockContext::at_height(height),
    );
    assert!(
        matches!(&refused, Err(NodeError::Governance { reason }) if reason.contains("due")),
        "the cap must refuse the next proposal for a full height, got {refused:?}"
    );
    assert!(
        db.proposal(&derive_proposal_id(&key.address(), nonce))
            .expect("read")
            .is_none()
    );

    // One block later closes one height later: a free slot.
    db.apply_block(
        &block_of(vec![proposal(nonce, height - 1, &key)]),
        BlockContext::at_height(height),
    )
    .expect("the next height has room");
}

/// Prints the wire size of a proposal and a cancellation, which is what the
/// fee is charged on. Run with `--ignored --nocapture`.
#[test]
#[ignore = "measurement: prints sizes"]
fn measure_governance_transaction_sizes() {
    let (_db, key, _dir) = open();
    let propose = proposal(0, 1, &key);
    let mut cancel = Transaction::with_kind(
        TxKind::CancelProposal(derive_proposal_id(&key.address(), 0)),
        1,
    );
    cancel.sign(&key, &common::test_chain()).expect("sign");
    println!(
        "governance tx bytes: propose={} cancel={}",
        propose.to_bytes().len(),
        cancel.to_bytes().len()
    );
}
