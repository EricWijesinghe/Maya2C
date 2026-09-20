//! Block features for the neural base-fee gain.
//!
//! The network and the fee rule live in `fee-market` (`src/model/`,
//! `src/rule.rs`), which nothing in this crate links: `tests/fee_market_tests.rs`
//! fails if any file under `src/` names that crate, and it stays true here.
//! This module does the half that needs chain types — reading a block — and
//! hands back six integers. The fee market's research branch reads them; no
//! consensus path calls either side. `tests/neural_fee_tests.rs` pins the
//! constants the two halves share.
//!
//! # What the brief asked for, and what a validator can compute
//!
//! Every feature is a function of the block and nothing else: no clock, no
//! allocator, no node-local measurement. A fee that depended on something one
//! node measured differently from another would split the chain, for the reason
//! invariant 28 gives about the breaker.
//!
//! | Brief | Here |
//! |---|---|
//! | tx size | mean transaction size and block fullness |
//! | state access overlap | accounts named by more than one transaction — the static access set, because execution builds one merged overlay per block and tracking per-transaction reads would allocate in the hot loop (execution directive 3) |
//! | memory allocation depth | **not computable deterministically**; replaced by declared contract fuel per byte |
//! | inter-shard dependencies | transactions whose accounts span more than one `blockgraph` shard. Hypothetical: no shards execute |
//! | historical DAG vertex metrics | the DAG here is the proof-of-work dataset, not a transaction graph; the features describe the parent block |

pub mod features;

pub use features::{
    BlockFeatures, FEATURE_COUNT, FEATURE_FRAC_BITS, FEATURE_LIMIT, FEATURE_ONE,
    FUEL_PER_BYTE_SCALE, MEAN_TX_SCALE_BYTES, extract,
};
