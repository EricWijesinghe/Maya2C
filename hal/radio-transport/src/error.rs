//! What can go wrong on a link nobody authenticates.
//!
//! Every variant names what was refused and why. A radio link has no operator
//! watching it and often no return path, so the error is frequently the only
//! record that anything happened — "malformed frame" would leave a field
//! engineer with a radio, a log, and nothing to act on.

use thiserror::Error;

/// The result of anything in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// A refusal.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// Bytes that are not a well-formed frame, symbol or bundle.
    #[error("malformed: {0}")]
    Malformed(String),

    /// A frame's checksum does not match its contents.
    ///
    /// Its own variant rather than part of [`Error::Malformed`]: a checksum
    /// failure is the ordinary weather of a radio link, and an operator
    /// counting them is measuring link quality rather than hunting a bug.
    #[error("checksum failed")]
    Checksum,

    /// Something exceeds a bound this crate puts on it.
    #[error("{what} exceeds its bound: {found} > {limit}")]
    Oversized {
        /// What was being read or built.
        what: &'static str,
        /// How much arrived.
        found: usize,
        /// The most this crate will handle.
        limit: usize,
    },

    /// The band's hourly airtime budget is spent.
    ///
    /// Carries how long to wait, so a caller sleeps exactly long enough rather
    /// than polling a radio it is not allowed to key.
    #[error(
        "duty cycle exhausted: this frame needs {airtime_micros}us, \
         {remaining_micros}us remain, and the window frees up in {wait_micros}us"
    )]
    DutyCycleExhausted {
        /// Airtime the refused frame would have taken.
        airtime_micros: u64,
        /// Airtime still available in the window.
        remaining_micros: u64,
        /// How long until the oldest transmission leaves the window.
        wait_micros: u64,
    },

    /// One transmission is longer than the band allows, whatever the budget.
    #[error("dwell exceeded: {airtime_micros}us in one transmission, limit {limit_micros}us")]
    DwellExceeded {
        /// Airtime the refused frame would have taken.
        airtime_micros: u64,
        /// The band's per-transmission cap.
        limit_micros: u64,
    },

    /// A bundle has been relayed as far as it may go.
    ///
    /// Not a failure of the bundle — it is how a mesh stops a message
    /// circulating forever, and a relay that treated it as an error would log
    /// one for every message it correctly declined to forward.
    #[error("hop budget spent")]
    HopsExhausted,

    /// A bundle is past its time to live.
    #[error("bundle expired at {expiry}, now {now}")]
    Expired {
        /// When it stopped being worth relaying.
        expiry: u64,
        /// The caller's clock.
        now: u64,
    },

    /// The store has no room and nothing worth evicting.
    #[error("relay store is full: {held} bundles")]
    StoreFull {
        /// How many bundles are held.
        held: usize,
    },
}
