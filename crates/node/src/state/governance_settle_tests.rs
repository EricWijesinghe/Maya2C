//! Cost and equivalence of the end-of-block governance pass.
//!
//! In-crate because the pass (`StateDB::settle_governance`) is `pub(crate)`,
//! and timing it through `apply_block` would mostly time the state root.
//! `settle_governance_full_scan` is the pass as it was before the settlement
//! index (ADR-082 amendment 2026-11-13), kept so both can be compared.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use maya_governance::Amount;
use maya_governance::params::ParameterKey;
use maya_governance::proposal::{Proposal, ProposalState, Schedule};
use maya_governance::tally::Tally;
use primitive_types::U256;
use tempfile::TempDir;

use crate::governance::work::WorkRecord;
use crate::governance::{DUE_PREFIX, ProposalRecord, TOTALS_KEY, Totals, due_key, proposal_key};
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};

fn open() -> (StateDB, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    (StateDB::open(dir.path()).expect("open"), dir)
}

fn id_of(index: u64) -> [u8; 32] {
    *blake3::hash(&index.to_le_bytes()).as_bytes()
}

/// Writes a record and the index keys `propose` would have written for it.
fn put(db: &StateDB, id: &[u8; 32], record: &ProposalRecord, indexed: bool) {
    db.raw_put(&proposal_key(id), &record.encode()).unwrap();
    if indexed {
        let schedule = record.proposal.schedule;
        for height in [schedule.voting_closes + 1, schedule.executable_from] {
            db.raw_put(&due_key(height, id), &[]).unwrap();
        }
    }
}

fn record(proposal: Proposal, changes: Vec<(u16, Amount)>) -> ProposalRecord {
    ProposalRecord {
        proposer: [7; 32],
        deposit: U256::zero(),
        proposal,
        changes,
        upgrade: None,
    }
}

fn run(db: &StateDB, height: u64, full_scan: bool) -> (crate::error::Result<()>, Overlay) {
    let mut overlay = Overlay::default();
    let context = BlockContext::at_height(height);
    let result = if full_scan {
        db.settle_governance_full_scan(&mut overlay, context)
    } else {
        db.settle_governance(&mut overlay, context)
    };
    (result, overlay)
}

// ----------------------------------------------------------------- behaviour

/// The pass must not read a proposal that is not due. Before the index it
/// read every record on every block, so an undecodable record that nothing
/// will ever settle failed every block; the full scan below still does.
#[test]
fn the_pass_reads_no_proposal_that_is_not_due() {
    let (db, _dir) = open();
    db.raw_put(&proposal_key(&id_of(0)), &[0xff]).unwrap();
    let due = Proposal::open(0, 7_200, 14_400).unwrap();
    put(&db, &id_of(1), &record(due, Vec::new()), true);
    let closes = due.schedule.voting_closes + 1;

    let (result, overlay) = run(&db, closes, false);
    result.expect("a record that is not due is not read");
    let written = overlay.records[&proposal_key(&id_of(1))].as_ref().unwrap();
    assert_eq!(
        ProposalRecord::decode(written).unwrap().proposal.state,
        ProposalState::Rejected,
        "the proposal that was due is settled"
    );
    assert_eq!(
        overlay.records[&due_key(closes, &id_of(1))],
        None,
        "its index key is consumed"
    );

    assert!(
        run(&db, closes, true).0.is_err(),
        "the full scan reads the record nobody is due to settle"
    );
}

/// A small deterministic generator: the test must replay identically.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

const STATES: [ProposalState; 6] = [
    ProposalState::Voting,
    ProposalState::Queued,
    ProposalState::Executed,
    ProposalState::Rejected,
    ProposalState::Cancelled,
    ProposalState::Expired,
];

fn random_record(rng: &mut XorShift) -> ProposalRecord {
    let state = STATES[rng.below(6) as usize];
    let created = rng.below(100);
    let voting_closes = created + 1 + rng.below(60);
    let executable_from = voting_closes + 1 + rng.below(60);
    let expires_after = executable_from + rng.below(60);
    let weight = |rng: &mut XorShift| Amount::from_u64(rng.below(1_000));
    let tally = Tally {
        for_votes: weight(rng),
        against_votes: weight(rng),
        abstain_votes: weight(rng),
    };
    // One protocol-fee change in half of them, so two proposals executing in
    // one pass overwrite each other and the order between them shows.
    let changes = if rng.below(2) == 0 {
        vec![(
            ParameterKey::DexProtocolFeeBps.tag(),
            Amount::from_u64(rng.below(201)),
        )]
    } else {
        Vec::new()
    };
    ProposalRecord {
        proposer: id_of(rng.below(4)),
        deposit: if state.is_settled() {
            U256::zero()
        } else {
            U256::from(rng.below(5))
        },
        proposal: Proposal {
            state,
            schedule: Schedule {
                created,
                voting_closes,
                executable_from,
                expires_after,
            },
            tally,
        },
        changes,
        upgrade: None,
    }
}

/// Staged records, and the staged accounts rendered for comparison.
type Effects = (Vec<(Vec<u8>, Option<Vec<u8>>)>, String);

/// Everything a pass staged except the index keys it consumed.
fn effects(overlay: &Overlay) -> Effects {
    let records = overlay
        .records
        .iter()
        .filter(|(key, _)| !key.starts_with(DUE_PREFIX))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    (records, format!("{:?}", overlay.accounts))
}

/// The indexed pass and the full scan stage the same records and balances,
/// or fail the same way, on random sets of proposals in every state, at
/// heights before, inside and after their windows, including heights a
/// replay reaches by skipping (both of a proposal's keys due at once).
#[test]
fn the_indexed_pass_matches_the_full_scan() {
    let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
    let mut settled = 0_usize;
    for _ in 0..40 {
        let (db, _dir) = open();
        let totals = Totals {
            locked: U256::from(1_500),
            work: WorkRecord {
                credit: 0,
                last_height: 0,
            },
        };
        db.raw_put(TOTALS_KEY, &totals.encode()).unwrap();
        for index in 0..30 {
            let record = random_record(&mut rng);
            // A settled proposal's keys may or may not have been consumed.
            let indexed = !record.proposal.state.is_settled() || rng.below(2) == 0;
            put(&db, &id_of(1_000 + index), &record, indexed);
        }
        for height in [0, 1, 30, 61, 62, 90, 121, 150, 200, 260, 400] {
            let (old_result, old) = run(&db, height, true);
            let (new_result, new) = run(&db, height, false);
            assert_eq!(
                format!("{old_result:?}"),
                format!("{new_result:?}"),
                "height {height}"
            );
            assert_eq!(effects(&old), effects(&new), "height {height}");
            settled += effects(&new).0.len();
        }
    }
    assert!(settled > 0, "the random sets must exercise some transition");
}

// --------------------------------------------------------------- measurement

/// `n` open proposals whose vote closes long after any height measured here,
/// with the index keys `propose` writes.
fn seed_open(db: &StateDB, n: u64) {
    let proposal = Proposal::open(1, 1_500_000, 3_000_000).unwrap();
    for index in 0..n {
        put(db, &id_of(index), &record(proposal, Vec::new()), true);
    }
}

fn median(runs: usize, mut time: impl FnMut(usize) -> Duration) -> Duration {
    let mut all: Vec<Duration> = (0..runs).map(&mut time).collect();
    all.sort();
    all[runs / 2]
}

/// Times both passes against `n` open proposals none of which is due, then
/// the indexed pass against a full height of due proposals that all execute.
/// Prints real figures; run with `--ignored --nocapture`.
#[test]
#[ignore = "measurement: prints timings, asserts nothing about speed"]
fn measure_settlement_pass() {
    const RUNS: usize = 5;
    for n in [0_u64, 1_000, 10_000, 100_000] {
        let (db, _dir) = open();
        seed_open(&db, n);
        let time = |full_scan: bool| {
            median(RUNS, |r| {
                let started = Instant::now();
                run(&db, 1_000 + r as u64, full_scan).0.unwrap();
                started.elapsed()
            })
        };
        println!(
            "governance-settle open={n:>6} not due: full-scan median={:?} indexed median={:?}",
            time(true),
            time(false)
        );
    }

    let (db, _dir) = open();
    let height = 50_000;
    let cap = crate::state::governance_exec::MAX_PROPOSALS_DUE_PER_HEIGHT as u64;
    for index in 0..cap {
        let mut proposal = Proposal::open(1, 7_200, 14_400).unwrap();
        proposal.state = ProposalState::Queued;
        proposal.schedule.executable_from = height;
        let change = (
            ParameterKey::DexProtocolFeeBps.tag(),
            Amount::from_u64(index),
        );
        db.raw_put(
            &proposal_key(&id_of(index)),
            &record(proposal, vec![change]).encode(),
        )
        .unwrap();
        db.raw_put(&due_key(height, &id_of(index)), &[]).unwrap();
    }
    let indexed = median(RUNS, |_| {
        let started = Instant::now();
        let (result, overlay) = run(&db, height, false);
        result.unwrap();
        assert!(
            overlay.records.len() as u64 > cap,
            "every due proposal executed"
        );
        started.elapsed()
    });
    println!("governance-settle due={cap} all executing: indexed median={indexed:?}");
}
