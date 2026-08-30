//! # Maya Flash
//!
//! A state-channel network for micro-payments over the Maya2C L1.
//!
//! Two parties lock capacity into a channel on chain, then exchange
//! bidirectionally signed states off chain at no per-payment cost. Only the
//! opening and the final settlement touch the base chain, so a channel carrying
//! ten thousand payments still costs two on-chain transactions.
//!
//! ## Modules
//!
//! - [`channel`] — bidirectionally signed state, revocation, settlement
//! - [`htlc`] — hash time-locked contracts, the primitive behind atomic
//!   multi-hop payments
//! - [`routing`] — the directed capacity graph and its pathfinder
//! - [`payment`] — assembling an HTLC chain across a route
//!
//! ## Trust model
//!
//! A channel participant never has to trust its counterparty, only the chain.
//! Every state is signed by both sides; advancing revokes the previous one. If a
//! counterparty publishes a revoked state, the revocation secret proves the
//! fraud and forfeits their entire channel balance. The security of the whole
//! construction rests on a defrauded party being able to get that proof on chain
//! inside the dispute window — which is why the window is a channel parameter
//! rather than a constant.
//!
//! ## What this layer does not do
//!
//! There is no gossip protocol here: [`routing::ChannelGraph`] is a local view a
//! caller populates. Real networks discover topology over the wire, and a router
//! is only as good as the freshness of the capacities it was told about.

#![warn(missing_docs)]

pub mod channel;
pub mod error;
pub mod htlc;
pub mod payment;
pub mod routing;

pub use channel::{Channel, ChannelState, Party, SignedState};
pub use error::{FlashError, Result};
pub use htlc::{Direction, HashLock, Htlc, Preimage, hash_lock};
pub use payment::{Payment, PaymentPlan};
pub use routing::{ChannelGraph, Edge, FeePolicy, Hop, NodeId, Route};
