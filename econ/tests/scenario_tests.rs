//! The scenarios run, are reproducible, and the chain's rules hold inside
//! them.

use maya_econ::estimator::measure;
use maya_econ::model::{Stability, run};
use maya_econ::params::EconParams;
use maya_econ::scenario::{self, Scenario};
use maya_fee_market::MAX_SUPPLY;

fn find(name: &str) -> Scenario {
    scenario::all().into_iter().find(|s| s.name.starts_with(name)).expect("scenario exists")
}

#[test]
fn every_scenario_runs_and_never_exceeds_the_supply_cap() {
    let p = EconParams::DRAFT;
    for s in scenario::all() {
        let r = run(&p, &s, 1);
        assert_eq!(r.days.len() as u64, s.days, "{}", s.name);
        assert!(r.days.iter().all(|d| d.supply <= MAX_SUPPLY), "{}", s.name);
        assert!(r.days.iter().all(|d| d.base_fee >= p.fees.min_base_fee), "{}", s.name);
    }
}

#[test]
fn a_run_is_a_function_of_its_seed() {
    let p = EconParams::DRAFT;
    let s = find("baseline");
    let a = run(&p, &s, 99);
    let b = run(&p, &s, 99);
    let key = |r: &maya_econ::model::Run| r.days.iter().map(|d| (d.supply, d.base_fee, d.staking_ppm)).collect::<Vec<_>>();
    assert_eq!(key(&a), key(&b));
}

#[test]
fn surging_usage_burns_more_than_baseline_and_spam_gets_expensive() {
    let p = EconParams::DRAFT;
    let base = run(&p, &find("baseline"), 3);
    let surge = run(&p, &find("sudden 100x"), 3);
    let burned = |r: &maya_econ::model::Run| r.days.iter().map(|d| u128::from(d.burned)).sum::<u128>();
    assert!(burned(&surge) > 10 * burned(&base));
    let spam = run(&p, &find("fee-market spam"), 3);
    let peak = spam.days[60..67].iter().map(|d| d.peak_base_fee).max().unwrap_or(0);
    let before = spam.days[59].base_fee;
    assert!(peak > 100 * before, "spam raised the base fee only {before} -> {peak}");
}

#[test]
fn the_validator_set_is_funded_above_the_break_even_price_and_not_below() {
    let p = EconParams::DRAFT;
    let be = p.break_even_price();
    let rich = EconParams { genesis_price: 2.0 * be, ..p };
    let poor = EconParams { genesis_price: 0.5 * be, ..p };
    assert!(run(&rich, &find("baseline"), 5).is_stable(p.validators, Stability::DEFAULT));
    assert!(run(&poor, &find("baseline"), 5).min_validators() < p.validators * 3 / 4);
}

#[test]
fn the_fee_quote_is_never_exceeded() {
    let f = EconParams::DRAFT.fees;
    let sizes: Vec<u64> = (0..5_000u64).map(|i| (i * 7919) % (2 * f.target_block_bytes)).collect();
    let a = measure(&sizes, 5, f.target_block_bytes, f.change_denominator, f.min_base_fee, 500);
    assert_eq!(a.violations, 0);
    assert!(a.samples > 4_000);
}

#[test]
fn supply_change_is_reported_without_overflow() {
    // A surge that burns a large share of supply once reported +0.86% here,
    // because the ppm product overflowed i64.
    let p = EconParams { genesis_price: 63.0, ..EconParams::DRAFT };
    let r = run(&p, &find("sudden 100x"), 42);
    let (first, last) = (r.days[0].supply, r.days[r.days.len() - 1].supply);
    assert!(last < first);
    assert!(r.supply_change_ppm() < 0);
}

