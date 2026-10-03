//! Governance against real block execution.
//!
//! `crates/governance/src/` proves the state machine and the bounds in isolation.
//! These cover what it deliberately does not know about: stake that has to be
//! locked before it counts, a timelock measured in blocks the chain actually
//! produces, and a rule change that a later block is observed to execute under.
//!
//! Three of these are load-bearing rather than merely nice:
//!
//! - **A hostile majority must hit the compiled floors.** Governance that can
//!   vote away its own quorum or timelock has neither.
//! - **A vote must not be sellable.** Stake locked only to the close of voting
//!   would let a voter be gone before the decision binds anyone who stayed.
//! - **A parameter change must take effect, and only after the timelock.** The
//!   whole point of the subsystem is a rule that a later block reads
//!   differently, and the whole point of the timelock is that it does not
//!   happen sooner.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::governance_payload::{
    Ballot, ProposalSubmission, StakeLock, StakeUnlock, WorkClaim,
};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::governance::derive_proposal_id;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_governance::limits::{MIN_TIMELOCK_BLOCKS, MIN_VOTING_BLOCKS};
use maya_governance::params::ParameterKey;
use maya_governance::proposal::ProposalState;
use maya_governance::tally::Choice;
use tempfile::TempDir;

mod common;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

const HOLDERS: usize = 3;
const BALANCE: u64 = 10_000_000;

struct Fixture {
    db: StateDB,
    holders: Vec<HybridSigningKey>,
    _dir: TempDir,
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            // Four leading zero bits, so a work claim credits a small,
            // predictable number rather than a difficulty-dependent one.
            difficulty_target: target_from_leading_zero_bits(4),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key, &common::test_chain()).expect("sign");
    tx
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
                balance: BALANCE,
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

    /// A block from one holder, at their next nonce.
    fn act(&self, index: usize, kind: TxKind, height: u64) -> Result<[u8; 32], NodeError> {
        let nonce = self.nonce_of(index);
        self.apply(vec![signed(kind, nonce, &self.holders[index])], height)
    }

    /// Locks `amount` for `index`, released at `unlock_height`.
    fn lock(&self, index: usize, amount: u64, unlock_height: u64, height: u64) {
        self.act(
            index,
            TxKind::LockStake(StakeLock {
                amount,
                unlock_height,
            }),
            height,
        )
        .expect("lock");
    }

    fn parameter(&self, key: ParameterKey) -> u64 {
        self.db.parameters().expect("parameters").get(key)
    }

    fn state_of(&self, id: &[u8; 32]) -> ProposalState {
        self.db
            .proposal(id)
            .expect("read")
            .expect("proposal")
            .proposal
            .state
    }

    /// Opens a proposal from holder 0 changing one parameter, and returns its
    /// identifier and the height it becomes executable at.
    fn propose(&self, key: ParameterKey, value: u64, height: u64) -> ([u8; 32], u64) {
        let nonce = self.nonce_of(0);
        let id = derive_proposal_id(&self.address(0), nonce);

        self.act(
            0,
            TxKind::Propose(Box::new(ProposalSubmission {
                voting_blocks: MIN_VOTING_BLOCKS,
                timelock_blocks: MIN_TIMELOCK_BLOCKS,
                changes: vec![(key.tag(), value)],
            })),
            height,
        )
        .expect("propose");

        let executable = self
            .db
            .proposal(&id)
            .expect("read")
            .expect("proposal")
            .proposal
            .schedule
            .executable_from;
        (id, executable)
    }

    fn vote(
        &self,
        index: usize,
        id: [u8; 32],
        choice: Choice,
        height: u64,
    ) -> Result<(), NodeError> {
        self.act(
            index,
            TxKind::CastVote(Ballot {
                proposal: id,
                choice: choice.tag(),
            }),
            height,
        )
        .map(|_| ())
    }
}

// ---------------------------------------------------------------------------
// stake and work
// ---------------------------------------------------------------------------

#[test]
fn locking_stake_moves_it_out_of_the_spendable_balance() {
    // Weight that could still be spent is weight that costs nothing to hold.
    let fixture = fixture();
    fixture.lock(0, 1_000, 5_000, 1);

    let account = fixture
        .db
        .get_account(&fixture.address(0))
        .expect("account");
    assert_eq!(account.balance, BALANCE - 1_000);
    assert_eq!(
        fixture
            .db
            .stake_lock(&fixture.address(0))
            .expect("lock")
            .amount,
        1_000
    );
    assert_eq!(
        fixture
            .db
            .voting_power(&fixture.address(0), 1)
            .expect("power"),
        1_000
    );
}

#[test]
fn stake_cannot_be_withdrawn_before_its_height() {
    let fixture = fixture();
    fixture.lock(0, 1_000, 5_000, 1);

    assert!(matches!(
        fixture
            .act(0, TxKind::UnlockStake(StakeUnlock { amount: 1_000 }), 4_999)
            .unwrap_err(),
        NodeError::StakeLocked { .. }
    ));

    fixture
        .act(0, TxKind::UnlockStake(StakeUnlock { amount: 1_000 }), 5_000)
        .expect("unlock");
    assert_eq!(
        fixture
            .db
            .get_account(&fixture.address(0))
            .expect("account")
            .balance,
        BALANCE
    );
}

#[test]
fn locking_again_extends_the_release_and_never_shortens_it() {
    // Shortening would let a voter reduce their own commitment after voting,
    // which is the vote-then-sell attack with an extra step.
    let fixture = fixture();
    fixture.lock(0, 1_000, 9_000, 1);
    fixture.lock(0, 500, 2_000, 2);

    let lock = fixture.db.stake_lock(&fixture.address(0)).expect("lock");
    assert_eq!(lock.amount, 1_500);
    assert_eq!(lock.unlock_height, 9_000, "the later height survived");
}

#[test]
fn a_work_claim_credits_the_address_the_miner_named() {
    // This chain has no coinbase and no miner field in the header, so before
    // the claim there was no on-chain record of who mined anything.
    let fixture = fixture();
    let beneficiary = fixture.address(1);

    fixture
        .act(0, TxKind::ClaimWork(WorkClaim { beneficiary }), 1)
        .expect("claim");

    assert!(fixture.db.work_credit(&beneficiary, 1).expect("credit") > 0);
    // The sender is not the beneficiary unless it says so.
    assert_eq!(
        fixture
            .db
            .work_credit(&fixture.address(0), 1)
            .expect("credit"),
        0
    );
}

#[test]
fn a_block_may_credit_its_work_only_once() {
    // A block represents one unit of work. A second claim would let a miner
    // mint voting weight out of a single proof of work.
    let fixture = fixture();
    let first = signed(
        TxKind::ClaimWork(WorkClaim {
            beneficiary: fixture.address(1),
        }),
        0,
        &fixture.holders[0],
    );
    let second = signed(
        TxKind::ClaimWork(WorkClaim {
            beneficiary: fixture.address(2),
        }),
        1,
        &fixture.holders[0],
    );

    assert!(matches!(
        fixture.apply(vec![first, second], 1).unwrap_err(),
        NodeError::DuplicateWorkClaim
    ));
}

#[test]
fn work_credit_decays_so_a_departed_miner_stops_governing() {
    use custom_l1_node::governance::WORK_HALF_LIFE_BLOCKS;

    let fixture = fixture();
    let beneficiary = fixture.address(1);
    fixture
        .act(0, TxKind::ClaimWork(WorkClaim { beneficiary }), 1)
        .expect("claim");

    let fresh = fixture.db.work_credit(&beneficiary, 1).expect("credit");
    let aged = fixture
        .db
        .work_credit(&beneficiary, 1 + WORK_HALF_LIFE_BLOCKS)
        .expect("credit");

    assert_eq!(aged, fresh / 2);
}

// ---------------------------------------------------------------------------
// the compiled floors
// ---------------------------------------------------------------------------

#[test]
fn a_proposal_below_the_timelock_floor_is_refused() {
    // The floor is what stops a hostile majority removing the exit window.
    // It is not a parameter, so there is no transaction that reaches it.
    let fixture = fixture();
    let error = fixture
        .act(
            0,
            TxKind::Propose(Box::new(ProposalSubmission {
                voting_blocks: MIN_VOTING_BLOCKS,
                timelock_blocks: MIN_TIMELOCK_BLOCKS - 1,
                changes: vec![(ParameterKey::DexProtocolFeeBps.tag(), 10)],
            })),
            1,
        )
        .unwrap_err();

    assert!(matches!(error, NodeError::Governance { .. }));
}

#[test]
fn no_proposal_can_name_the_quorum_or_the_timelock() {
    // They appear in no `ParameterKey`, so the tags simply do not exist. A
    // proposal naming one is refused at decode of the key rather than at a
    // range check, which is the difference between "not permitted" and "not
    // representable".
    let fixture = fixture();
    for tag in [100u16, 200, u16::MAX] {
        let error = fixture
            .act(
                0,
                TxKind::Propose(Box::new(ProposalSubmission {
                    voting_blocks: MIN_VOTING_BLOCKS,
                    timelock_blocks: MIN_TIMELOCK_BLOCKS,
                    changes: vec![(tag, 1)],
                })),
                1,
            )
            .unwrap_err();
        assert!(matches!(error, NodeError::Governance { .. }));
    }
}

#[test]
fn a_proposal_outside_a_parameters_hard_range_is_refused() {
    // The protocol fee is capped at two percent. A captured governance can
    // reach the cap and no further.
    let fixture = fixture();
    let bounds = ParameterKey::DexProtocolFeeBps.bounds();

    assert!(matches!(
        fixture
            .act(
                0,
                TxKind::Propose(Box::new(ProposalSubmission {
                    voting_blocks: MIN_VOTING_BLOCKS,
                    timelock_blocks: MIN_TIMELOCK_BLOCKS,
                    changes: vec![(ParameterKey::DexProtocolFeeBps.tag(), bounds.max + 1)],
                })),
                1,
            )
            .unwrap_err(),
        NodeError::Governance { .. }
    ));

    // And the ceiling itself is reachable.
    fixture
        .act(
            0,
            TxKind::Propose(Box::new(ProposalSubmission {
                voting_blocks: MIN_VOTING_BLOCKS,
                timelock_blocks: MIN_TIMELOCK_BLOCKS,
                changes: vec![(ParameterKey::DexProtocolFeeBps.tag(), bounds.max)],
            })),
            1,
        )
        .expect("the ceiling is a permitted value");
}

// ---------------------------------------------------------------------------
// a full cycle
// ---------------------------------------------------------------------------

#[test]
fn a_proposal_that_passes_changes_a_rule_the_chain_then_reads_differently() {
    // The whole point of the subsystem, end to end: a value that is one thing
    // before a vote and another after it, with a timelock in between.
    let fixture = fixture();
    assert_eq!(fixture.parameter(ParameterKey::DexProtocolFeeBps), 0);

    // Two holders lock past the execution height, so their stake is committed
    // for longer than the decision takes.
    fixture.lock(0, 4_000_000, 200_000, 1);
    fixture.lock(1, 4_000_000, 200_000, 2);

    let (id, executable) = fixture.propose(ParameterKey::DexProtocolFeeBps, 25, 3);
    assert_eq!(fixture.state_of(&id), ProposalState::Voting);

    fixture.vote(0, id, Choice::For, 10).expect("vote");
    fixture.vote(1, id, Choice::For, 11).expect("vote");

    // Still open while voting runs.
    fixture.apply(vec![], 100).expect("empty block");
    assert_eq!(fixture.state_of(&id), ProposalState::Voting);

    // Voting closes: the proposal queues rather than executing. One block
    // *past* the closing height, because a vote arriving exactly at the close
    // is still accepted — finalizing there would cut off a valid vote.
    fixture
        .apply(vec![], executable - MIN_TIMELOCK_BLOCKS + 1)
        .expect("close");
    assert_eq!(fixture.state_of(&id), ProposalState::Queued);
    assert_eq!(
        fixture.parameter(ParameterKey::DexProtocolFeeBps),
        0,
        "queued is not executed"
    );

    // One block before the timelock elapses, still nothing.
    fixture.apply(vec![], executable - 1).expect("still queued");
    assert_eq!(fixture.parameter(ParameterKey::DexProtocolFeeBps), 0);

    // And then it lands.
    fixture.apply(vec![], executable).expect("execute");
    assert_eq!(fixture.state_of(&id), ProposalState::Executed);
    assert_eq!(fixture.parameter(ParameterKey::DexProtocolFeeBps), 25);
}

#[test]
fn a_proposal_that_misses_quorum_is_rejected_and_changes_nothing() {
    let fixture = fixture();
    // A single small lock against a large denominator.
    fixture.lock(0, 100, 200_000, 1);
    fixture.lock(1, 9_000_000, 200_000, 2);

    let (id, executable) = fixture.propose(ParameterKey::DexProtocolFeeBps, 25, 3);
    fixture.vote(0, id, Choice::For, 10).expect("vote");

    fixture.apply(vec![], executable).expect("settle");
    assert_eq!(fixture.state_of(&id), ProposalState::Rejected);
    assert_eq!(fixture.parameter(ParameterKey::DexProtocolFeeBps), 0);
}

#[test]
fn a_proposal_voted_down_is_rejected_and_changes_nothing() {
    let fixture = fixture();
    fixture.lock(0, 3_000_000, 200_000, 1);
    fixture.lock(1, 4_000_000, 200_000, 2);

    let (id, executable) = fixture.propose(ParameterKey::DexProtocolFeeBps, 25, 3);
    fixture.vote(0, id, Choice::For, 10).expect("vote");
    fixture.vote(1, id, Choice::Against, 11).expect("vote");

    fixture.apply(vec![], executable).expect("settle");
    assert_eq!(fixture.state_of(&id), ProposalState::Rejected);
    assert_eq!(fixture.parameter(ParameterKey::DexProtocolFeeBps), 0);
}

#[test]
fn the_proposers_deposit_returns_when_the_proposal_settles() {
    // Anti-spam, not a barrier. Charging only for rejection would make
    // proposing an unpopular change expensive, which is backwards.
    let fixture = fixture();
    fixture.lock(0, 3_000_000, 200_000, 1);

    let deposit = fixture.parameter(ParameterKey::GovernanceProposalDeposit);
    let before = fixture
        .db
        .get_account(&fixture.address(0))
        .expect("a")
        .balance;

    let (id, executable) = fixture.propose(ParameterKey::DexProtocolFeeBps, 25, 2);
    assert_eq!(
        fixture
            .db
            .get_account(&fixture.address(0))
            .expect("a")
            .balance,
        before - deposit
    );

    fixture.apply(vec![], executable).expect("settle");
    assert_eq!(fixture.state_of(&id), ProposalState::Rejected);
    assert_eq!(
        fixture
            .db
            .get_account(&fixture.address(0))
            .expect("a")
            .balance,
        before,
        "the deposit came back"
    );
}

// ---------------------------------------------------------------------------
// voting rules
// ---------------------------------------------------------------------------

#[test]
fn a_voter_whose_lock_expires_before_execution_is_refused() {
    // The vote-then-sell guard. Without it the cheapest way to decide
    // something is to acquire weight, vote, and be gone before the decision
    // binds anyone who stayed.
    let fixture = fixture();
    let (id, executable) = fixture.propose(ParameterKey::DexProtocolFeeBps, 25, 1);

    // Locked to one block short of the execution height.
    fixture.lock(1, 1_000_000, executable - 1, 2);

    assert!(matches!(
        fixture.vote(1, id, Choice::For, 3).unwrap_err(),
        NodeError::Governance { .. }
    ));

    // Locked past it, the same vote is accepted.
    fixture.lock(1, 1, executable, 4);
    fixture.vote(1, id, Choice::For, 5).expect("vote");
}

#[test]
fn an_address_may_vote_only_once_on_a_proposal() {
    let fixture = fixture();
    fixture.lock(0, 1_000_000, 200_000, 1);
    let (id, _) = fixture.propose(ParameterKey::DexProtocolFeeBps, 25, 2);

    fixture.vote(0, id, Choice::For, 3).expect("vote");
    assert!(matches!(
        fixture.vote(0, id, Choice::Against, 4).unwrap_err(),
        NodeError::Governance { .. }
    ));
}

#[test]
fn a_voter_with_no_weight_is_refused_rather_than_counted_as_zero() {
    // Counted as zero, a chain of empty votes would pad turnout toward a
    // quorum at no cost.
    let fixture = fixture();
    fixture.lock(0, 1_000_000, 200_000, 1);
    let (id, _) = fixture.propose(ParameterKey::DexProtocolFeeBps, 25, 2);

    assert!(matches!(
        fixture.vote(2, id, Choice::For, 3).unwrap_err(),
        NodeError::Governance { .. }
    ));
}

#[test]
fn a_vote_after_the_window_closes_is_refused() {
    let fixture = fixture();
    fixture.lock(0, 1_000_000, 200_000, 1);
    let (id, executable) = fixture.propose(ParameterKey::DexProtocolFeeBps, 25, 2);

    let closes = executable - MIN_TIMELOCK_BLOCKS;
    assert!(matches!(
        fixture.vote(0, id, Choice::For, closes + 1).unwrap_err(),
        NodeError::Governance { .. }
    ));
}

#[test]
fn only_the_proposer_may_withdraw_a_proposal() {
    let fixture = fixture();
    let (id, _) = fixture.propose(ParameterKey::DexProtocolFeeBps, 25, 1);

    assert!(matches!(
        fixture.act(1, TxKind::CancelProposal(id), 2).unwrap_err(),
        NodeError::NotProposer { .. }
    ));

    fixture
        .act(0, TxKind::CancelProposal(id), 3)
        .expect("cancel");
    assert_eq!(fixture.state_of(&id), ProposalState::Cancelled);
}

// ---------------------------------------------------------------------------
// the rule change actually binds
// ---------------------------------------------------------------------------

#[test]
fn a_governed_ceiling_is_read_from_state_rather_than_from_the_binary() {
    // The mechanism, checked on a second parameter so the test is about the
    // table rather than about one call site.
    let fixture = fixture();
    fixture.lock(0, 4_000_000, 200_000, 1);
    fixture.lock(1, 4_000_000, 200_000, 2);

    let default = fixture.parameter(ParameterKey::DexMaxOrdersPerBook);
    assert_eq!(default, ParameterKey::DexMaxOrdersPerBook.bounds().default);

    let (id, executable) = fixture.propose(ParameterKey::DexMaxOrdersPerBook, 512, 3);
    fixture.vote(0, id, Choice::For, 10).expect("vote");
    fixture.vote(1, id, Choice::For, 11).expect("vote");
    fixture.apply(vec![], executable).expect("execute");

    assert_eq!(fixture.state_of(&id), ProposalState::Executed);
    assert_eq!(fixture.parameter(ParameterKey::DexMaxOrdersPerBook), 512);
}

// ---------------------------------------------------------------------------
// reorgs and the state root
// ---------------------------------------------------------------------------

#[test]
fn reverting_a_block_restores_the_rules_it_changed() {
    // A parameter left at the abandoned chain's value is a rule nobody voted
    // for on the chain that survived.
    let fixture = fixture();
    fixture.lock(0, 4_000_000, 200_000, 1);
    fixture.lock(1, 4_000_000, 200_000, 2);

    let (id, executable) = fixture.propose(ParameterKey::DexProtocolFeeBps, 25, 3);
    fixture.vote(0, id, Choice::For, 10).expect("vote");
    fixture.vote(1, id, Choice::For, 11).expect("vote");

    let root_before = fixture.db.state_root().expect("root");
    let block_id = [77u8; 32];
    let context = BlockContext::at_height(executable);
    let mut block = block_of(vec![]);
    block.header.state_root = fixture.db.preview_root(&block, context).expect("preview");
    fixture
        .db
        .apply_block_journaled(&block, &block_id, context)
        .expect("execute");
    assert_eq!(
        fixture.db.uncovered_keys().expect("scan"),
        Vec::<Vec<u8>>::new(),
        "every stored key must be under the state root or declared local-only"
    );

    assert_eq!(fixture.parameter(ParameterKey::DexProtocolFeeBps), 25);

    fixture.db.revert_block(&block_id).expect("revert");

    assert_eq!(fixture.parameter(ParameterKey::DexProtocolFeeBps), 0);
    assert_eq!(fixture.state_of(&id), ProposalState::Voting);
    assert_eq!(fixture.db.state_root().expect("root"), root_before);
}

#[test]
fn a_chain_that_has_never_governed_has_the_state_root_it_always_had() {
    // The subsystem folds into the root only when there is something in it, so
    // adding governance does not change a chain that has not used it.
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    common::bind(&db);
    let key = generate_signing_key().expect("key");
    let account = Account {
        balance: 1_000,
        nonce: 0,
    };
    db.put_account(&key.address(), &account).expect("fund");

    let expected = custom_l1_node::state::merkle_root(&[custom_l1_node::state::account_leaf(
        &key.address(),
        &account,
    )]);
    assert_eq!(db.state_root().expect("root"), expected);

    // And a block still applies: with no proposals, the end-of-block pass
    // writes nothing at all.
    db.apply_block(&block_of(vec![]), BlockContext::at_height(1))
        .expect("apply");
    assert_eq!(db.state_root().expect("root"), expected);
}

#[test]
fn every_parameter_reads_as_its_compiled_default_before_any_vote() {
    let fixture = fixture();
    let table = fixture.db.parameters().expect("parameters");
    for (key, value) in table.entries() {
        assert_eq!(value, key.bounds().default, "{}", key.label());
    }
}
