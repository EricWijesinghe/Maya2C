//! Named scenarios: Master Prompt 18 §1 and the macro shocks of Master
//! Prompt 6 §6. Each is a set of paths over the simulated days.

/// What happens to one input over time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shock {
    /// Nothing.
    None,
    /// Demand multiplied by `factor` from `day` on (until `until`).
    Usage {
        /// First day.
        day: u64,
        /// Last day (exclusive).
        until: u64,
        /// Multiplier on transaction demand.
        factor: f64,
    },
    /// Token price multiplied by `factor` from `day` on.
    Price {
        /// Day of the move.
        day: u64,
        /// Multiplier.
        factor: f64,
    },
    /// Fiat-denominated validator costs multiplied by `factor` from `day` on
    /// (a local-currency devaluation against the currency costs are paid in).
    Costs {
        /// Day of the move.
        day: u64,
        /// Multiplier.
        factor: f64,
    },
    /// A share (ppm) of all stake leaves at once on `day`.
    StakeExit {
        /// Day of the exit.
        day: u64,
        /// Share of stake, parts per million.
        ppm: u64,
    },
    /// A spammer fills every block to the maximum for `days`, paying the base
    /// fee, until `budget` base units are spent.
    Spam {
        /// First day.
        day: u64,
        /// Duration.
        days: u64,
        /// Spend limit, base units.
        budget: u64,
    },
}

/// A named scenario.
#[derive(Clone, Debug)]
pub struct Scenario {
    /// Name used in reports.
    pub name: &'static str,
    /// Simulated days.
    pub days: u64,
    /// Baseline demand, transactions per second.
    pub base_tps: f64,
    /// Shocks, applied in order.
    pub shocks: Vec<Shock>,
}

const TWO_YEARS: u64 = 730;
const BASE_TPS: f64 = 20.0;

/// Every scenario the reports run, in report order.
pub fn all() -> Vec<Scenario> {
    let s = |name, shocks| Scenario {
        name,
        days: TWO_YEARS,
        base_tps: BASE_TPS,
        shocks,
    };
    vec![
        s("baseline", vec![]),
        s(
            "low usage for 2 years (0.05x)",
            vec![Shock::Usage {
                day: 0,
                until: TWO_YEARS,
                factor: 0.05,
            }],
        ),
        s(
            "sudden 100x usage from day 90",
            vec![Shock::Usage {
                day: 90,
                until: TWO_YEARS,
                factor: 100.0,
            }],
        ),
        s(
            "80% price drop on day 180",
            vec![Shock::Price {
                day: 180,
                factor: 0.2,
            }],
        ),
        s(
            "large holder exits staking (30% of stake, day 120)",
            vec![Shock::StakeExit {
                day: 120,
                ppm: 300_000,
            }],
        ),
        s(
            "fee-market spam campaign (days 60-67)",
            vec![Shock::Spam {
                day: 60,
                days: 7,
                budget: 2_000_000 * crate::params::UNIT,
            }],
        ),
        // Master Prompt 6 §6 macro shocks.
        s(
            "MP6: 30% currency devaluation (costs x1.43)",
            vec![Shock::Costs {
                day: 100,
                factor: 1.0 / 0.7,
            }],
        ),
        s(
            "MP6: 50% market crash",
            vec![Shock::Price {
                day: 100,
                factor: 0.5,
            }],
        ),
        s(
            "MP6: liquidity shock (demand -90% 60 days, 20% unstake)",
            vec![
                Shock::Usage {
                    day: 100,
                    until: 160,
                    factor: 0.1,
                },
                Shock::StakeExit {
                    day: 100,
                    ppm: 200_000,
                },
            ],
        ),
    ]
}
