//! Scheduled payments (ADR-135 P-7, feature bit 46) against real block
//! execution: the replay tests `docs/threat-models/v4-scheduled-payments.md`
//! lists, one per attack, each named for what it pins, plus the activation,
//! cancel and refusal rules.
//!
//! Every refusal asserts the specific error, so a stub that refuses everything
//! fails these tests rather than passing them. Schedules many tests need in
//! bulk are seeded straight to disk in the layout `state/scheduled.rs`
//! documents, as a committed earlier block would have left them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::core::schedule_payload::ScheduleAction;
use custom_l1_node::core::vault_payload::{VaultAction, VaultConfig};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::governance::ParameterTable;
use custom_l1_node::network::mempool::Mempool;
use custom_l1_node::state::fees::FEE_COLLECTOR;
use custom_l1_node::state::invariant_guard::{BreakerRecord, Invariant, Module, guard_key};
use custom_l1_node::state::scheduled::{
    MAX_DUE_PER_BLOCK, MAX_MISSED, MAX_RUNS, MIN_INTERVAL, ScheduleRecord, schedule_id,
};
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_governance::limits::MAX_SCHEDULE_HORIZON_BLOCKS as HORIZON;
use maya_governance::params::ParameterKey;
use primitive_types::U256;
use tempfile::TempDir;

mod common;

/// The v4 activation height these tests run under.
const V4: u64 = 10;
const FUNDS: u64 = 1_000_000;
const AMOUNT: u64 = 100;
const PAYEE: Address = [0x42; 32];

struct Fixture {
    db: Arc<StateDB>,
    _dir: TempDir,
}

fn fixture(funded: &[Address]) -> Fixture {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    common::bind(&db);
    for address in funded {
        db.put_account(
            address,
            &Account {
                balance: U256::from(FUNDS),
                nonce: 0,
            },
        )
        .expect("fund");
    }
    Fixture {
        db: Arc::new(db),
        _dir: dir,
    }
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
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

/// A context at `height` with bit 46 active from [`V4`].
fn at(height: u64) -> BlockContext {
    BlockContext::at_height(height).with_scheduled_activation(V4)
}

fn apply(f: &Fixture, txs: Vec<Transaction>, height: u64) -> Result<[u8; 32], NodeError> {
    f.db.apply_block(&block_of(txs), at(height))
}

fn create(payee: Address, amount: U256, first_due: u64, runs: u32) -> TxKind {
    TxKind::Schedule(ScheduleAction::Create {
        payee,
        amount,
        first_due,
        interval: MIN_INTERVAL,
        runs,
    })
}

fn cancel(id: [u8; 32]) -> TxKind {
    TxKind::Schedule(ScheduleAction::Cancel { schedule_id: id })
}

fn balance(f: &Fixture, who: &Address) -> U256 {
    f.db.get_account(who).expect("read").balance
}

fn schedule(f: &Fixture, id: &[u8; 32]) -> Option<ScheduleRecord> {
    f.db.get_schedule(id).expect("read")
}

fn indexed(f: &Fixture, next_due: u64, id: &[u8; 32]) -> bool {
    f.db.is_due_indexed(next_due, id).expect("read")
}

fn refusal(error: &NodeError) -> &'static str {
    match error {
        NodeError::ScheduleRefused { reason } => reason,
        other => panic!("expected ScheduleRefused, got {other:?}"),
    }
}

/// A payer that created one schedule at nonce 0 in block [`V4`]: `runs` runs
/// of [`AMOUNT`] to [`PAYEE`], the first due at `V4 + 5`.
struct Created {
    f: Fixture,
    key: HybridSigningKey,
    payer: Address,
    id: [u8; 32],
    first_due: u64,
}

fn created(runs: u32) -> Created {
    let key = generate_signing_key().expect("key");
    let payer = key.address();
    let f = fixture(&[payer]);
    let first_due = V4 + 5;
    let tx = signed(create(PAYEE, U256::from(AMOUNT), first_due, runs), 0, &key);
    apply(&f, vec![tx], V4).expect("create");
    Created {
        f,
        key,
        payer,
        id: schedule_id(&payer, 0),
        first_due,
    }
}

/// Writes a schedule and its due-index row straight to disk.
fn seed(f: &Fixture, id: &[u8; 32], record: &ScheduleRecord) {
    f.db.raw_put_for_test(&[&b"sp:s:"[..], &id[..]].concat(), &record.encode())
        .expect("seed record");
    f.db.raw_put_for_test(
        &[&b"sp:d:"[..], &record.next_due.to_be_bytes()[..], &id[..]].concat(),
        &[],
    )
    .expect("seed index");
}

/// A one-run schedule of one unit to `payee`, due at `due`, prepaid.
fn one_run(payer: Address, payee: Address, due: u64) -> ScheduleRecord {
    ScheduleRecord {
        payer,
        payee,
        amount: U256::one(),
        next_due: due,
        interval: MIN_INTERVAL,
        remaining: 1,
        escrow: U256::one(),
        missed: 0,
    }
}

/// `count` one-run schedules due at `due`, each paying its own payee, so a
/// payee's balance says whether its schedule ran. Ids sorted ascending: the
/// order the step takes them in.
fn seed_many(f: &Fixture, count: usize, due: u64) -> Vec<([u8; 32], Address)> {
    let mut seeded: Vec<([u8; 32], Address)> = (0..count)
        .map(|i| {
            let n = u64::try_from(i).unwrap();
            let id = *blake3::hash(&n.to_le_bytes()).as_bytes();
            let payee = *blake3::hash(&[&b"payee"[..], &n.to_le_bytes()[..]].concat()).as_bytes();
            (id, payee)
        })
        .collect();
    for (id, payee) in &seeded {
        seed(f, id, &one_run([0x11; 32], *payee, due));
    }
    seeded.sort_unstable();
    seeded
}

fn ran(f: &Fixture, payee: &Address) -> bool {
    balance(f, payee) == U256::one()
}

fn set_run_fee(f: &Fixture, fee: u64) {
    let table = ParameterTable::new().with(
        ParameterKey::ScheduleRunFee,
        maya_dex::Amount::from_u64(fee),
    );
    f.db.raw_put_for_test(b"g:params", &table.encode())
        .expect("params");
}

// 1 (S-6)
#[test]
fn a_schedule_runs_at_its_due_height_and_not_one_block_earlier() {
    let c = created(2);
    let escrow = U256::from(2 * AMOUNT);
    assert_eq!(balance(&c.f, &c.payer), U256::from(FUNDS) - escrow);
    let record = schedule(&c.f, &c.id).expect("written");
    assert_eq!(record.escrow, escrow);
    assert!(indexed(&c.f, c.first_due, &c.id));

    apply(&c.f, vec![], c.first_due - 1).expect("early block");
    assert_eq!(balance(&c.f, &PAYEE), U256::zero(), "not one block early");

    apply(&c.f, vec![], c.first_due).expect("due block");
    assert_eq!(balance(&c.f, &PAYEE), U256::from(AMOUNT));
    let after = schedule(&c.f, &c.id).expect("one run left");
    assert_eq!(after.remaining, 1);
    assert_eq!(after.escrow, U256::from(AMOUNT));
    assert_eq!(after.next_due, c.first_due + MIN_INTERVAL);
    assert!(!indexed(&c.f, c.first_due, &c.id), "old index row gone");
    assert!(indexed(&c.f, c.first_due + MIN_INTERVAL, &c.id));

    // The last run closes the schedule: both rows deleted, nothing refunded
    // because nothing is left.
    apply(&c.f, vec![], c.first_due + MIN_INTERVAL).expect("last run");
    assert_eq!(balance(&c.f, &PAYEE), U256::from(2 * AMOUNT));
    assert!(schedule(&c.f, &c.id).is_none());
    assert!(!indexed(&c.f, c.first_due + MIN_INTERVAL, &c.id));
    assert_eq!(balance(&c.f, &c.payer), U256::from(FUNDS) - escrow);
}

// 2 (S-1, invariant 33): the blocking worst-case block. At most
// MAX_DUE_PER_BLOCK runs in one block, in (next_due, id) order; the rest
// carries to the next block and runs there first.
#[test]
fn the_257th_due_schedule_runs_first_in_the_next_block() {
    assert_eq!(MAX_DUE_PER_BLOCK, 256, "ADR-135 P-7 fixes the bound");
    let f = fixture(&[]);
    let h = V4 + 100;
    let seeded = seed_many(&f, MAX_DUE_PER_BLOCK + 1, h);
    // One more due later, so "first in the next block" is observable: the
    // carried-over row sorts before a row due at the next height.
    let later = ([0u8; 32], [0xEE; 32]);
    seed(&f, &later.0, &one_run([0x11; 32], later.1, h + 1));

    apply(&f, vec![], h).expect("capped block");
    for (i, (id, payee)) in seeded.iter().enumerate() {
        if i < MAX_DUE_PER_BLOCK {
            assert!(ran(&f, payee), "#{i} is within the cap, in id order");
            assert!(schedule(&f, id).is_none());
        } else {
            assert!(!ran(&f, payee), "#{i} carries over");
            assert!(indexed(&f, h, id), "#{i} keeps its due row");
        }
    }
    assert!(!ran(&f, &later.1));

    apply(&f, vec![], h + 1).expect("next block");
    assert!(ran(&f, &seeded[MAX_DUE_PER_BLOCK].1), "carried row ran");
    assert!(ran(&f, &later.1), "then the row due at h + 1");
}

// The scan reads `cap` plus this block's staged deletions, so a cancel among
// the first due rows does not cost the block one of its 256 runs.
#[test]
fn a_cancelled_due_row_does_not_shrink_the_blocks_run_count() {
    let key = generate_signing_key().expect("key");
    let f = fixture(&[key.address()]);
    let h = V4 + 100;
    // Id [0; 32] sorts before every hashed id at the same height.
    let first = [0u8; 32];
    seed(&f, &first, &one_run(key.address(), [0xEE; 32], h));
    let seeded = seed_many(&f, MAX_DUE_PER_BLOCK + 1, h);

    apply(&f, vec![signed(cancel(first), 0, &key)], h).expect("cancel + step");
    assert!(schedule(&f, &first).is_none());
    assert!(!ran(&f, &[0xEE; 32]), "the cancelled row did not run");
    assert_eq!(balance(&f, &key.address()), U256::from(FUNDS + 1), "refund");
    for (i, (_, payee)) in seeded.iter().enumerate() {
        assert_eq!(ran(&f, payee), i < MAX_DUE_PER_BLOCK, "#{i}");
    }
}

// 3 (S-1, S-2): a backlog delays schedules, never transactions.
#[test]
fn one_thousand_schedules_due_at_one_height_delay_schedules_not_transactions() {
    const BACKLOG: usize = 1_000;
    let sender = generate_signing_key().expect("key");
    let f = fixture(&[sender.address()]);
    let h = V4 + 100;
    let seeded = seed_many(&f, BACKLOG, h);

    let mut transfer = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: U256::from(7u64),
            recipient: [0x77; 32],
        }],
        0,
    );
    transfer.sign(&sender, &common::test_chain()).expect("sign");
    apply(&f, vec![transfer], h).expect("the transfer lands under the backlog");
    assert_eq!(balance(&f, &[0x77; 32]), U256::from(7u64));

    let mut done = seeded.iter().filter(|(_, p)| ran(&f, p)).count();
    assert_eq!(done, MAX_DUE_PER_BLOCK);
    let mut height = h;
    while done < BACKLOG {
        height += 1;
        apply(&f, vec![], height).expect("drain");
        let now = seeded.iter().filter(|(_, p)| ran(&f, p)).count();
        assert_eq!(now - done, (BACKLOG - done).min(MAX_DUE_PER_BLOCK));
        done = now;
    }
    // ceil(1000 / 256) = 4 blocks.
    assert_eq!(height, h + 3);
}

// 4 (S-4)
#[test]
fn escrow_arithmetic_at_the_2_pow_249_bound_is_accepted_and_one_over_refused() {
    let key = generate_signing_key().expect("key");
    let payer = key.address();
    let f = fixture(&[]);
    let bound = U256::one() << 249;
    let largest = bound - U256::one();
    let runs = MAX_RUNS;
    let escrow = largest * U256::from(runs);
    f.db.put_account(
        &payer,
        &Account {
            balance: escrow,
            nonce: 0,
        },
    )
    .expect("fund");

    let over = signed(create(PAYEE, bound, V4 + 5, runs), 0, &key);
    let err = apply(&f, vec![over], V4).unwrap_err();
    assert!(refusal(&err).contains("2^249"), "{err}");

    let at_bound = signed(create(PAYEE, largest, V4 + 5, runs), 0, &key);
    apply(&f, vec![at_bound], V4).expect("the largest unit at MAX_RUNS");
    assert_eq!(balance(&f, &payer), U256::zero());
    assert_eq!(
        schedule(&f, &schedule_id(&payer, 0))
            .expect("written")
            .escrow,
        escrow
    );

    // With a run fee the bound is on the sum: 2^249 - 1 plus 1 is refused.
    let g = fixture(&[]);
    g.db.put_account(
        &payer,
        &Account {
            balance: escrow,
            nonce: 0,
        },
    )
    .expect("fund");
    set_run_fee(&g, 1);
    let summed = signed(create(PAYEE, largest, V4 + 5, 1), 0, &key);
    let err =
        g.db.apply_block(&block_of(vec![summed]), at(V4))
            .unwrap_err();
    assert!(refusal(&err).contains("2^249"), "{err}");
}

// 5 (S-5, invariant 24)
#[test]
fn a_block_whose_root_omits_a_due_run_is_refused() {
    let c = created(1);
    let mut block = block_of(vec![]);
    // The root this block would have without the step: the same block under
    // a context where bit 46 is dark.
    let omitting_root =
        c.f.db
            .preview_root(&block, BlockContext::at_height(c.first_due))
            .expect("preview without the step");
    block.header.state_root = omitting_root;
    let err =
        c.f.db
            .apply_block_journaled(&block, &[1; 32], at(c.first_due))
            .unwrap_err();
    assert!(
        matches!(err, NodeError::StateRootMismatch { .. }),
        "{err:?}"
    );
    assert!(
        schedule(&c.f, &c.id).is_some(),
        "a refused block writes nothing"
    );

    block.header.state_root =
        c.f.db
            .preview_root(&block, at(c.first_due))
            .expect("preview");
    assert_ne!(
        block.header.state_root, omitting_root,
        "the run moves the root"
    );
    c.f.db
        .apply_block_journaled(&block, &[2; 32], at(c.first_due))
        .expect("the honest root");
    assert_eq!(balance(&c.f, &PAYEE), U256::from(AMOUNT));
}

// 6 (S-7): transactions apply before the step.
#[test]
fn a_cancel_in_the_due_block_wins_over_the_run() {
    let c = created(3);
    let tx = signed(cancel(c.id), 1, &c.key);
    apply(&c.f, vec![tx], c.first_due).expect("cancel at the due height");
    assert_eq!(
        balance(&c.f, &PAYEE),
        U256::zero(),
        "the run did not happen"
    );
    assert_eq!(balance(&c.f, &c.payer), U256::from(FUNDS), "full refund");
    assert!(schedule(&c.f, &c.id).is_none());
    assert!(!indexed(&c.f, c.first_due, &c.id));
}

#[test]
fn only_the_payer_cancels_and_a_missing_schedule_is_refused() {
    let c = created(1);
    let stranger = generate_signing_key().expect("key");
    c.f.db
        .put_account(
            &stranger.address(),
            &Account {
                balance: U256::from(FUNDS),
                nonce: 0,
            },
        )
        .expect("fund");
    let err = apply(&c.f, vec![signed(cancel(c.id), 0, &stranger)], V4 + 1).unwrap_err();
    assert!(refusal(&err).contains("payer"), "{err}");
    let err = apply(&c.f, vec![signed(cancel([9; 32]), 1, &c.key)], V4 + 1).unwrap_err();
    assert!(refusal(&err).contains("no such schedule"), "{err}");
    assert!(schedule(&c.f, &c.id).is_some());
}

// 7 (S-8, S-9): an escrow short of a run is a counted no-op, never a block
// failure; the third miss closes and refunds.
#[test]
fn a_run_fee_raised_above_escrow_misses_three_times_then_refunds() {
    assert_eq!(MAX_MISSED, 3);
    let c = created(2);
    set_run_fee(&c.f, 1_000);
    let mut due = c.first_due;
    for miss in 1..MAX_MISSED {
        apply(&c.f, vec![], due).expect("a miss does not void the block");
        let r = schedule(&c.f, &c.id).expect("still open");
        assert_eq!(r.missed, miss);
        assert_eq!(r.remaining, 2, "a miss is not a run");
        assert_eq!(r.escrow, U256::from(2 * AMOUNT), "nothing moved");
        due += MIN_INTERVAL;
        assert_eq!(r.next_due, due);
        assert!(indexed(&c.f, due, &c.id));
    }
    apply(&c.f, vec![], due).expect("third miss");
    assert!(schedule(&c.f, &c.id).is_none(), "closed");
    assert!(!indexed(&c.f, due, &c.id));
    assert_eq!(balance(&c.f, &PAYEE), U256::zero());
    assert_eq!(
        balance(&c.f, &c.payer),
        U256::from(FUNDS),
        "escrow refunded"
    );
    assert_eq!(balance(&c.f, &FEE_COLLECTOR), U256::zero());
}

// RUN_FEE joins the tips: it is paid to the fee collector per run.
#[test]
fn a_run_pays_the_run_fee_to_the_fee_collector() {
    let key = generate_signing_key().expect("key");
    let payer = key.address();
    let f = fixture(&[payer]);
    set_run_fee(&f, 3);
    let tx = signed(create(PAYEE, U256::from(AMOUNT), V4 + 5, 2), 0, &key);
    apply(&f, vec![tx], V4).expect("create");
    assert_eq!(
        balance(&f, &payer),
        U256::from(FUNDS - 2 * (AMOUNT + 3)),
        "(amount + RUN_FEE) * runs, prepaid"
    );
    apply(&f, vec![], V4 + 5).expect("run");
    assert_eq!(balance(&f, &PAYEE), U256::from(AMOUNT));
    assert_eq!(balance(&f, &FEE_COLLECTOR), U256::from(3u64));
}

// 8 (S-10)
#[test]
fn a_reorg_restores_escrow_next_due_and_the_due_index() {
    let c = created(2);
    let root_before = c.f.db.state_root().expect("root");
    let before = schedule(&c.f, &c.id).expect("written");

    let mut block = block_of(vec![]);
    block.header.state_root =
        c.f.db
            .preview_root(&block, at(c.first_due))
            .expect("preview");
    let id = [9u8; 32];
    c.f.db
        .apply_block_journaled(&block, &id, at(c.first_due))
        .expect("run");
    assert_eq!(balance(&c.f, &PAYEE), U256::from(AMOUNT));
    assert_eq!(
        c.f.db.uncovered_keys().expect("scan"),
        Vec::<Vec<u8>>::new(),
        "sp: rows are under the root (invariant 25)"
    );

    c.f.db.revert_block(&id).expect("revert");
    assert_eq!(schedule(&c.f, &c.id), Some(before));
    assert!(indexed(&c.f, c.first_due, &c.id));
    assert!(!indexed(&c.f, c.first_due + MIN_INTERVAL, &c.id));
    assert_eq!(balance(&c.f, &PAYEE), U256::zero());
    assert_eq!(c.f.db.state_root().expect("root"), root_before);
}

// 9 (S-11)
#[test]
fn a_scheduled_breaker_skips_the_step_and_resumes_in_order() {
    assert!(Module::of(&cancel([1; 32])).is_some());
    assert_eq!(Module::of(&cancel([1; 32])), Some(Module::Scheduled));
    assert_eq!(
        Module::from_tag(Module::Scheduled.tag()),
        Some(Module::Scheduled)
    );
    let key = generate_signing_key().expect("key");
    let f = fixture(&[key.address()]);
    let h = V4 + 100;
    let seeded = seed_many(&f, MAX_DUE_PER_BLOCK + 1, h);
    let tripped = BreakerRecord {
        module: Module::Scheduled,
        invariant: Invariant::PoolValuePerShare,
        tripped_at: h,
        until: h + 2,
    };
    f.db.raw_put_for_test(&guard_key(Module::Scheduled), &tripped.encode())
        .expect("trip");

    apply(&f, vec![], h).expect("the step is skipped, not failed");
    apply(&f, vec![], h + 1).expect("still skipped");
    assert!(seeded.iter().all(|(_, p)| !ran(&f, p)), "nothing ran");
    let halted = signed(create(PAYEE, U256::from(AMOUNT), h + 10, 1), 0, &key);
    assert!(matches!(
        apply(&f, vec![halted], h + 1).unwrap_err(),
        NodeError::ModuleHalted { .. }
    ));

    // Lifted at h + 2: the backlog resumes in (next_due, id) order, capped.
    apply(&f, vec![], h + 2).expect("resumes");
    for (i, (_, payee)) in seeded.iter().enumerate() {
        assert_eq!(ran(&f, payee), i < MAX_DUE_PER_BLOCK, "#{i}");
    }
}

// 10 (S-13, invariant 32). The threat model's name for this test assumed the
// escrow debit would be charged against the vault's window; invariant 32
// admits only a plain transfer or a vault action from a vault account, so the
// form is refused outright, which is stricter.
#[test]
fn a_vault_may_not_create_a_schedule_and_runs_are_not_vault_outflow() {
    let c = created(2);
    let guardian = [0x99; 32];
    let configure = VaultAction::Configure(VaultConfig {
        delay_blocks: 10,
        limit: U256::from(5u64),
        guardians: vec![guardian],
    });
    apply(
        &c.f,
        vec![signed(TxKind::Vault(Box::new(configure)), 1, &c.key)],
        V4 + 1,
    )
    .expect("the payer becomes a vault");
    let spent = || {
        c.f.db
            .committed_vault(&c.payer)
            .expect("read")
            .expect("vault")
            .window_spent
    };
    let before = spent();

    let tx = signed(create(PAYEE, U256::from(AMOUNT), V4 + 50, 1), 2, &c.key);
    let err = apply(&c.f, vec![tx], V4 + 2).unwrap_err();
    assert!(
        matches!(&err, NodeError::Vault(why) if why.contains("may not send")),
        "{err:?}"
    );

    // The schedule made before the vault still runs: escrow is already held
    // by the chain, so a run moves nothing out of the vault account.
    apply(&c.f, vec![], c.first_due).expect("run");
    assert_eq!(balance(&c.f, &PAYEE), U256::from(AMOUNT));
    assert_eq!(spent(), before, "a run is not charged to the vault window");
}

// 11 (S-14). FEE_SINK is the zero address, so two payees cover three names.
#[test]
fn a_payee_of_fee_collector_fee_sink_or_zero_is_refused() {
    let key = generate_signing_key().expect("key");
    let f = fixture(&[key.address()]);
    for payee in [FEE_COLLECTOR, [0u8; 32]] {
        let tx = signed(create(payee, U256::from(AMOUNT), V4 + 5, 1), 0, &key);
        let err = apply(&f, vec![tx], V4).unwrap_err();
        assert!(refusal(&err).contains("payee"), "{err}");
    }
    assert_eq!(balance(&f, &key.address()), U256::from(FUNDS));
}

// 12
#[test]
fn the_root_is_unchanged_when_no_schedule_is_due() {
    let dark = fixture(&[]);
    let lit = fixture(&[]);
    let empty = block_of(vec![]);
    let a = dark
        .db
        .apply_block(&empty, BlockContext::at_height(V4))
        .expect("dark");
    let b = lit.db.apply_block(&empty, at(V4)).expect("lit");
    assert_eq!(a, b);

    // A schedule that exists but is not due moves nothing: the same seeded
    // row under a dark binary and a lit one gives one root.
    let (dark, lit) = (fixture(&[]), fixture(&[]));
    for f in [&dark, &lit] {
        seed(f, &[3; 32], &one_run([0x11; 32], [0x33; 32], V4 + 50));
    }
    let a = dark
        .db
        .apply_block(&empty, BlockContext::at_height(V4 + 49))
        .expect("dark");
    let b = lit.db.apply_block(&empty, at(V4 + 49)).expect("lit");
    assert_eq!(a, b);
}

// 13 (plan I6, worst-case block): a number, not a target. The test pins the
// bound; the time it prints goes to `reports/` with the command and hardware.
#[test]
fn worst_case_block_with_max_due_runs_reports_its_wall_time() {
    let f = fixture(&[]);
    let h = V4 + 100;
    let seeded = seed_many(&f, MAX_DUE_PER_BLOCK * 2, h);
    let started = std::time::Instant::now();
    apply(&f, vec![], h).expect("worst-case block");
    let elapsed = started.elapsed();
    let done = seeded.iter().filter(|(_, p)| ran(&f, p)).count();
    assert_eq!(
        done, MAX_DUE_PER_BLOCK,
        "the cap holds with twice as many due"
    );
    eprintln!(
        "scheduled-payments worst case: {MAX_DUE_PER_BLOCK} runs in one block applied in {elapsed:?} (debug or release as built)"
    );
}

#[test]
fn creation_bounds_runs_interval_first_due_and_amount() {
    let key = generate_signing_key().expect("key");
    let f = fixture(&[key.address()]);
    let form = |amount: u64, first_due: u64, interval: u64, runs: u32| {
        signed(
            TxKind::Schedule(ScheduleAction::Create {
                payee: PAYEE,
                amount: U256::from(amount),
                first_due,
                interval,
                runs,
            }),
            0,
            &key,
        )
    };
    for (tx, rule) in [
        (form(AMOUNT, V4 + 5, MIN_INTERVAL, MAX_RUNS + 1), "runs"),
        (form(AMOUNT, V4 + 5, MIN_INTERVAL, 0), "runs"),
        (form(AMOUNT, V4 + 5, MIN_INTERVAL - 1, 1), "interval"),
        (form(AMOUNT, V4, MIN_INTERVAL, 1), "first_due"),
        (form(0, V4 + 5, MIN_INTERVAL, 1), "amount"),
        (form(AMOUNT, V4 + HORIZON + 1, MIN_INTERVAL, 1), "horizon"),
        (form(AMOUNT, V4 + 5, HORIZON + 1, 2), "horizon"),
        (form(AMOUNT, u64::MAX - 1, MIN_INTERVAL, 2), "horizon"),
    ] {
        let err = apply(&f, vec![tx], V4).unwrap_err();
        assert!(refusal(&err).contains(rule), "{rule}: {err}");
    }
    // Every boundary's accepted side, at once.
    apply(&f, vec![form(AMOUNT, V4 + 1, MIN_INTERVAL, MAX_RUNS)], V4)
        .expect("runs 120, interval 3600, first_due = height + 1");
    assert_eq!(
        balance(&f, &key.address()),
        U256::from(FUNDS - AMOUNT * u64::from(MAX_RUNS))
    );
    // The payer's own balance bounds the escrow: short by one unit is the
    // sender's refusal, not a no-op.
    let poor = generate_signing_key().expect("key");
    let g = fixture(&[]);
    g.db.put_account(
        &poor.address(),
        &Account {
            balance: U256::from(2 * AMOUNT - 1),
            nonce: 0,
        },
    )
    .expect("fund");
    let tx = signed(create(PAYEE, U256::from(AMOUNT), V4 + 5, 2), 0, &poor);
    assert!(matches!(
        apply(&g, vec![tx], V4).unwrap_err(),
        NodeError::InsufficientBalance { .. }
    ));
}

#[test]
fn schedules_are_refused_below_activation_and_never_by_default() {
    let key = generate_signing_key().expect("key");
    let f = fixture(&[key.address()]);
    let tx = || signed(create(PAYEE, U256::from(AMOUNT), V4 + 5, 1), 0, &key);
    assert_eq!(
        apply(&f, vec![tx()], V4 - 1).unwrap_err(),
        NodeError::ScheduleInactive {
            height: V4 - 1,
            activation: V4,
        }
    );
    assert!(matches!(
        f.db.apply_block(&block_of(vec![tx()]), BlockContext::at_height(u64::MAX - 1))
            .unwrap_err(),
        NodeError::ScheduleInactive { .. }
    ));
    // Admission refuses it too: no schedule entry names bit 46 here.
    let mempool = Mempool::new(Arc::clone(&f.db));
    assert!(matches!(
        mempool.validate(&tx()).unwrap_err(),
        NodeError::ScheduleInactive { .. }
    ));
    // A seeded row below activation is never run: the step is dark too.
    let g = fixture(&[]);
    seed(&g, &[3; 32], &one_run([0x11; 32], [0x33; 32], V4 - 2));
    g.db.apply_block(&block_of(vec![]), BlockContext::at_height(V4 - 1))
        .expect("dark step");
    assert!(!ran(&g, &[0x33; 32]));
}
