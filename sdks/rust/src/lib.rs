//! Typed Rust client for the Maya2C API gateway (REST `/v1`).
//!
//! The gateway holds no keys and signs nothing. Transactions are signed
//! client-side (`l1-wallet`, `maya-sdk-wasm`) and handed here as hex.
//!
//! ```no_run
//! # fn main() -> Result<(), maya2c_sdk::Error> {
//! let client = maya2c_sdk::Client::new("https://api.example.invalid")?;
//! let balance = client.balance("maya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxsgd736f")?;
//! println!("{} units, next nonce {}", balance.balance, balance.nonce);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

pub mod address;
mod client;
mod error;
mod types;

pub use address::{Network, decode_address, encode_address, to_hex};
pub use client::{Client, DEFAULT_TIMEOUT};
pub use error::{ApiError, Error};
pub use types::{Balance, ChainInfo, FeeInfo, Health, SealedAccepted, Supply};
