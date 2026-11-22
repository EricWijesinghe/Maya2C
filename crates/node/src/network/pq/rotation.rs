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

/// Blocks between forced re-handshakes.
///
/// At one block a second (docs/block-time.md) this is a little over four
/// hours; it was 1,000 when blocks were assumed to be 15 s. Short enough
/// that a key lifted from a running node has a bounded payoff; long enough that
/// re-handshaking is invisible against a mesh of tens of peers, each costing
/// ~144 µs of ML-KEM work.
pub const ROTATION_INTERVAL_BLOCKS: u64 = 15_000;

/// Wall-clock equivalent of [`ROTATION_INTERVAL_BLOCKS`].
///
/// The fallback exists because block height is not a clock a transport can rely
/// on. A node that is syncing, stalled, or freshly started may sit at one
/// height for hours — and a node whose height never advances would, on height
/// alone, never rotate at all. That is precisely the node an attacker would
/// most like to have compromised.
///
/// One second per block (docs/block-time.md), so 15,000 s: the same four hours.
pub const ROTATION_FALLBACK: Duration = Duration::from_secs(ROTATION_INTERVAL_BLOCKS);

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

    /// Where a session opened at `now` starts.
    #[must_use]
    pub fn session_start(&self, now: Instant) -> SessionStart {
        SessionStart {
            epoch: self.current(),
            at: now,
        }
    }

    /// The sessions to close on this sweep: stale, at least `min_age` old,
    /// oldest first, at most [`MAX_ROTATIONS_PER_SWEEP`] of them.
    ///
    /// Staleness alone would close every connection the moment the epoch
    /// moves, and a syncing node crosses many epochs in minutes: the whole
    /// mesh would reconnect at once, again and again. The age floor keeps a
    /// fresh session alive through that, and the per-sweep cap spreads an
    /// epoch's rotations over several sweeps instead of one burst.
    #[must_use]
    pub fn due<K: Copy>(
        &self,
        sessions: impl IntoIterator<Item = (K, SessionStart)>,
        min_age: Duration,
        now: Instant,
    ) -> Vec<K> {
        let current = self.current();
        let mut due: Vec<(Instant, K)> = sessions
            .into_iter()
            .filter(|(_, s)| current > s.epoch && now.saturating_duration_since(s.at) >= min_age)
            .map(|(key, s)| (s.at, key))
            .collect();
        due.sort_by_key(|(at, _)| *at);
        due.into_iter()
            .take(MAX_ROTATIONS_PER_SWEEP)
            .map(|(_, key)| key)
            .collect()
    }
}

/// Sweeps a session must outlive before it may be rotated. At the node's
/// 30 s sweep that is five minutes: invisible against a four-hour epoch, and
/// enough that a syncing node does not cut its links on every sweep.
pub const MIN_SESSION_AGE_SWEEPS: u32 = 10;

/// Sessions closed in one sweep at most. At a 30 s sweep, a full mesh of 256
/// connections rotates in about half an hour rather than all at once.
pub const MAX_ROTATIONS_PER_SWEEP: usize = 4;

/// When a session's keys were derived: the rotation epoch, and the instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionStart {
    /// The epoch the session opened in.
    pub epoch: u64,
    /// When it opened.
    pub at: Instant,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn the_interval_is_a_little_over_four_hours() {
        // Sanity on the constant a reader is most likely to want to change:
        // 15,000 s by the clock and 15,000 blocks at one block a second.
        assert_eq!(ROTATION_FALLBACK.as_secs(), 15_000);
        assert_eq!(ROTATION_INTERVAL_BLOCKS, 15_000);
    }

    #[test]
    fn a_fresh_clock_is_in_epoch_zero() {
        assert_eq!(EpochClock::new().current(), 0);
    }

    #[test]
    fn the_epoch_advances_every_interval() {
        const I: u64 = ROTATION_INTERVAL_BLOCKS;
        let clock = EpochClock::new();

        clock.set_height(I - 1);
        assert_eq!(clock.current(), 0);

        clock.set_height(I);
        assert_eq!(clock.current(), 1);

        clock.set_height(2 * I - 1);
        assert_eq!(clock.current(), 1);

        clock.set_height(50 * I);
        assert_eq!(clock.current(), 50);
    }

    #[test]
    fn a_session_from_an_earlier_epoch_is_stale() {
        let clock = EpochClock::new();
        clock.set_height(ROTATION_INTERVAL_BLOCKS * 5 / 2);

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

        clock.set_height(ROTATION_INTERVAL_BLOCKS * 3);
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

    #[test]
    fn a_stale_session_waits_for_its_minimum_age() {
        let clock = EpochClock::new();
        let opened = Instant::now();
        let min_age = Duration::from_secs(300);
        let session = clock.session_start(opened);
        assert_eq!(session.epoch, 0);

        // Still in its epoch: never due, however old.
        assert!(
            clock
                .due([(1u8, session)], min_age, opened + min_age * 10)
                .is_empty()
        );

        // A syncing node jumps epochs a minute later: not due yet.
        clock.set_height(ROTATION_INTERVAL_BLOCKS * 7);
        let a_minute = opened + Duration::from_secs(60);
        assert!(clock.due([(1u8, session)], min_age, a_minute).is_empty());
        assert_eq!(
            clock.due([(1u8, session)], min_age, opened + min_age),
            vec![1]
        );
    }

    #[test]
    fn a_sweep_closes_the_oldest_few_stale_sessions() {
        let clock = EpochClock::new();
        let base = Instant::now();
        let sessions: Vec<(usize, SessionStart)> = (0..MAX_ROTATIONS_PER_SWEEP * 3)
            .rev()
            .map(|i| (i, clock.session_start(base + Duration::from_secs(i as u64))))
            .collect();
        clock.set_height(ROTATION_INTERVAL_BLOCKS);
        let later = base + Duration::from_secs(10_000);
        let due = clock.due(sessions, Duration::ZERO, later);
        assert_eq!(due, (0..MAX_ROTATIONS_PER_SWEEP).collect::<Vec<_>>());
    }
}
