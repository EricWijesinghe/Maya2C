//! **SIM** — the chain's economic model as code (Master Prompts 6 §6 and 18).
//!
//! An agent-based simulator of mainnet-core economics: emission, staking
//! yield, fee burn, treasury inflow, validator costs and delegation, driven
//! through scenarios and macro shocks. Plus the analyses Master Prompt 18
//! asks for: cost-of-attack tables ([`attack`]), stake concentration
//! ([`concentration`]), sandwich extraction against the continuous curve and
//! the batch auction ([`mev`]), and a fee estimator measured against
//! simulated traffic ([`estimator`]).
//!
//! # What this is not
//!
//! Not a price model and not advice. Token prices are **inputs** to each
//! scenario, never predictions; the simulator answers "if the price did X,
//! what would validators earn and would the staking ratio hold", not "what
//! will the price do". No module here guarantees price stability, and no
//! output is legal or financial advice (`docs/LEGAL_NOTICE.md`). Floats appear
//! only in prices, ratios and costs; token amounts are integers, and nothing
//! here is on a consensus path.
//!
//! # No drift from the chain
//!
//! The base-fee step and the burn/treasury split are
//! `maya_fee_market::next_base_fee` and `maya_fee_market::split`, called with
//! `maya_fee_market::FeeConfig::TESTING` — the active configuration the
//! chain's fee market ships. The AMM is `maya_dex`. The emission schedule is
//! a draft parameter set ([`params::EconParams::DRAFT`]) because the chain has
//! no final genesis emission yet (Master Prompt 20 freezes it).

#![warn(missing_docs)]

pub mod attack;
pub mod concentration;
pub mod estimator;
pub mod mev;
pub mod model;
pub mod params;
pub mod rng;
pub mod scenario;

/// Printed at the top of every report and CLI run (Standing Order 3).
pub const SIM_BANNER: &str = "[SIM] maya-econ: agent-based model; prices are scenario inputs, not predictions; not financial or legal advice";
