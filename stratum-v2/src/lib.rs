//! Stratum V2 framing and mining messages for Maya2C.
//!
//! ## What this is, and what it deliberately is not
//!
//! This is Stratum V2's *architecture* — binary framing, a channel abstraction,
//! per-channel targets, batched share acknowledgement, and the separation of
//! template choice from pool operation — carrying a mining sub-protocol shaped
//! around Maya2C's header.
//!
//! It is **not** wire-compatible with stock Stratum V2 clients, and it does not
//! pretend to be. SV2's mining messages assume a Bitcoin header: they carry a
//! `merkle_root`, a compact `nbits`, a rollable `version`, and a coinbase to
//! hold an extranonce. Maya2C's 144-byte header (`src/core/block.rs`) keeps
//! only the root, as `tx_root`. No SRI or Braiins client could mine this chain even against a
//! byte-perfect implementation of the specification — it would need an
//! ArgonBlake hasher and a Maya2C header builder, at which point it is a
//! different client.
//!
//! Given that, announcing compatibility would have been the harmful choice. So
//! Maya2C's messages live under their own extension id
//! ([`frame::MAYA_EXTENSION_TYPE`]) and a stock client's frames are refused with
//! [`error::Sv2Error::UnknownExtension`] rather than misparsed. Message
//! *numbering* still tracks the specification slot for slot, so the
//! correspondence stays legible; [`messages::mining`] tabulates every field that
//! had to change and why.
//!
//! ## No chain dependency
//!
//! Nothing here depends on `custom-l1-node`. Every byte this crate reads was
//! chosen by a stranger, so it is built to be fuzzed and reviewed in isolation,
//! the way `ledger-math` is built to be model-checked in isolation. Protocol
//! values meet chain types one layer up, in the pool daemon.
//!
//! ## Decoding discipline
//!
//! Copied from `src/core/codec.rs` rather than reinvented:
//!
//! - every read is range-checked, so a truncated frame is an error and never a
//!   panic — a pool that aborts on a malformed frame is a pool that one
//!   connection can take down;
//! - a declared payload length above [`frame::MAX_PAYLOAD_LEN`] is refused
//!   before anything is reserved, because with 50,000 connections a length
//!   field is an allocation primitive;
//! - [`codec::Reader::finish`] rejects trailing bytes, so one message cannot
//!   have two encodings. For a share submission that matters concretely: two
//!   encodings would be two share identities for one piece of work, and the
//!   duplicate check keys on decoded fields.

pub mod codec;
pub mod error;
pub mod frame;
pub mod messages;

pub use error::{Result, Sv2Error};
pub use frame::{Frame, FrameHeader, MAX_PAYLOAD_LEN, MAYA_EXTENSION_TYPE};
pub use messages::{Message, Protocol, msg_type};
