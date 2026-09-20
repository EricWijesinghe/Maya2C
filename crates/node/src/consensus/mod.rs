//! Consensus: difficulty retargeting, cumulative work, fork choice, and mining.

pub mod chain;
pub mod difficulty;
pub mod miner;
pub mod uint;

pub use chain::{BlockId, BlockRecord, Chain, ChainConfig, InsertOutcome};
pub use difficulty::{
    EXPECTED_TIMESPAN, MAX_ADJUSTMENT_FACTOR, RETARGET_INTERVAL, TARGET_BLOCK_TIME,
    cumulative_work, default_pow_limit, is_retarget_height, retarget, unlimited_pow_limit,
    work_from_target,
};
pub use miner::{MiningResult, PowMode, mine_header, mine_header_with, suggested_threads};
pub use uint::U256;
