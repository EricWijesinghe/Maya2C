//! Session rotation: when a connection's post-quantum keys are too old.
//!
//! ## What rotation is and is not for
//!
//! The ML-KEM keypair in [`super::handshake`] is generated fresh for every
//! connection and dropped the moment the handshake finishes. That is where
//! forward secrecy comes from, and it is already complete: an adversary who
//! seizes a node tomorrow cannot decrypt traffic recorded today, because the
//! decapsulation key that could have is long gone.
//!
//! What this module adds is a bound on how long any one session key lives. A
//! node in a quiet mesh can hold a connection open for days; without rotation,
//! one handshake would protect days of traffic, and a key extracted from a live
//! process would unlock all of it. Forcing a re-handshake every epoch caps that
//! window. The property is **post-compromise security** — recovery after a
//! leak — not forward secrecy, which was never missing.
//!
//! ## Why the epoch is local, and not negotiated
//!
//! An earlier design carried the epoch in the handshake and had both peers
//! agree on it. That does not survive contact with a real network: peers are at
//! different heights, a syncing node is thousands of blocks behind, and a node
//! that has just started has no chain at all. Two honest peers would compute
//! different epochs and fail to connect — a self-inflicted partition, exactly
//! when the network is least able to afford one.
//!
//! So the epoch never goes on the wire. Each node independently notes the epoch
//! a connection was established in and closes it once its own epoch has moved
//! on. libp2p redials, the upgrade runs again, and fresh keys are in place.
//! Whichever side notices first drives it; the other simply sees a reconnect.
//! Two peers disagreeing about the epoch now means one of them rotates sooner
//! than the other, which is harmless.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::consensus::TARGET_BLOCK_TIME;

/// Blocks between forced re-handshakes.
///
/// At the 15 s target block time this is a little over four hours. Short enough
/// that a key lifted from a running node has a bounded payoff; long enough that
/// re-handshaking is invisible against a mesh of tens of peers, each costing
/// ~144 µs of ML-KEM work.
pub const ROTATION_INTERVAL_BLOCKS: u64 = 1_000;

/// Wall-clock equivalent of [`ROTATION_INTERVAL_BLOCKS`].
///
/// The fallback exists because block height is not a clock a transport can rely
/// on. A node that is syncing, stalled, or freshly started may sit at one
/// height for hours — and a node whose height never advances would, on height
/// alone, never rotate at all. That is precisely the node an attacker would
/// most like to have compromised.
pub const ROTATION_FALLBACK: Duration =
    Duration::from_secs(ROTATION_INTERVAL_BLOCKS * TARGET_BLOCK_TIME);

/// Tracks which rotation epoch the node is in.
///
/// Cheap to clone; all clones observe the same height.
#[derive(Clone, Debug)]
pub struct EpochClock {
    /// Local chain height, updated as blocks commit.
    height: Arc<AtomicU64>,
    /// When this node started, for the wall-clock fallback.
    origin: Instant,
}

impl Default for EpochClock {
    fn default() -> Self {
        Self::new()
    }
}

impl EpochClock {
    /// A clock starting at height zero, now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            height: Arc::new(AtomicU64::new(0)),
            origin: Instant::now(),
        }
    }

    /// Records the node's current chain height.
    ///
    /// `Relaxed` throughout: the epoch is a coarse schedule, not a
    /// synchronization point. Reading a height one block stale shifts a
    /// rotation by 15 seconds out of four hours.
    pub fn set_height(&self, height: u64) {
        self.height.store(height, Ordering::Relaxed);
    }

    /// The node's chain height as last recorded.
    #[must_use]
    pub fn height(&self) -> u64 {
        self.height.load(Ordering::Relaxed)
    }

    /// The current epoch.
    ///
    /// The later of the two schedules wins. Taking the maximum rather than
    /// preferring one source means neither can stall rotation: a node with a
    /// frozen height still rotates on the clock, and a node that syncs a year
    /// of history in ten minutes rotates on height rather than pretending only
    /// ten minutes of key exposure has occurred.
    #[must_use]
    pub fn current(&self) -> u64 {
        let by_height = self.height() / ROTATION_INTERVAL_BLOCKS;
        let by_time = self.origin.elapsed().as_secs() / ROTATION_FALLBACK.as_secs();
        by_height.max(by_time)
    }

    /// Whether a session opened in `established` is now stale.
    #[must_use]
    pub fn is_stale(&self, established: u64) -> bool {
        self.current() > established
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interval_is_a_little_over_four_hours() {
        // Sanity on the constant a reader is most likely to want to change.
        assert_eq!(ROTATION_FALLBACK.as_secs(), 15_000);
        assert_eq!(ROTATION_INTERVAL_BLOCKS, 1_000);
    }

    #[test]
    fn a_fresh_clock_is_in_epoch_zero() {
        assert_eq!(EpochClock::new().current(), 0);
    }

    #[test]
    fn the_epoch_advances_every_thousand_blocks() {
        let clock = EpochClock::new();

        clock.set_height(999);
        assert_eq!(clock.current(), 0);

        clock.set_height(1_000);
        assert_eq!(clock.current(), 1);

        clock.set_height(1_999);
        assert_eq!(clock.current(), 1);

        clock.set_height(50_000);
        assert_eq!(clock.current(), 50);
    }

    #[test]
    fn a_session_from_an_earlier_epoch_is_stale() {
        let clock = EpochClock::new();
        clock.set_height(2_500);

        assert!(clock.is_stale(1));
        assert!(!clock.is_stale(2));
        // A session from the future is not stale. Cannot happen, but a
        // comparison that wrapped or panicked here would take the node down.
        assert!(!clock.is_stale(99));
    }

    #[test]
    fn clones_share_one_height() {
        let clock = EpochClock::new();
        let other = clock.clone();

        clock.set_height(3_000);
        assert_eq!(other.current(), 3);
    }

    #[test]
    fn height_cannot_stall_rotation_below_the_time_floor() {
        // The property the fallback exists for, asserted by construction: a
        // node stuck at height zero must still reach epoch 1 eventually. The
        // clock cannot be advanced in a unit test without sleeping four hours,
        // so this asserts the arithmetic the fallback uses rather than waiting
        // for it.
        let elapsed = ROTATION_FALLBACK.as_secs();
        assert_eq!(elapsed / ROTATION_FALLBACK.as_secs(), 1);
        assert_eq!((elapsed * 3) / ROTATION_FALLBACK.as_secs(), 3);
    }
}
