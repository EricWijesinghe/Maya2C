//! An upgrade entry with a feature bit this binary does not implement, end to
//! end through `Chain::insert_block` (ADR-082 amendment, invariant 34).
//!
//! The entry is passed by the real governance path — proposal, two votes,
//! timelock, execution — on a state database, and the entry that execution
//! stored is read back. Governance requires a lead of `MIN_UPGRADE_LEAD_BLOCKS`
//! (2,592,000 blocks) before it may activate, which no test can build through
//! `insert_block`. So the chain receives the *same* entry (version and feature
//! bits as governance stored them) with its activation moved to a height the
//! test can reach, written into committed state before the chain opens. What
//! is under test is the chain's import path, not the lead rule, which
//! `upgrade_schedule_tests.rs` pins on its own.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::governance_payload::{
    Ballot, ProposalSubmission, ScheduleUpgrade, StakeLock,
};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::genesis::GenesisConfig;
use custom_l1_node::governance::derive_proposal_id;
use custom_l1_node::state::upgrades::version_key;
use custom_l1_node::state::{Account, BlockContext, StateDB};
use custom_l1_node::upgrade::{FeatureSet, ProtocolUpgrade, SUPPORTED_PROTOCOL_VERSION};
use maya_governance::limits::{
    EXECUTION_GRACE_BLOCKS, MIN_TIMELOCK_BLOCKS, MIN_UPGRADE_LEAD_BLOCKS, MIN_VOTING_BLOCKS,
};
use maya_governance::tally::Choice;
use primitive_types::U256;
use tempfile::TempDir;

mod common;

/// A bit no binary defines: the top one, clear of bits claimed in flight.
const UNKNOWN_BIT: u64 = 1 << 63;
/// The height the chain's copy of the entry activates at.
const H: u64 = 6;

const PROPOSED_AT: u64 = 3;
const EXECUTABLE: u64 = PROPOSED_AT + MIN_VOTING_BLOCKS + MIN_TIMELOCK_BLOCKS;
const FIRST_LEGAL_ACTIVATION: u64 = EXECUTABLE + EXECUTION_GRACE_BLOCKS + MIN_UPGRADE_LEAD_BLOCKS;

fn bare_block(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(4),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn act(db: &StateDB, key: &HybridSigningKey, kind: TxKind, height: u64) -> Result<(), NodeError> {
    let nonce = db.get_account(&key.address()).expect("account").nonce;
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key, &common::test_chain()).expect("sign");
    db.apply_block(&bare_block(vec![tx]), BlockContext::at_height(height))
        .map(|_| ())
}

/// Passes a schedule entry carrying `UNKNOWN_BIT` through governance and
/// returns the entry execution stored.
fn governance_written_entry() -> ProtocolUpgrade {
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    common::bind(&db);
    let holders: Vec<HybridSigningKey> = (0..2).map(|_| generate_signing_key().unwrap()).collect();
    for holder in &holders {
        db.put_account(
            &holder.address(),
            &Account {
                balance: U256::from(10_000_000u64) + U256::exp10(27),
                nonce: 0,
            },
        )
        .unwrap();
    }
    for (i, holder) in holders.iter().enumerate() {
        let lock = StakeLock {
            amount: U256::from(4_000_000u64),
            unlock_height: 200_000,
        };
        act(&db, holder, TxKind::LockStake(lock), i as u64 + 1).expect("lock");
    }

    let proposer = &holders[0];
    let nonce = db.get_account(&proposer.address()).unwrap().nonce;
    let id = derive_proposal_id(&proposer.address(), nonce);
    let submission = ProposalSubmission {
        voting_blocks: MIN_VOTING_BLOCKS,
        timelock_blocks: MIN_TIMELOCK_BLOCKS,
        changes: Vec::new(),
        upgrade: Some(ScheduleUpgrade {
            version: SUPPORTED_PROTOCOL_VERSION,
            activation_height: FIRST_LEGAL_ACTIVATION,
            features: FeatureSet(FeatureSet::DVP | UNKNOWN_BIT),
            sunsets: Vec::new(),
        }),
    };
    act(
        &db,
        proposer,
        TxKind::Propose(Box::new(submission)),
        PROPOSED_AT,
    )
    .expect("step 1: the block carrying an unknown-bit proposal is admitted");
    for holder in &holders {
        let ballot = Ballot {
            proposal: id,
            choice: Choice::For.tag(),
        };
        act(&db, holder, TxKind::CastVote(ballot), 10).expect("vote");
    }
    db.apply_block(&bare_block(Vec::new()), BlockContext::at_height(EXECUTABLE))
        .expect("execute");

    let entries = db.effective_schedule().unwrap().upgrades.entries().to_vec();
    assert_eq!(
        entries.len(),
        1,
        "the passed entry was written, not expired"
    );
    assert!(entries[0].features.contains(UNKNOWN_BIT));
    entries[0]
}

fn genesis_config() -> GenesisConfig {
    GenesisConfig {
        chain_id: "unknown-bit-halt".to_string(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: Vec::new(),
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: Vec::new(),
        suite_deprecations: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: None,
    }
}

fn reopen(dir: &TempDir) -> Chain {
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    let genesis = genesis_config().genesis_block().unwrap();
    Chain::open(state, genesis, ChainConfig::without_pow_verification()).unwrap()
}

/// A fresh chain whose committed schedule holds `entry`.
fn chain_with(dir: &TempDir, entry: &ProtocolUpgrade) -> Chain {
    {
        let state = StateDB::open(dir.path()).unwrap();
        genesis_config().seed_state(&state).unwrap();
        state
            .raw_put_for_test(&version_key(entry.version), &common::version_record(entry))
            .unwrap();
    }
    reopen(dir)
}

fn next_block(chain: &Chain) -> Block {
    let h = chain.height() + 1;
    chain
        .candidate_block(1_000_000 + h * 15, Vec::new())
        .unwrap()
}

fn assert_unknown_bit_halt(err: &NodeError) {
    assert!(
        matches!(err, NodeError::UpgradeRequired { version, height, .. }
            if *version == SUPPORTED_PROTOCOL_VERSION && *height == H),
        "{err}"
    );
    assert!(
        err.to_string().contains(&format!("{UNKNOWN_BIT:#x}")),
        "the halt names the unknown bit: {err}"
    );
}

#[test]
fn a_governance_passed_unknown_bit_halts_insert_block_at_its_height_and_after_a_restart() {
    let passed = governance_written_entry();
    let entry = ProtocolUpgrade {
        activation_height: H,
        ..passed
    };
    let dir = TempDir::new().unwrap();
    let mut chain = chain_with(&dir, &entry);
    assert_eq!(
        chain
            .state()
            .effective_schedule()
            .unwrap()
            .upgrades
            .entries(),
        &[entry],
        "the chain holds governance's entry, activation moved to H"
    );

    // Step 2: every block below H is accepted.
    while chain.height() < H - 1 {
        let block = next_block(&chain);
        chain
            .insert_block(block)
            .expect("a block below H is ours to judge");
    }
    assert_eq!(chain.height(), H - 1);

    // Step 3: block H is refused, and the refusal writes nothing.
    let tip = chain.tip();
    let root = chain.state().state_root().unwrap();
    let headers = chain.state().stored_headers().unwrap().len();
    let block_h = next_block(&chain);
    let id_h = block_h.header.id();
    let halt = chain.insert_block(block_h.clone()).unwrap_err();
    assert_unknown_bit_halt(&halt);
    assert_eq!(chain.height(), H - 1);
    assert_eq!(chain.tip(), tip, "the tip did not move");
    assert_eq!(chain.state().state_root().unwrap(), root, "state untouched");
    assert_eq!(chain.get(&tip).unwrap().header.state_root, root);
    assert_eq!(
        chain.state().stored_headers().unwrap().len(),
        headers,
        "no header stored for the refused block"
    );
    assert!(!chain.state().has_body(&id_h).unwrap(), "no body stored");
    assert!(
        !chain.state().has_undo(&id_h).unwrap(),
        "no journal written"
    );
    assert!(
        chain.state().has_undo(&tip).unwrap(),
        "the tip's journal stays"
    );
    drop(chain);

    // Step 4: the halt is read from committed state, so it survives a restart.
    let mut restarted = reopen(&dir);
    assert_eq!(restarted.height(), H - 1, "resumes at the last good block");
    assert_eq!(restarted.tip(), tip);
    assert_eq!(restarted.state().state_root().unwrap(), root);
    let halt = restarted.insert_block(block_h).unwrap_err();
    assert_unknown_bit_halt(&halt);
    assert_eq!(restarted.height(), H - 1);
    assert!(!restarted.state().has_undo(&id_h).unwrap());
}
