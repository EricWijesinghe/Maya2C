//! Block bytes and transaction count move by vote, inside compiled ceilings
//! (ADR-155 §3, B1 and NEW-4), and the bounds that contradicted them agree
//! (the epoch sync bound, the contract-code decoder).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::consensus::bft::certsync;
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::payload::{ContractDeploy, TxKind};
use custom_l1_node::core::transaction::Transaction;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::governance::ParameterTable;
use custom_l1_node::state::{BlockContext, StateDB};
use maya_dex::Amount;
use maya_fee_market::limits::{BLOCK_BYTES_CEILING, BLOCK_TXS_CEILING, MAX_BLOCK_BYTES};
use maya_governance::params::ParameterKey;
use tempfile::TempDir;

mod common;

fn header() -> BlockHeader {
    BlockHeader {
        version: 0,
        prev_hash: [0; 32],
        state_root: [0; 32],
        timestamp: 1_790_000_001,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    }
}

#[test]
fn block_bytes_and_transaction_count_are_governed_under_compiled_ceilings() {
    let bytes = ParameterKey::from_tag(21).unwrap();
    let count = ParameterKey::from_tag(22).unwrap();
    assert_eq!(bytes, ParameterKey::BlockMaxBytes);
    assert_eq!(count, ParameterKey::BlockMaxTransactions);
    // Today's cap is the starting value; the ceiling is the most a vote can set.
    assert_eq!(bytes.bounds().default, Amount::from_u64(MAX_BLOCK_BYTES));
    assert_eq!(bytes.bounds().max, Amount::from_u64(BLOCK_BYTES_CEILING));
    assert_eq!(count.bounds().max, Amount::from_u64(BLOCK_TXS_CEILING));
    assert!(
        BLOCK_BYTES_CEILING > MAX_BLOCK_BYTES,
        "room above today's cap"
    );
    assert!(BLOCK_TXS_CEILING > 65_536, "room above the old decoder cap");
    // Both decide which blocks are valid, so both wait for the validity lead.
    assert!(bytes.is_validity_rule() && count.is_validity_rule());
    // The governance crate restates the ceilings (it has no dependencies);
    // the two must agree.
    assert_eq!(
        maya_governance::params::BLOCK_BYTES_CEILING,
        BLOCK_BYTES_CEILING
    );
}

#[test]
fn a_block_of_more_than_65536_transactions_decodes() {
    let txs: Vec<Transaction> = (0..65_537u64)
        .map(|n| Transaction::new(vec![], vec![], n))
        .collect();
    // The decoder's count bound follows the byte ceiling, not a fixed 65,536;
    // whether a block this large is valid is the byte and count caps' call.
    let bytes = Block::new(header(), txs).to_bytes();
    let back = Block::from_bytes(&bytes).unwrap();
    assert_eq!(back.transactions.len(), 65_537);
}

#[test]
fn a_block_over_the_governed_transaction_count_is_refused_where_it_is_staged() {
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    common::bind(&db);
    // A table that lowers the count to two, as a passed vote would.
    let table = ParameterTable::new().with(ParameterKey::BlockMaxTransactions, Amount::from_u64(2));
    db.raw_put_for_test(b"g:params", &table.encode()).unwrap();
    let txs: Vec<Transaction> = (0..3u64)
        .map(|n| Transaction::new(vec![], vec![], n))
        .collect();
    let refused = db.apply_block(&Block::new(header(), txs), BlockContext::at_height(1));
    assert!(
        matches!(refused, Err(NodeError::BlockTooLarge { .. })),
        "{refused:?}"
    );
}

#[test]
fn a_block_over_the_governed_byte_cap_is_refused_where_it_is_staged() {
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    common::bind(&db);
    let floor = ParameterKey::BlockMaxBytes.bounds().min;
    let table = ParameterTable::new().with(ParameterKey::BlockMaxBytes, floor);
    db.raw_put_for_test(b"g:params", &table.encode()).unwrap();
    // Unsigned transactions are enough: the size check runs before any other.
    // Four deployments of 1,070,000 bytes: over the lowest cap a vote can set,
    // under the starting 5 MiB.
    let filler = vec![0u8; 1_070_000];
    let txs: Vec<Transaction> = (0..4u64)
        .map(|n| {
            Transaction::with_kind(
                TxKind::DeployContract(ContractDeploy {
                    code: filler.clone(),
                }),
                n,
            )
        })
        .collect();
    let block = Block::new(header(), txs);
    assert!((block.to_bytes().len() as u64) < MAX_BLOCK_BYTES);
    assert!(Amount::from_u64(block.to_bytes().len() as u64) > floor);
    let refused = db.apply_block(&block, BlockContext::at_height(1));
    assert!(
        matches!(refused, Err(NodeError::BlockTooLarge { .. })),
        "{refused:?}"
    );
}

#[test]
fn a_contract_of_the_governed_module_ceiling_decodes() {
    let ceiling = ParameterKey::VmMaxModuleBytes
        .bounds()
        .max
        .saturating_to_u64();
    let code = vec![0u8; usize::try_from(ceiling).unwrap()];
    let tx = Transaction::with_kind(TxKind::DeployContract(ContractDeploy { code }), 0);
    let back = Transaction::from_bytes(&tx.to_bytes()).unwrap();
    assert_eq!(
        back, tx,
        "a module the vote range allows is one every node can read"
    );
}

#[test]
fn the_epoch_sync_bound_holds_a_full_mainnet_epoch_at_the_ceiling() {
    // 3,600 blocks a mainnet epoch (`staking.epoch_blocks`).
    let epoch_blocks = 3_600;
    assert!(certsync::max_epoch_bytes(epoch_blocks) >= epoch_blocks * BLOCK_BYTES_CEILING);
    // The old fixed bound was 4 GiB: 3,600 blocks at today's 5 MiB alone are 17.58 GiB.
    assert!(certsync::max_epoch_bytes(epoch_blocks) > 4 << 30);
}

#[test]
fn the_lowest_byte_cap_a_vote_can_set_still_holds_the_largest_transaction() {
    use custom_l1_node::core::block::block_wire_len;
    use custom_l1_node::core::transaction::MAX_TX_BYTES;
    let floor = ParameterKey::BlockMaxBytes.bounds().min.saturating_to_u64();
    assert!(block_wire_len([MAX_TX_BYTES]) <= floor);
    // And the contract decoder reads the top of the module range, no more.
    assert_eq!(
        custom_l1_node::core::payload::MAX_CONTRACT_CODE as u64,
        ParameterKey::VmMaxModuleBytes
            .bounds()
            .max
            .saturating_to_u64()
    );
}
