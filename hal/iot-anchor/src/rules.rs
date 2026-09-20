//! What consensus decides about a verified batch. Pure integer functions, no
//! crypto, so the Kani harnesses in `proofs.rs` (compiled only under
//! `cfg(kani)`) can state them exhaustively.
//!
//! Outcomes that are not a recording are never errors: two gateways relaying
//! one batch, or a batch overtaken by a newer one, is ordinary, and an error
//! would void the block the earlier copy sits in (invariant 7).

use crate::record::Progress;
use crate::types::Bounds;

/// Heights after the last batch (or enrollment) at which a device reads as
/// silent. Silence is not evidence of tampering.
pub const SILENT_AFTER_BLOCKS: u64 = 720;

/// The predecessor a device's first batch names: the start of its hash chain.
pub const GENESIS_PREVIOUS: [u8; 32] = [0; 32];

/// A device's lifecycle. Every state but `Active` is terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceStatus {
    /// Accepting telemetry.
    Active,
    /// Signed a tamper event.
    Tampered,
    /// Signed conflicting batches.
    Compromised,
    /// Revoked by its owner.
    Revoked,
}

impl DeviceStatus {
    /// Wire tag. Consensus; never renumber.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Active => 1,
            Self::Tampered => 2,
            Self::Compromised => 3,
            Self::Revoked => 4,
        }
    }

    /// The status a tag names.
    #[must_use]
    pub const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Active),
            2 => Some(Self::Tampered),
            3 => Some(Self::Compromised),
            4 => Some(Self::Revoked),
            _ => None,
        }
    }

    /// Whether nothing moves this device again.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Active)
    }

    /// Fixed label for RPC.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Tampered => "tampered",
            Self::Compromised => "compromised",
            Self::Revoked => "revoked",
        }
    }
}

/// The parts of a verified batch the rules read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchView {
    /// First counter.
    pub first: u64,
    /// Last counter.
    pub last: u64,
    /// Hash of the signed body.
    pub hash: [u8; 32],
    /// Hash of the device's previous batch, or [`GENESIS_PREVIOUS`].
    pub previous: [u8; 32],
    /// Lowest reading.
    pub min: i64,
    /// Highest reading.
    pub max: i64,
}

/// What a verified batch does to a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchOutcome {
    /// Recorded as the device's latest batch.
    Recorded {
        /// Outside the declared bounds or step. Recorded regardless.
        anomalous: bool,
    },
    /// The latest batch again: a second relay. No-op.
    Duplicate,
    /// Older than the latest batch and not overlapping it. No-op.
    Stale,
    /// Ahead of the latest batch, but naming a predecessor that is not the
    /// recorded head: relayed out of order. No-op; the gateway retries it
    /// once its predecessor is recorded.
    Unlinked,
    /// Conflicts with the latest batch — a second successor of its
    /// predecessor, overlapping counters, or an extension that reuses
    /// counters — so two signers hold the key.
    Equivocation,
    /// The device is in a terminal state. No-op.
    Inactive,
}

fn distance(a: i64, b: i64) -> u128 {
    (i128::from(a) - i128::from(b)).unsigned_abs()
}

/// Whether `view` lies outside `bounds`, or moved further than
/// `bounds.max_step` from the previous batch.
#[must_use]
pub fn is_anomalous(bounds: &Bounds, progress: &Progress, view: &BatchView) -> bool {
    let out_of_range = view.min < bounds.min || view.max > bounds.max;
    let step = u128::from(bounds.max_step);
    let jumped = progress.has_batch
        && (distance(view.min, progress.min) > step || distance(view.max, progress.max) > step);
    out_of_range || jumped
}

/// Judges a batch whose signature already verified.
#[must_use]
pub fn judge_batch(
    status: DeviceStatus,
    progress: &Progress,
    bounds: &Bounds,
    view: &BatchView,
) -> BatchOutcome {
    if status.is_terminal() {
        return BatchOutcome::Inactive;
    }
    let recorded = || BatchOutcome::Recorded {
        anomalous: is_anomalous(bounds, progress, view),
    };
    if !progress.has_batch {
        return if view.previous == GENESIS_PREVIOUS {
            recorded()
        } else {
            BatchOutcome::Unlinked
        };
    }
    if view.hash == progress.hash {
        return BatchOutcome::Duplicate;
    }
    if view.previous == progress.hash {
        // Extends the head: accepted only if it moves the counter forward.
        return if view.first > progress.last {
            recorded()
        } else {
            BatchOutcome::Equivocation
        };
    }
    let fork = view.previous == progress.previous;
    let overlap = view.first <= progress.last && view.last >= progress.first;
    if fork || overlap {
        BatchOutcome::Equivocation
    } else if view.first <= progress.last {
        BatchOutcome::Stale
    } else {
        BatchOutcome::Unlinked
    }
}

/// The progress after recording `view` at `height`. Returns a new value.
#[must_use]
pub fn record_batch(
    progress: &Progress,
    view: &BatchView,
    anomalous: bool,
    height: u64,
) -> Progress {
    Progress {
        has_batch: true,
        first: view.first,
        last: view.last,
        hash: view.hash,
        previous: view.previous,
        min: view.min,
        max: view.max,
        height,
        batches: progress.batches.saturating_add(1),
        anomalies: progress.anomalies.saturating_add(u64::from(anomalous)),
    }
}

/// Whether a device has been silent too long at `height`.
#[must_use]
pub fn is_silent(progress: &Progress, enrolled_height: u64, height: u64) -> bool {
    let since = if progress.has_batch {
        progress.height.max(enrolled_height)
    } else {
        enrolled_height
    };
    height.saturating_sub(since) >= SILENT_AFTER_BLOCKS
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const BOUNDS: Bounds = Bounds {
        min: -10,
        max: 10,
        max_step: 5,
    };

    /// A view whose body hash is `[tag; 32]` and whose predecessor is
    /// `[previous; 32]` (`0` is the chain start).
    fn view(first: u64, last: u64, tag: u8, previous: u8, min: i64, max: i64) -> BatchView {
        BatchView {
            first,
            last,
            hash: [tag; 32],
            previous: [previous; 32],
            min,
            max,
        }
    }

    #[test]
    fn a_first_batch_must_start_the_chain_and_bounds_only_flag() {
        let fresh = Progress::default();
        let judge = |v| judge_batch(DeviceStatus::Active, &fresh, &BOUNDS, &v);
        assert_eq!(
            judge(view(0, 9, 1, 0, 0, 3)),
            BatchOutcome::Recorded { anomalous: false }
        );
        assert_eq!(
            judge(view(0, 9, 1, 0, 0, 50)),
            BatchOutcome::Recorded { anomalous: true }
        );
        assert_eq!(judge(view(0, 9, 1, 5, 0, 3)), BatchOutcome::Unlinked);
    }

    #[test]
    fn extensions_replays_forks_and_old_batches_are_told_apart() {
        // Head: hash [1], predecessor the chain start, counters 10..=19.
        let head = record_batch(&Progress::default(), &view(10, 19, 1, 0, 0, 3), false, 5);
        let judge = |v| judge_batch(DeviceStatus::Active, &head, &BOUNDS, &v);

        assert_eq!(judge(view(10, 19, 1, 0, 0, 3)), BatchOutcome::Duplicate);
        assert_eq!(
            judge(view(20, 29, 2, 1, 1, 4)),
            BatchOutcome::Recorded { anomalous: false }
        );
        assert_eq!(
            judge(view(20, 29, 2, 1, 1, 9)),
            BatchOutcome::Recorded { anomalous: true }
        );
        // Extends the head but reuses counters the head already spoke for.
        assert_eq!(judge(view(15, 25, 2, 1, 0, 3)), BatchOutcome::Equivocation);
        // A second successor of the head's own predecessor: a fork.
        assert_eq!(
            judge(view(200, 209, 3, 0, 0, 3)),
            BatchOutcome::Equivocation
        );
        // Overlaps the head's counters with different content.
        assert_eq!(judge(view(15, 25, 4, 9, 0, 3)), BatchOutcome::Equivocation);
        // Older and unrelated: a late relay.
        assert_eq!(judge(view(0, 9, 5, 9, 0, 3)), BatchOutcome::Stale);
        // Ahead, but its predecessor is not recorded yet: relayed out of order.
        assert_eq!(judge(view(30, 39, 6, 9, 0, 3)), BatchOutcome::Unlinked);
        assert_eq!(
            judge_batch(
                DeviceStatus::Revoked,
                &head,
                &BOUNDS,
                &view(20, 29, 2, 1, 1, 4)
            ),
            BatchOutcome::Inactive
        );
    }

    #[test]
    fn extreme_readings_do_not_overflow_the_step() {
        let head = record_batch(
            &Progress::default(),
            &view(0, 0, 1, 0, i64::MIN, i64::MIN),
            false,
            1,
        );
        // The full i64 span is u64::MAX: one below it is a jump, equal is not.
        let wide = Bounds {
            min: i64::MIN,
            max: i64::MAX,
            max_step: u64::MAX - 1,
        };
        assert_eq!(
            judge_batch(
                DeviceStatus::Active,
                &head,
                &wide,
                &view(1, 1, 2, 1, i64::MAX, i64::MAX)
            ),
            BatchOutcome::Recorded { anomalous: true }
        );
    }

    #[test]
    fn silence_is_measured_from_the_latest_activity() {
        let fresh = Progress::default();
        assert!(!is_silent(&fresh, 100, 100 + SILENT_AFTER_BLOCKS - 1));
        assert!(is_silent(&fresh, 100, 100 + SILENT_AFTER_BLOCKS));
        let active = record_batch(&fresh, &view(0, 0, 1, 0, 0, 0), false, 500);
        assert!(!is_silent(&active, 100, 500 + SILENT_AFTER_BLOCKS - 1));
    }
}
