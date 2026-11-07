//! The on-chain upgrade schedule (ADR-082): governance's `ScheduleUpgrade`, the
//! compiled lead it must clear, the ordering every entry must respect, and the
//! committed halt that follows from an entry.
//!
//! Heights here are the numbers a block would carry. Governance is driven at the
//! state level with arbitrary heights, as `governance_tests.rs` does, because a
//! scheduled upgrade's lead is 2.6 million blocks: no chain in a test reaches it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::governance_payload::{ProposalSubmission, ScheduleUpgrade};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::crypto::suites::{SuiteSunsets, verification_policy};
use custom_l1_node::error::NodeError;
use custom_l1_node::governance::derive_proposal_id;
use custom_l1_node::state::upgrades::{ScheduleRequest, SunsetSpec};
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use custom_l1_node::upgrade::{FeatureSet, SUPPORTED_PROTOCOL_VERSION};
use maya_crypto_pq::agility::MIN_MIGRATION_WINDOW;
use maya_crypto_pq::suite::SuiteId;
use maya_governance::limits::{MIN_TIMELOCK_BLOCKS, MIN_UPGRADE_LEAD_BLOCKS, MIN_VOTING_BLOCKS};
use maya_governance::tally::Choice;
use tempfile::TempDir;

mod common;

const HOLDERS: usize = 2;
const BALANCE: u64 = 10_000_000;
/// Blocks a proposal stays open before its timelock: voting plus timelock.
const PROPOSED_AT: u64 = 3;
const EXECUTABLE: u64 = PROPOSED_AT + MIN_VOTING_BLOCKS + MIN_TIMELOCK_BLOCKS;
/// The latest height a proposal opened at `PROPOSED_AT` can execute at.
const EXPIRES_AFTER: u64 = EXECUTABLE + maya_governance::limits::EXECUTION_GRACE_BLOCKS;

struct Fixture {
    db: StateDB,
    holders: Vec<HybridSigningKey>,
    _dir: TempDir,
}

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key, &common::test_chain()).expect("sign");
    tx
}

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

fn fixture() -> Fixture {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    common::bind(&db);
    let holders: Vec<HybridSigningKey> = (0..HOLDERS)
        .map(|_| generate_signing_key().expect("key"))
        .collect();
    for holder in &holders {
        db.put_account(
            &holder.address(),
            &Account {
                balance: primitive_types::U256::from(BALANCE) + primitive_types::U256::exp10(27),
                nonce: 0,
            },
        )
        .expect("fund");
    }
    Fixture {
        db,
        holders,
        _dir: dir,
    }
}

impl Fixture {
    fn address(&self, index: usize) -> Address {
        self.holders[index].address()
    }

    fn nonce_of(&self, index: usize) -> u64 {
        self.db
            .get_account(&self.address(index))
            .expect("account")
            .nonce
    }

    fn apply(&self, transactions: Vec<Transaction>, height: u64) -> Result<[u8; 32], NodeError> {
        self.db
            .apply_block(&block_of(transactions), BlockContext::at_height(height))
    }

    fn act(&self, index: usize, kind: TxKind, height: u64) -> Result<[u8; 32], NodeError> {
        let nonce = self.nonce_of(index);
        self.apply(vec![signed(kind, nonce, &self.holders[index])], height)
    }

    fn lock(&self, index: usize, amount: u64, unlock_height: u64, height: u64) {
        self.act(
            index,
            TxKind::LockStake(custom_l1_node::core::governance_payload::StakeLock {
                amount: primitive_types::U256::from(amount),
                unlock_height,
            }),
            height,
        )
        .expect("lock");
    }

    /// Opens a proposal from holder 0 carrying `upgrade` at `PROPOSED_AT`.
    fn propose(&self, upgrade: ScheduleUpgrade) -> Result<[u8; 32], NodeError> {
        let id = derive_proposal_id(&self.address(0), self.nonce_of(0));
        self.act(
            0,
            TxKind::Propose(Box::new(ProposalSubmission {
                voting_blocks: MIN_VOTING_BLOCKS,
                timelock_blocks: MIN_TIMELOCK_BLOCKS,
                changes: Vec::new(),
                upgrade: Some(upgrade),
            })),
            PROPOSED_AT,
        )?;
        Ok(id)
    }

    /// Two holders lock past execution and vote for `id`, then the block at
    /// the executable height settles it.
    fn pass(&self, id: [u8; 32]) {
        self.lock(0, 4_000_000, 200_000, 1);
        self.lock(1, 4_000_000, 200_000, 2);
        for index in 0..HOLDERS {
            self.act(
                index,
                TxKind::CastVote(custom_l1_node::core::governance_payload::Ballot {
                    proposal: id,
                    choice: Choice::For.tag(),
                }),
                10,
            )
            .expect("vote");
        }
        self.apply(Vec::new(), EXECUTABLE).expect("execute");
    }
}

fn upgrade(version: u32, activation_height: u64, features: u64) -> ScheduleUpgrade {
    ScheduleUpgrade {
        version,
        activation_height,
        features: FeatureSet(features),
        sunsets: Vec::new(),
    }
}

/// The earliest activation a proposal opened at `PROPOSED_AT` may name.
const FIRST_LEGAL_ACTIVATION: u64 = EXPIRES_AFTER + MIN_UPGRADE_LEAD_BLOCKS;

#[test]
fn a_passed_schedule_upgrade_is_written_and_activates_its_feature_at_its_height() {
    let f = fixture();
    let id = f
        .propose(upgrade(
            SUPPORTED_PROTOCOL_VERSION,
            FIRST_LEGAL_ACTIVATION,
            FeatureSet::DVP,
        ))
        .expect("a lead of exactly the minimum is accepted");
    f.pass(id);

    let schedule = f.db.effective_schedule().expect("schedule");
    assert_eq!(schedule.upgrades.entries().len(), 1);
    assert_eq!(schedule.features().dvp, FIRST_LEGAL_ACTIVATION);
    assert_eq!(schedule.features().iot, u64::MAX, "other features stay off");
    assert!(schedule.check_halt(FIRST_LEGAL_ACTIVATION).is_ok());
}

#[test]
fn a_schedule_upgrade_one_block_short_of_the_lead_is_refused_at_proposal() {
    let f = fixture();
    let short = upgrade(
        SUPPORTED_PROTOCOL_VERSION,
        FIRST_LEGAL_ACTIVATION - 1,
        FeatureSet::DVP,
    );
    assert!(f.propose(short).is_err(), "one block under the lead");
    assert!(
        f.db.effective_schedule()
            .unwrap()
            .upgrades
            .entries()
            .is_empty(),
        "a refused proposal writes nothing"
    );
}

#[test]
fn a_version_this_binary_lacks_halts_at_the_height_the_committed_entry_names() {
    let f = fixture();
    let scheduled = SUPPORTED_PROTOCOL_VERSION + 1;
    let id = f
        .propose(upgrade(scheduled, FIRST_LEGAL_ACTIVATION, 0))
        .expect("propose");
    f.pass(id);

    let schedule = f.db.effective_schedule().expect("schedule");
    schedule
        .check_halt(FIRST_LEGAL_ACTIVATION - 1)
        .expect("the block before the activation is ours to judge");
    let halt = schedule
        .check_halt(FIRST_LEGAL_ACTIVATION)
        .expect_err("the activation height is refused");
    assert!(
        matches!(halt, NodeError::UpgradeRequired { version, height, supported }
            if version == scheduled && height == FIRST_LEGAL_ACTIVATION
                && supported == SUPPORTED_PROTOCOL_VERSION),
        "{halt}"
    );
}

#[test]
fn a_new_entry_must_lie_after_every_entry_already_written() {
    let f = fixture();
    let id = f
        .propose(upgrade(
            SUPPORTED_PROTOCOL_VERSION,
            FIRST_LEGAL_ACTIVATION,
            FeatureSet::DVP,
        ))
        .expect("propose");
    f.pass(id);

    let schedule = f.db.effective_schedule().expect("schedule");
    let later_version = ScheduleRequest {
        activation: FIRST_LEGAL_ACTIVATION + 1,
        version: Some(SUPPORTED_PROTOCOL_VERSION + 1),
        features: FeatureSet(FeatureSet::IOT),
        sunsets: Vec::new(),
    };
    // Admitted at the earliest height that clears the lead from `now`.
    let now = FIRST_LEGAL_ACTIVATION + 1 - MIN_UPGRADE_LEAD_BLOCKS;
    schedule
        .admit(
            &later_version,
            now,
            MIN_UPGRADE_LEAD_BLOCKS,
            SuiteId::MlDsa87,
        )
        .expect("a later height and version is admitted");

    let not_later = ScheduleRequest {
        activation: FIRST_LEGAL_ACTIVATION,
        ..later_version.clone()
    };
    assert!(
        schedule
            .admit(&not_later, now, MIN_UPGRADE_LEAD_BLOCKS, SuiteId::MlDsa87)
            .is_err()
    );
    let same_version = ScheduleRequest {
        version: Some(SUPPORTED_PROTOCOL_VERSION),
        ..later_version
    };
    assert!(
        schedule
            .admit(
                &same_version,
                now,
                MIN_UPGRADE_LEAD_BLOCKS,
                SuiteId::MlDsa87
            )
            .is_err()
    );
}

#[test]
fn a_schedule_entry_that_schedules_nothing_or_a_feature_on_a_sunset_is_refused() {
    let schedule = fixture().db.effective_schedule().expect("schedule");
    let nothing = ScheduleRequest {
        activation: FIRST_LEGAL_ACTIVATION,
        version: None,
        features: FeatureSet::default(),
        sunsets: Vec::new(),
    };
    assert!(
        schedule
            .admit(&nothing, 1, MIN_UPGRADE_LEAD_BLOCKS, SuiteId::MlDsa87)
            .is_err()
    );
    let feature_without_version = ScheduleRequest {
        features: FeatureSet(FeatureSet::DVP),
        sunsets: vec![SunsetSpec {
            suite: SuiteId::MlDsa65.to_byte(),
            sunset_height: FIRST_LEGAL_ACTIVATION + MIN_MIGRATION_WINDOW,
            emergency: false,
        }],
        ..nothing
    };
    assert!(
        schedule
            .admit(
                &feature_without_version,
                1,
                MIN_UPGRADE_LEAD_BLOCKS,
                SuiteId::MlDsa87
            )
            .is_err()
    );
}

// ---------------------------------------------------------------------------
// suite sunsets
// ---------------------------------------------------------------------------

#[test]
fn a_committed_sunset_refuses_its_suite_at_its_height_and_accepts_it_one_block_before() {
    let f = fixture();
    let sunset_height = FIRST_LEGAL_ACTIVATION + MIN_MIGRATION_WINDOW;
    let request = ScheduleUpgrade {
        version: SUPPORTED_PROTOCOL_VERSION,
        activation_height: FIRST_LEGAL_ACTIVATION,
        features: FeatureSet::default(),
        sunsets: vec![custom_l1_node::state::upgrades::SunsetSpec {
            suite: SuiteId::MlDsa65.to_byte(),
            sunset_height,
            emergency: false,
        }],
    };
    let id = f.propose(request).expect("propose a sunset");
    f.pass(id);

    let policy = verification_policy(&f.db.effective_schedule().unwrap().sunsets).unwrap();
    let admissible =
        |h| custom_l1_node::crypto::suites::check_admissible(&policy, SuiteId::MlDsa65, h).is_ok();
    assert!(admissible(sunset_height - 1), "accepted one block before");
    assert!(!admissible(sunset_height), "refused at its height");
    assert!(
        custom_l1_node::crypto::suites::check_admissible(&policy, SuiteId::MlDsa87, sunset_height)
            .is_ok(),
        "other suites are untouched"
    );
}

#[test]
fn an_unused_schedule_has_the_genesis_suite_policy_and_no_sunsets() {
    let schedule = fixture().db.effective_schedule().expect("schedule");
    assert_eq!(schedule.sunsets, SuiteSunsets::EMPTY);
    assert_eq!(
        (schedule.last_version_height, schedule.last_sunset_height),
        (0, 0)
    );
}
