//! # `Maya2C` Explorer
//!
//! A web explorer: an Axum server rendering Leptos views over an index of chain
//! data, kept current by a background indexer.
//!
//! ## Layout
//!
//! - [`indexer`] — pulls blocks from the node and writes them to a store
//! - [`store`] — the [`store::BlockStore`] trait, with PostgreSQL and in-memory
//!   implementations
//! - [`model`] — the explorer's own denormalised view of chain data
//! - [`ui`] — Leptos server-rendered components and pages
//! - [`server`] — routes, JSON API, and WebSocket endpoints
//!
//! ## Two decisions worth knowing about
//!
//! **Rendering is server-side only.** Leptos signals drive the render, but no
//! client WASM bundle rehydrates them — a hydrating build needs `cargo-leptos`
//! and `wasm-bindgen`. Liveness comes from the WebSocket endpoints instead.
//!
//! **SQL is issued at runtime, not checked at compile time.** `sqlx::query!`
//! validates SQL against a live database while compiling, which would make this
//! crate unbuildable without a database to hand. The cost is real: a column
//! typo surfaces at runtime, so the schema in
//! [`store::postgres::SCHEMA`] and the queries beside it have to be kept in
//! step deliberately.

#![warn(missing_docs)]

pub mod error;
pub mod indexer;
pub mod model;
pub mod plain;
pub mod server;
pub mod store;
pub mod ui;

pub use error::{ExplorerError, Result};
pub use indexer::{IndexEvent, Indexer, RpcSource};
pub use model::{HashratePoint, IndexedBlock, IndexedTx, NetworkStats};
pub use server::{AppState, ExplorerServer, serve};
pub use store::BlockStore;
pub use store::memory::MemoryStore;
pub use store::postgres::PostgresStore;
