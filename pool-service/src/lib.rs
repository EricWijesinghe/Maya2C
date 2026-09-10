//! # Maya2C pool service
//!
//! A mining pool: a Stratum V2 listener that validates shares against the
//! chain's own proof-of-work rule, an append-only ledger that records what each
//! share was worth, a PPLNS engine that turns those records into credits, and a
//! payout engine that settles them as ordinary signed transfers.
//!
//! ## Layout
//!
//! - [`config`] — policy, and the checks that run before anything binds
//! - [`model`] — the pool's own view of miners, shares, and payouts
//! - [`channel`] — open channels, disjoint nonce ranges, duplicate memory
//! - [`vardiff`] — per-channel difficulty, the pool's load governor
//! - [`job`] — work templates and the ids miners submit against
//! - [`share`] — verification, through the chain's rule rather than a copy
//! - [`ledger`] — the append-only share ledger, behind a trait
//! - [`pplns`] — window accounting and the split
//! - [`treasury`] — the key, the nonce, and the spend caps
//! - [`payout`] — the batch state machine that settles credits on chain
//! - [`telemetry`] — measured hash rate, efficiency, and reported rig health
//! - [`metrics`] — Prometheus instrumentation
//! - [`node`] — the JSON-RPC client the pool drives the chain through
//! - [`server`] — JSON API, dashboard routes, and the WebSocket
//! - [`ui`] — Leptos server-rendered views
//! - [`daemon`] — the Stratum listener and the loops that tie it together
//!
//! ## Three decisions worth knowing about
//!
//! **There is no block reward to split.** `docs/stratum-v2.md` §5 establishes
//! it: `apply_block_checked` credits no subsidy and fees burn to `FEE_SINK`.
//! Share credits therefore accrue in *weight*, and coin enters only at
//! settlement, from a treasury the operator funds. [`config::PoolConfig`]
//! refuses to start without that decision having been made explicitly.
//!
//! **Share validation is the scaling wall, not the connection count.** A share
//! costs 25.4 ms of Argon2id below the DAG fork. [`vardiff`] is what keeps the
//! aggregate rate survivable, and backpressure sheds load by raising targets —
//! never by dropping shares a miner already did the work for.
//!
//! **Payouts are idempotent through the treasury nonce.** A batch reserves a
//! nonce and writes itself down *before* it is signed, and a resumed batch
//! rebroadcasts the same bytes rather than signing new ones. A crash at any
//! point produces one transaction or none, never two.

#![warn(missing_docs)]

pub mod api;
pub mod channel;
pub mod config;
pub mod daemon;
pub mod error;
pub mod job;
pub mod ledger;
pub mod metrics;
pub mod model;
pub mod node;
pub mod payout;
pub mod pplns;
pub mod server;
pub mod session;
pub mod share;
pub mod state;
pub mod telemetry;
pub mod treasury;
pub mod ui;
pub mod validator;
pub mod vardiff;

pub use config::PoolConfig;
pub use error::{PoolError, Result};
pub use model::{
    MinerAccount, PayoutBatch, PayoutEntry, PayoutState, RejectReason, ShareOutcome, ShareRecord,
    WorkerKey, WorkerStats,
};
