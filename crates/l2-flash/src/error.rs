//! Errors produced by the Flash layer.

use thiserror::Error;

/// A channel or routing failure.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FlashError {
    /// A state update did not advance the sequence number.
    ///
    /// Sequence numbers must increase strictly. Accepting an equal or lower one
    /// would let a counterparty overwrite an agreed state with a stale set of
    /// balances.
    #[error("state sequence must advance: current {current}, proposed {proposed}")]
    StaleSequence {
        /// Sequence the channel is currently at.
        current: u64,
        /// Sequence the rejected update carried.
        proposed: u64,
    },

    /// A state update did not conserve the channel's capacity.
    #[error("state does not conserve capacity: expected {expected}, state totals {actual}")]
    CapacityMismatch {
        /// The channel's funded capacity.
        expected: u64,
        /// What the proposed state accounts for.
        actual: u64,
    },

    /// A party tried to send more than its side holds.
    #[error("insufficient channel balance: need {required}, have {available}")]
    InsufficientBalance {
        /// Amount the transfer needs.
        required: u64,
        /// Amount available on that side.
        available: u64,
    },

    /// A signature did not verify against the expected participant.
    #[error("channel state signature is invalid for {party}")]
    InvalidSignature {
        /// Which side failed: `"a"` or `"b"`.
        party: &'static str,
    },

    /// A state was used before both parties signed it.
    #[error("state {seq} is not fully signed")]
    NotFullySigned {
        /// Sequence of the incomplete state.
        seq: u64,
    },

    /// An HTLC preimage did not match its hash lock.
    #[error("preimage does not match HTLC {id}'s hash lock")]
    PreimageMismatch {
        /// The HTLC that rejected the preimage.
        id: u64,
    },

    /// An HTLC was referenced but does not exist in the state.
    #[error("no HTLC with id {id} in this channel state")]
    UnknownHtlc {
        /// Requested id.
        id: u64,
    },

    /// An HTLC was fulfilled after it had already expired.
    #[error("HTLC {id} expired at height {expiry}, current height {height}")]
    HtlcExpired {
        /// The expired HTLC.
        id: u64,
        /// Height at which it expired.
        expiry: u64,
        /// Current height.
        height: u64,
    },

    /// An HTLC refund was attempted before expiry.
    #[error("HTLC {id} cannot be refunded before height {expiry}, current height {height}")]
    HtlcNotExpired {
        /// The HTLC.
        id: u64,
        /// Expiry height.
        expiry: u64,
        /// Current height.
        height: u64,
    },

    /// Arithmetic on channel balances overflowed.
    #[error("channel balance arithmetic overflowed")]
    BalanceOverflow,

    /// A settlement was attempted while HTLCs were still in flight.
    ///
    /// The on-chain closure format carries only final balances, so a state with
    /// pending HTLCs has no faithful on-chain representation — settling one
    /// would silently discard the in-flight value.
    #[error("cannot settle with {pending} HTLC(s) still pending")]
    HtlcsPending {
        /// How many remain unresolved.
        pending: usize,
    },

    /// No route with sufficient capacity exists between two nodes.
    #[error("no route from {from} to {to} for {amount} units")]
    NoRoute {
        /// Hex-encoded source.
        from: String,
        /// Hex-encoded destination.
        to: String,
        /// Amount that could not be routed.
        amount: u64,
    },

    /// A route exceeded the permitted number of hops.
    #[error("route of {hops} hops exceeds the maximum {max}")]
    RouteTooLong {
        /// Hops the candidate route required.
        hops: usize,
        /// Configured ceiling.
        max: usize,
    },

    /// A channel referenced by a route or update is unknown.
    #[error("unknown channel {0}")]
    UnknownChannel(String),
}

/// Convenience alias.
pub type Result<T> = core::result::Result<T, FlashError>;
