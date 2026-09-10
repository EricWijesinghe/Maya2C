//! The oracle subsystem: a randomness beacon and a multi-signed price feed.
//!
//! ## The chain's first trusted party
//!
//! Everything else in this node is trustless in the strict sense — proof of
//! work decides ordering, signatures decide authority over funds, and a
//! validator who lies is caught by arithmetic anyone can redo. None of that is
//! available here.
//!
//! An external price is a fact about the world, and no amount of cryptography
//! makes a chain able to observe one. So the oracle introduces a named set of
//! authorities and believes what a quorum of them agrees on. That is a genuine
//! reduction in the chain's security model, and every design decision below
//! exists to bound it rather than to disguise it:
//!
//! - **A quorum, not a party.** A feed moves only when
//!   [`registry::OracleRegistry::quorum`] distinct authorities sign the same
//!   round.
//! - **The median, not the mean.** One compromised authority moves a mean
//!   without limit. It moves a median by nothing at all until it holds a
//!   majority of the quorum.
//! - **Rotation from the first block.** A set that cannot be changed is a set
//!   that is permanent, including after a key leaks.
//! - **Freshness is not optional.** A contract reading a feed states how stale
//!   it will tolerate, and gets nothing if the feed is older. See
//!   [`crate::state::oracle_exec`].
//!
//! ## The beacon is a different kind of thing
//!
//! Randomness is *not* a fact about the world, so the beacon needs no trust in
//! what an authority observes — only that it holds a key. A VRF output is
//! unique per (key, input), so an authority cannot choose its value; it can
//! only refuse to produce it, and refusal is handled by a fallback that keeps
//! the chain moving.
//!
//! What an authority and a miner *together* can still do is bounded and stated
//! in [`beacon`].

pub mod beacon;
pub mod feed;
pub mod registry;

pub use beacon::{BeaconState, beacon_alpha, fallback_beacon};
pub use feed::{FEED_NAME_LEN, FEED_SCALE, FeedRecord, derive_feed_id, median, observation_bytes};
pub use registry::{MAX_AUTHORITIES, OracleAuthority, OracleRegistry};

/// Key prefix shared by every record the oracle subsystem owns.
///
/// One byte apart from the trading subsystem's `d:`, and distinct from every
/// pre-existing prefix (`acct:`, `chan:`, `code:`, `cstate:`, `null:`, `shld:`,
/// `undo:`). One scan collects the whole subsystem, which is what lets it fold
/// into the state root as a single layer and journal into the undo record as a
/// single section.
pub(crate) const ORACLE_PREFIX: &[u8] = b"o:";

/// Storage key for the authority set.
///
/// One blob rather than one key per authority. The set is never read in part —
/// verifying a quorum needs all of it, and so does selecting a beacon
/// proposer — and at [`MAX_AUTHORITIES`] it is under a kilobyte.
pub(crate) const REGISTRY_KEY: &[u8] = b"o:authorities";

/// Storage key for the beacon accumulator.
pub(crate) const BEACON_KEY: &[u8] = b"o:beacon";

/// Prefix under which each feed's record lives.
pub(crate) const FEED_PREFIX: &[u8] = b"o:feed:";

/// Storage key for one feed.
#[must_use]
pub fn feed_key(feed_id: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(FEED_PREFIX.len() + 32);
    key.extend_from_slice(FEED_PREFIX);
    key.extend_from_slice(feed_id);
    key
}
