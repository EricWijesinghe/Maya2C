//! JSON-RPC interface.
//!
//! | Method | Params | Returns |
//! |---|---|---|
//! | `get_balance` | `[address_hex]` | [`types::AccountInfo`] |
//! | `send_raw_transaction` | `[tx_hex]` | [`types::SubmitTransactionResult`] |
//! | `get_block_by_height` | `[height]` | [`types::BlockInfo`] |
//! | `get_mining_candidate` | `[]` | [`types::MiningCandidate`] |
//! | `submit_block` | `[block_hex]` | [`types::SubmitBlockResult`] |
//! | `get_supply` | `[]` | [`market::SupplyReport`] |
//! | `threat_indicators` | `[]` | `Vec<`[`types::ThreatIndicatorInfo`]`>` |
//! | `threat_peer_addresses` | `[]` | `Vec<`[`types::PeerAddressInfo`]`>` — only with [`RpcContext::with_peers`] |
//!
//! `submit_block` is the counterpart to `get_mining_candidate`: a candidate a
//! miner can fetch but never return is not a usable interface, and the
//! end-to-end tests exercise the full fetch-mine-submit round trip.

//!
//! JSON-RPC is not the whole HTTP surface. Listing aggregators fetch supply with
//! an unauthenticated `GET` and parse the body as a number; they do not speak
//! JSON-RPC. Those endpoints live in [`market_http`] on their own port.

pub mod bootstrap;
pub mod limit;
pub mod market;
pub mod market_http;
pub mod server;
pub mod types;

pub use market::{MarketFeed, MarketQuote, SupplyReport};
pub use market_http::{MarketServer, MarketState, serve as serve_market};
pub use server::{RpcContext, RpcServer, build_module, serve};
pub use types::{
    AccountInfo, BlockInfo, HeaderInfo, HtlcLockInfo, IotDeviceInfo, MiningCandidate, OutputInfo,
    PeerAddressInfo, SubmitBlockResult, SubmitTransactionResult, SuiteKeyInfo, ThreatIndicatorInfo,
    TransactionInfo,
};
