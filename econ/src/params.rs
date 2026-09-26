//! Parameters: the chain's fee configuration, plus the draft economics.

use maya_fee_market::FeeConfig;

/// Everything a run needs besides the scenario.
#[derive(Clone, Copy, Debug)]
pub struct EconParams {
    /// The chain's fee market configuration, used unmodified.
    pub fees: FeeConfig,
    /// Seconds per block (ADR-015: DAG-BFT; 2 s draft).
    pub block_secs: u64,
    /// Blocks per epoch.
    pub epoch_blocks: u64,
    /// Supply at genesis, base units.
    pub genesis_supply: u64,
    /// Annual emission as parts per million of *current* supply.
    pub emission_ppm_per_year: u64,
    /// Validators in the active set.
    pub validators: u32,
    /// A validator's all-in cost per year, in fiat units.
    pub validator_cost_per_year: f64,
    /// Average transaction size, bytes.
    pub avg_tx_bytes: u64,
    /// Staking ratio at genesis, parts per million.
    pub initial_staking_ppm: u64,
    /// Yield (ppm/year) below which delegators start to unstake.
    pub delegator_hurdle_ppm: u64,
    /// Token price at genesis, fiat. A scenario **input**, never a forecast.
    pub genesis_price: f64,
    /// Share of active stake that is operators' own bond, ppm.
    pub self_bond_ppm: u64,
    /// Validator commission on delegators' rewards, ppm.
    pub commission_ppm: u64,
    /// Most of the liquid supply users spend on fees in a day, ppm. Fees are
    /// paid from balances; without this bound a fixed-price model burns
    /// tokens nobody holds.
    pub max_daily_fee_spend_ppm: u64,
}

/// Base units per token.
pub const UNIT: u64 = 100_000_000;

impl EconParams {
    /// Draft mainnet parameters. Every number here is a modelling assumption
    /// for Master Prompt 20's genesis review, not a decision.
    pub const DRAFT: Self = Self {
        fees: FeeConfig::TESTING,
        block_secs: 2,
        epoch_blocks: 43_200, // one day at 2 s
        // 10M tokens: half of `maya_fee_market::MAX_SUPPLY` (21M), so the
        // hard cap binds within the horizons simulated only under extreme
        // emission, and the cap check is exercised rather than assumed.
        genesis_supply: 10_000_000 * UNIT,
        emission_ppm_per_year: 30_000, // 3%
        validators: 100,
        validator_cost_per_year: 24_000.0,
        avg_tx_bytes: 5_200, // one ML-DSA-65 signature + key hash + payload
        initial_staking_ppm: 500_000,
        delegator_hurdle_ppm: 40_000,
        genesis_price: 1.0,
        self_bond_ppm: 100_000,
        commission_ppm: 100_000,
        max_daily_fee_spend_ppm: 10_000,
    };

    /// Blocks per year.
    pub const fn blocks_per_year(&self) -> u64 {
        365 * 24 * 3600 / self.block_secs
    }

    /// Epochs per year.
    pub const fn epochs_per_year(&self) -> u64 {
        self.blocks_per_year() / self.epoch_blocks
    }

    /// Share of staking rewards that reaches operators, ppm: all of the
    /// self-bond's rewards plus commission on the delegated remainder.
    pub const fn operator_share_ppm(&self) -> u64 {
        self.self_bond_ppm + (1_000_000 - self.self_bond_ppm) * self.commission_ppm / 1_000_000
    }

    /// The token price at which `validators` operators break even on
    /// emission alone at genesis supply (fees ignored, so an upper bound on
    /// the price needed).
    #[allow(clippy::cast_precision_loss)]
    pub fn break_even_price(&self) -> f64 {
        let emission_tokens =
            self.genesis_supply as f64 * self.emission_ppm_per_year as f64 / 1e6 / UNIT as f64;
        let operator_tokens = emission_tokens * self.operator_share_ppm() as f64 / 1e6;
        f64::from(self.validators) * self.validator_cost_per_year / operator_tokens
    }
}
