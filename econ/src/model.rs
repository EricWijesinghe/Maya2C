//! The day-by-day model.
//!
//! Each simulated day runs `SAMPLED_BLOCKS` blocks through the chain's own
//! fee market — demand drawn per block, base fee stepped by
//! `maya_fee_market::next_base_fee`, each block's fees divided by
//! `maya_fee_market::split` — and scales the day's fee totals by
//! `epoch_blocks / SAMPLED_BLOCKS`. The base fee carries across days, so its
//! dynamics are the real rule's; only the day's totals are extrapolated.

#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use maya_fee_market::{MAX_SUPPLY, next_base_fee, split};

use crate::params::{EconParams, UNIT};
use crate::rng::Rng;
use crate::scenario::{Scenario, Shock};

/// Blocks simulated per day.
pub const SAMPLED_BLOCKS: u64 = 288;

/// A fiat willingness-to-pay per transaction; demand thins above it.
const WILLINGNESS_FIAT: f64 = 0.05;
/// Validators never fall below this many (the set's BFT floor for f = 1).
const MIN_VALIDATORS: u32 = 4;

/// One simulated day.
#[derive(Clone, Copy, Debug)]
pub struct Day {
    /// Day index.
    pub day: u64,
    /// Total supply, base units.
    pub supply: u64,
    /// Staked share, ppm.
    pub staking_ppm: u64,
    /// Base fee at day end, per byte.
    pub base_fee: u64,
    /// Highest base fee reached during the day, per byte.
    pub peak_base_fee: u64,
    /// Burned today, base units.
    pub burned: u64,
    /// Treasury inflow today, base units.
    pub treasury: u64,
    /// Emitted today, base units.
    pub emitted: u64,
    /// Validators active.
    pub validators: u32,
    /// Annualised income of one validator, fiat.
    pub validator_income_fiat: f64,
    /// Annualised cost of one validator, fiat.
    pub validator_cost_fiat: f64,
    /// Fee of an average transaction, fiat.
    pub fee_per_tx_fiat: f64,
    /// Mean block fullness against the 2x-target maximum, ppm.
    pub utilisation_ppm: u64,
}

/// A whole run and its verdict.
#[derive(Clone, Debug)]
pub struct Run {
    /// Scenario name.
    pub name: &'static str,
    /// Every day.
    pub days: Vec<Day>,
}

/// Stability criteria a parameter set must meet in every scenario before it
/// may become a governance proposal (Master Prompt 6 §6).
#[derive(Clone, Copy, Debug)]
pub struct Stability {
    /// Staking ratio never below this, ppm (one third: below it, stake that
    /// could halt the chain is cheap relative to supply).
    pub min_staking_ppm: u64,
    /// Validators never below this share of the initial set, ppm.
    pub min_validator_share_ppm: u64,
}

impl Stability {
    /// The criteria the reports apply.
    pub const DEFAULT: Self = Self { min_staking_ppm: 333_333, min_validator_share_ppm: 666_667 };
}

impl Run {
    /// Lowest staking ratio seen, ppm.
    pub fn min_staking_ppm(&self) -> u64 {
        self.days.iter().map(|d| d.staking_ppm).min().unwrap_or(0)
    }

    /// Fewest validators seen.
    pub fn min_validators(&self) -> u32 {
        self.days.iter().map(|d| d.validators).min().unwrap_or(0)
    }

    /// Highest average-transaction fee seen, fiat.
    pub fn max_fee_fiat(&self) -> f64 {
        self.days.iter().map(|d| d.fee_per_tx_fiat).fold(0.0, f64::max)
    }

    /// Supply change over the run, ppm of the start (negative = deflation).
    pub fn supply_change_ppm(&self) -> i64 {
        let (Some(first), Some(last)) = (self.days.first(), self.days.last()) else {
            return 0;
        };
        let change = (i128::from(last.supply) - i128::from(first.supply)) * 1_000_000
            / i128::from(first.supply.max(1));
        i64::try_from(change).unwrap_or(i64::MIN)
    }

    /// Whether every stability criterion held on every day, and no supply
    /// ever exceeded the hard cap.
    pub fn is_stable(&self, initial_validators: u32, s: Stability) -> bool {
        let floor = u64::from(initial_validators) * s.min_validator_share_ppm / 1_000_000;
        self.min_staking_ppm() >= s.min_staking_ppm
            && u64::from(self.min_validators()) >= floor
            && self.days.iter().all(|d| d.supply <= MAX_SUPPLY)
    }
}

struct State {
    supply: u64,
    staked: u64,
    base_fee: u64,
    validators: u32,
    price: f64,
    cost_factor: f64,
    spam_left: u64,
}

/// Runs `scenario` under `params` from `seed`.
pub fn run(params: &EconParams, scenario: &Scenario, seed: u64) -> Run {
    let mut rng = Rng::new(seed);
    let mut st = State {
        supply: params.genesis_supply,
        staked: (u128::from(params.genesis_supply) * u128::from(params.initial_staking_ppm) / 1_000_000) as u64,
        base_fee: params.fees.initial_base_fee,
        validators: params.validators,
        price: params.genesis_price,
        cost_factor: 1.0,
        spam_left: 0,
    };
    let mut days = Vec::with_capacity(scenario.days as usize);
    for day in 0..scenario.days {
        apply_step_shocks(&mut st, scenario, day);
        days.push(simulate_day(params, scenario, day, &mut st, &mut rng));
    }
    Run { name: scenario.name, days }
}

fn apply_step_shocks(st: &mut State, scenario: &Scenario, day: u64) {
    for shock in &scenario.shocks {
        match *shock {
            Shock::Price { day: d, factor } if d == day => st.price *= factor,
            Shock::Costs { day: d, factor } if d == day => st.cost_factor *= factor,
            Shock::StakeExit { day: d, ppm } if d == day => {
                st.staked -= (u128::from(st.staked) * u128::from(ppm) / 1_000_000) as u64;
            }
            Shock::Spam { day: d, budget, .. } if d == day => st.spam_left = budget,
            _ => {}
        }
    }
}

fn usage_factor(scenario: &Scenario, day: u64) -> f64 {
    scenario.shocks.iter().fold(1.0, |f, s| match *s {
        Shock::Usage { day: d, until, factor } if day >= d && day < until => f * factor,
        _ => f,
    })
}

fn spam_active(scenario: &Scenario, day: u64) -> bool {
    scenario
        .shocks
        .iter()
        .any(|s| matches!(*s, Shock::Spam { day: d, days, .. } if day >= d && day < d + days))
}

fn simulate_day(params: &EconParams, scenario: &Scenario, day: u64, st: &mut State, rng: &mut Rng) -> Day {
    let target = params.fees.target_block_bytes;
    let max_block = 2 * target;
    let (mut burned, mut treasury, mut tips, mut used) = (0u64, 0u64, 0u64, 0u64);
    let mut peak_base_fee = st.base_fee;
    let usage = usage_factor(scenario, day);
    let spam = spam_active(scenario, day);
    let scale = params.epoch_blocks / SAMPLED_BLOCKS;
    // The spammer's budget is real base units; each sampled block stands for
    // `scale` real ones, so it spends `scale` times what the sample shows.
    let mut spam_budget = st.spam_left / scale.max(1);
    for _ in 0..SAMPLED_BLOCKS {
        let fee_fiat = st.base_fee as f64 * params.avg_tx_bytes as f64 / UNIT as f64 * st.price;
        let elasticity = (WILLINGNESS_FIAT * rng.noise(0.5) / fee_fiat.max(1e-12)).min(1.0);
        let txs = scenario.base_tps * params.block_secs as f64 * usage * rng.noise(0.3) * elasticity;
        let mut bytes = ((txs * params.avg_tx_bytes as f64) as u64).min(max_block);
        if spam && spam_budget > 0 {
            let room = max_block - bytes;
            let cost = st.base_fee.saturating_mul(room);
            let paid = cost.min(spam_budget);
            spam_budget -= paid;
            bytes += paid / st.base_fee.max(1);
        }
        let base_paid = st.base_fee.saturating_mul(bytes);
        let tip = bytes / params.avg_tx_bytes.max(1); // one base unit per tx
        let s = split(base_paid, tip, params.fees.treasury_bps);
        burned += s.burned;
        treasury += s.treasury;
        tips += s.tip;
        used += bytes;
        st.base_fee = next_base_fee(
            st.base_fee,
            bytes,
            target,
            params.fees.change_denominator,
            params.fees.min_base_fee,
        );
        peak_base_fee = peak_base_fee.max(st.base_fee);
    }
    st.spam_left = spam_budget * scale;
    let (mut burned, mut treasury, mut tips) = (burned * scale, treasury * scale, tips * scale);
    // Fees come out of liquid balances. Demand beyond what holders can pay
    // is demand that did not happen; scale the day's fees down to the bound.
    let liquid = st.supply.saturating_sub(st.staked);
    let cap = u128::from(liquid) * u128::from(params.max_daily_fee_spend_ppm) / 1_000_000;
    let spent = u128::from(burned) + u128::from(treasury) + u128::from(tips);
    if spent > cap && spent > 0 {
        let shrink = |x: u64| (u128::from(x) * cap / spent) as u64;
        (burned, treasury, tips) = (shrink(burned), shrink(treasury), shrink(tips));
    }

    let emitted = (u128::from(st.supply) * u128::from(params.emission_ppm_per_year)
        / 1_000_000
        / u128::from(params.epochs_per_year())) as u64;
    let emitted = emitted.min(MAX_SUPPLY - st.supply.min(MAX_SUPPLY));
    st.supply = st.supply + emitted - burned.min(st.supply);

    // Delegation: stake follows yield against the hurdle.
    let yield_ppm = if st.staked == 0 {
        u64::MAX
    } else {
        (u128::from(emitted) * u128::from(params.epochs_per_year()) * 1_000_000 / u128::from(st.staked)) as u64
    };
    let liquid = st.supply.saturating_sub(st.staked);
    if yield_ppm > params.delegator_hurdle_ppm {
        st.staked += liquid / 200;
    } else {
        st.staked -= st.staked / 100;
    }
    st.staked += (u128::from(emitted) * 9 / 10) as u64; // most rewards are restaked
    st.staked = st.staked.min(st.supply);

    // Validator operators: commission on rewards plus every tip.
    let per_validator = (u128::from(emitted) * u128::from(params.operator_share_ppm()) / 1_000_000
        + u128::from(tips))
        / u128::from(st.validators.max(1));
    let income = per_validator as f64 * params.epochs_per_year() as f64 / UNIT as f64 * st.price;
    let cost = params.validator_cost_per_year * st.cost_factor;
    if income < cost && st.validators > MIN_VALIDATORS {
        // The least efficient operators leave, a few per day; they do not
        // all leave at once, which would overstate the shock.
        let leaving = ((cost - income) / cost * 3.0).ceil() as u32;
        st.validators = st.validators.saturating_sub(leaving).max(MIN_VALIDATORS);
    } else if income > cost * 1.5 && st.validators < 2 * params.validators {
        st.validators += 1;
    }

    Day {
        day,
        supply: st.supply,
        staking_ppm: (u128::from(st.staked) * 1_000_000 / u128::from(st.supply.max(1))) as u64,
        base_fee: st.base_fee,
        peak_base_fee,
        burned,
        treasury,
        emitted,
        validators: st.validators,
        validator_income_fiat: income,
        validator_cost_fiat: cost,
        fee_per_tx_fiat: st.base_fee as f64 * params.avg_tx_bytes as f64 / UNIT as f64 * st.price,
        utilisation_ppm: used * 1_000_000 / (max_block * SAMPLED_BLOCKS),
    }
}
