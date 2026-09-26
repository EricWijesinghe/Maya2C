//! Runs every scenario and analysis and prints Markdown for
//! `reports/18-economics.md`. `cargo run -p maya-econ --release --bin econ-sim`.

#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use maya_econ::attack::{Attack, tokens_needed, weak_subjectivity_days};
use maya_econ::concentration::{apply_cap, gini_ppm, nakamoto_coefficient};
use maya_econ::estimator::measure;
use maya_econ::mev::{reference_pool, sandwich_batch, sandwich_continuous};
use maya_econ::model::{Stability, run};
use maya_econ::params::{EconParams, UNIT};
use maya_econ::rng::Rng;
use maya_econ::{SIM_BANNER, scenario};

fn main() {
    let draft = EconParams::DRAFT;
    // Shocks are run from a funded starting point, 1.5x the break-even
    // price, so each row shows what the *shock* does rather than a set that
    // was never viable. The draft's own price input and its break-even are
    // printed first, so the choice is visible.
    let p = EconParams { genesis_price: (1.5 * draft.break_even_price()).round(), ..draft };
    println!("{SIM_BANNER}\n");
    println!(
        "Draft parameters: {} validators at {:.0} fiat/yr each, emission {:.1}%/yr, operator share of rewards {:.1}%.",
        p.validators,
        p.validator_cost_per_year,
        p.emission_ppm_per_year as f64 / 1e4,
        p.operator_share_ppm() as f64 / 1e4
    );
    println!(
        "Break-even token price for the full set on emission alone: {:.3} fiat. At the draft's own input of {:.3}, the set shrinks to what the budget pays for. Scenarios below start at {:.0} (1.5x break-even; an input).\n",
        p.break_even_price(),
        draft.genesis_price,
        p.genesis_price
    );
    println!("## Scenarios (730 days each, seed 42)\n");
    println!("| Scenario | min staking | final validators (min) | supply change | peak base fee | max fee/tx (fiat) | stable |");
    println!("|---|---|---|---|---|---|---|");
    for s in scenario::all() {
        let r = run(&p, &s, 42);
        let last = r.days.last().map_or(0, |d| d.validators);
        println!(
            "| {} | {:.1}% | {} ({}) | {:+.2}% | {} | {:.5} | {} |",
            s.name,
            r.min_staking_ppm() as f64 / 1e4,
            last,
            r.min_validators(),
            r.supply_change_ppm() as f64 / 1e4,
            r.days.iter().map(|d| d.peak_base_fee).max().unwrap_or(0),
            r.max_fee_fiat(),
            if r.is_stable(p.validators, Stability::DEFAULT) { "yes" } else { "**no**" }
        );
    }

    println!("\n## Cost of attack (supply 10M tokens)\n");
    println!("| Attack | stake needed | at 30% staked | at 50% | at 70% | slashable |");
    println!("|---|---|---|---|---|---|");
    let supply = p.genesis_supply / UNIT;
    for a in Attack::ALL {
        let t = |ratio| tokens_needed(a, ratio, supply);
        println!(
            "| {} | >{:.1}% of stake | {} | {} | {} | {} |",
            a.name(),
            a.stake_ppm() as f64 / 1e4,
            t(300_000),
            t(500_000),
            t(700_000),
            if a.slashable() { "yes" } else { "no" }
        );
    }
    println!("\nWeak-subjectivity period for a 21-day unbonding: {} days.", weak_subjectivity_days(21));

    println!("\n## Stake concentration (synthetic power-law set of 100 validators)\n");
    let mut rng = Rng::new(7);
    let stakes: Vec<u64> = (1..=100u64)
        .map(|i| 1_000_000 / i + rng.next_u64() % 1_000)
        .collect();
    for cap in [None, Some(50_000u64), Some(20_000)] {
        let s = cap.map_or_else(|| stakes.clone(), |c| apply_cap(&stakes, c));
        println!(
            "- cap {}: Nakamoto coefficient (1/3) = {}, Gini = {:.3}",
            cap.map_or("none".to_string(), |c| format!("{:.0}%", c as f64 / 1e4)),
            nakamoto_coefficient(&s, 333_334),
            gini_ppm(&s) as f64 / 1e6
        );
    }

    println!("\n## Sandwich extraction (maya-dex pool 10k/10k, victim buys 500)\n");
    println!("| Front-run size | continuous curve | batch auction |");
    println!("|---|---|---|");
    for attack in [100_000_000u64, 500_000_000, 1_000_000_000, 2_000_000_000] {
        let pool = reference_pool();
        println!(
            "| {} | {:+} | {:+} |",
            attack,
            sandwich_continuous(pool, 500_000_000, attack).unwrap_or(0),
            sandwich_batch(pool, 500_000_000, attack).unwrap_or(0)
        );
    }

    println!("\n## Fee estimator accuracy (k-block guaranteed ceiling)\n");
    let f = p.fees;
    let mut rng = Rng::new(9);
    let sizes: Vec<u64> = (0..50_000)
        .map(|i| {
            let surge = if (10_000..12_000).contains(&i) { 2.0 } else { 0.8 };
            ((f.target_block_bytes as f64 * surge * rng.noise(0.4)) as u64).min(2 * f.target_block_bytes)
        })
        .collect();
    for k in [1u32, 3, 10] {
        let a = measure(&sizes, k, f.target_block_bytes, f.change_denominator, f.min_base_fee, 1_000);
        println!(
            "- k = {k}: {} quotes, {} exceeded, mean over-estimate {:.1}%",
            a.samples,
            a.violations,
            a.mean_overestimate_ppm as f64 / 1e4
        );
    }
}
