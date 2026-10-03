//! `l1-wallet` as a library.
//!
//! The crate began as a binary and its two modules stayed private to it. The
//! pool service needs both — [`keystore`] to unlock the treasury key, [`client`]
//! to reach the node's JSON-RPC — and the alternative was a second
//! implementation of the keystore format living in another crate.
//!
//! That alternative is worse than it sounds. A keystore is often the only copy
//! of a key: two readers of one format drift, and the drift surfaces as a file
//! that one tool wrote and the other cannot open. There is exactly one reader
//! of this format, and it is here.
//!
//! The binary in `main.rs` consumes this library rather than re-declaring the
//! modules, so there is one compilation of each and one definition of each.

#![warn(missing_docs)]

pub mod client;
pub mod keystore;
pub mod staking;
