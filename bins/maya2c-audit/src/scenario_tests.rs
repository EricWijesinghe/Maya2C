//! The audit run end to end against the in-process fake: the happy chain passes,
//! and each seeded fault is caught by the check that owns it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_amount::Amount;

use crate::fixture::{Fake, chain, plan};
use crate::genesis::Plan;
use crate::numbers::{FEE_COLLECTOR, FEE_SINK};
use crate::report::{Finding, Report, Verdict};
use crate::{AuditError, audit_supply};

fn run(fake: &Fake, plan: &Plan) -> Report {
    audit_supply(fake, plan, None, None).expect("audit runs")
}

fn finding<'a>(report: &'a Report, id: &str) -> &'a Finding {
    report
        .findings
        .iter()
        .find(|f| f.id == id)
        .expect("finding present")
}

fn mentions(finding: &Finding, needle: &str) -> bool {
    finding.lines.iter().any(|l| l.contains(needle))
}

#[test]
fn the_honest_chain_passes_every_check() {
    let report = run(&chain(), &plan());
    for f in &report.findings {
        assert_eq!(f.verdict, Verdict::Pass, "{}: {:?}", f.id, f.lines);
    }
    assert_eq!(report.overall(), Verdict::Pass);
    assert_eq!(report.exit_code(), 0);
    assert!(mentions(finding(&report, "reserve-emission"), "paid 1650"));
    assert!(mentions(
        finding(&report, "reserve-emission"),
        "unspent miner budget 850"
    ));
    assert!(mentions(
        finding(&report, "reserve-genesis"),
        "= genesis 1000000"
    ));
}

#[test]
fn a_printed_coin_fails_supply_conservation_at_the_tip() {
    let mut fake = chain();
    fake.supply_now.total = Amount::from_u64(1_005_001);
    let report = run(&fake, &plan());
    let f = finding(&report, "supply-conservation");
    assert_eq!(f.verdict, Verdict::Fail);
    assert!(mentions(f, "tip 12"));
    assert!(mentions(f, "coins were printed"));
    assert_eq!(report.exit_code(), 1);
}

#[test]
fn a_vanished_coin_is_reported_as_missing_not_minted() {
    let mut fake = chain();
    fake.supply_now.total = Amount::from_u64(1_004_999);
    let f = finding(&run(&fake, &plan()), "supply-conservation").clone();
    assert_eq!(f.verdict, Verdict::Fail);
    assert!(mentions(&f, "value vanished"));
}

#[test]
fn miners_paid_past_their_budget_fail_the_epoch() {
    let mut fake = chain();
    // Height 1 pays 800 instead of 200: epoch 0 shares become 1050, over the 700 budget.
    fake.changes.get_mut(&1).unwrap()[0].after = Amount::from_u64(998_800);
    let f = finding(&run(&fake, &plan()), "reserve-emission").clone();
    assert_eq!(f.verdict, Verdict::Fail);
    assert!(mentions(
        &f,
        "epoch 0 (heights 1..=4): miners paid 1450, over the budget 700"
    ));
}

#[test]
fn a_reserve_that_falls_short_of_the_committee_fails() {
    let mut fake = chain();
    // Height 8, the boundary of epoch 1, pays nothing. Epoch 1 drops 100 against a committee of 300.
    fake.changes.get_mut(&8).unwrap().clear();
    let f = finding(&run(&fake, &plan()), "reserve-emission").clone();
    assert_eq!(f.verdict, Verdict::Fail);
    assert!(mentions(
        &f,
        "epoch 1 (heights 5..=8): reserve fell 100, below the committee part 300"
    ));
}

#[test]
fn the_reserve_rising_fails_emission_and_reconciliation() {
    let mut fake = chain();
    fake.reserve_move(3, 999_550, 999_600);
    let report = run(&fake, &plan());
    assert_eq!(finding(&report, "reserve-emission").verdict, Verdict::Fail);
    assert!(mentions(
        finding(&report, "reserve-emission"),
        "reserve rose 999550 -> 999600"
    ));
    assert_eq!(finding(&report, "reserve-genesis").verdict, Verdict::Fail);
}

#[test]
fn a_reserve_that_does_not_reconcile_to_genesis_fails() {
    let mut fake = chain();
    fake.reserve_now = Amount::from_u64(998_349);
    let f = finding(&run(&fake, &plan()), "reserve-genesis").clone();
    assert_eq!(f.verdict, Verdict::Fail);
    assert!(mentions(&f, "!= genesis 1000000"));
}

#[test]
fn a_burn_larger_than_the_fees_collected_fails_at_that_height() {
    let mut fake = chain();
    // Height 2 collected 10. Sink +11 is 11 burned.
    fake.changes
        .get_mut(&2)
        .unwrap()
        .iter_mut()
        .find(|c| c.address == FEE_SINK)
        .unwrap()
        .after = Amount::from_u64(11);
    let f = finding(&run(&fake, &plan()), "fee-sink").clone();
    assert_eq!(f.verdict, Verdict::Fail);
    assert!(mentions(
        &f,
        "height 2: burned 11 from fees, but only 10 was collected"
    ));
}

#[test]
fn a_collector_that_gains_the_wrong_amount_fails_off_boundary() {
    let mut fake = chain();
    // Height 6 collected 7 and burned 3, so the collector must gain 4, not 7.
    fake.changes
        .get_mut(&6)
        .unwrap()
        .iter_mut()
        .find(|c| c.address == FEE_COLLECTOR)
        .unwrap()
        .after = Amount::from_u64(57);
    let f = finding(&run(&fake, &plan()), "fee-sink").clone();
    assert_eq!(f.verdict, Verdict::Fail);
    assert!(mentions(
        &f,
        "height 6: collector gained 7, expected collected 7 - burned 3"
    ));
}

#[test]
fn a_pruned_height_is_not_verified_and_never_a_pass() {
    let mut fake = chain();
    fake.refuse_changes.push(3);
    let report = run(&fake, &plan());
    assert_eq!(
        finding(&report, "reserve-emission").verdict,
        Verdict::Unavailable
    );
    assert_eq!(finding(&report, "fee-sink").verdict, Verdict::Unavailable);
    // Epoch 0 holds the refused height; epochs 1 and 2 hold none and still pass.
    assert!(mentions(
        finding(&report, "reserve-emission"),
        "epochs 1..=2 judged"
    ));
    assert_eq!(report.overall(), Verdict::Unavailable);
    assert_eq!(report.exit_code(), 3);
}

#[test]
fn an_emptied_reserve_ends_emission_and_is_not_a_failure() {
    let mut fake = chain();
    // Height 12 leaves the reserve at zero: the emission ended, nothing to judge.
    fake.changes.get_mut(&12).unwrap()[0].after = Amount::ZERO;
    fake.reserve_now = Amount::ZERO;
    let report = run(&fake, &plan());
    let f = finding(&report, "reserve-emission");
    assert_eq!(f.verdict, Verdict::Unavailable);
    assert!(mentions(
        f,
        "epoch 2 (heights 9..=12): reserve empty at the end"
    ));
}

#[test]
fn a_chain_without_mining_skips_emission_and_still_checks_fees() {
    let mut p = plan();
    p.mining = None;
    let report = run(&chain(), &p);
    assert_eq!(
        finding(&report, "reserve-emission").verdict,
        Verdict::Skipped
    );
    assert_eq!(
        finding(&report, "reserve-genesis").verdict,
        Verdict::Skipped
    );
    assert_eq!(finding(&report, "fee-sink").verdict, Verdict::Pass);
}

#[test]
fn a_window_that_cuts_an_epoch_judges_only_whole_epochs() {
    let report = audit_supply(&chain(), &plan(), Some(2), Some(12)).expect("audit runs");
    let f = finding(&report, "reserve-emission");
    assert_eq!(f.verdict, Verdict::Pass);
    assert!(mentions(f, "epochs 1..=2 judged"));
    assert!(mentions(f, "1 epoch(s) only partly inside the window"));
    assert_eq!(
        finding(&report, "reserve-genesis").verdict,
        Verdict::Skipped
    );
}

#[test]
fn a_window_outside_the_chain_is_refused() {
    let err = audit_supply(&chain(), &plan(), Some(1), Some(99)).unwrap_err();
    assert!(matches!(err, AuditError::Window { tip: 12, .. }));
}

#[test]
fn a_transport_failure_stops_the_audit_instead_of_passing_it() {
    let mut fake = chain();
    fake.broken = true;
    assert!(matches!(
        audit_supply(&fake, &plan(), None, None),
        Err(AuditError::Rpc(_))
    ));
}
