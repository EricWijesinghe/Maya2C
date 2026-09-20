//! Kani proofs over the batch rules. `cargo kani -p maya-iot-anchor` (Linux).

use crate::record::Progress;
use crate::rules::{
    BatchOutcome, BatchView, DeviceStatus, GENESIS_PREVIOUS, judge_batch, record_batch,
};
use crate::types::Bounds;

fn any_status() -> DeviceStatus {
    match kani::any::<u8>() % 4 {
        0 => DeviceStatus::Active,
        1 => DeviceStatus::Tampered,
        2 => DeviceStatus::Compromised,
        _ => DeviceStatus::Revoked,
    }
}

fn any_progress() -> Progress {
    Progress {
        has_batch: kani::any(),
        first: kani::any(),
        last: kani::any(),
        hash: [kani::any(); 32],
        previous: [kani::any(); 32],
        min: kani::any(),
        max: kani::any(),
        height: kani::any(),
        batches: kani::any(),
        anomalies: kani::any(),
    }
}

fn any_view() -> BatchView {
    BatchView {
        first: kani::any(),
        last: kani::any(),
        hash: [kani::any(); 32],
        previous: [kani::any(); 32],
        min: kani::any(),
        max: kani::any(),
    }
}

fn any_bounds() -> Bounds {
    Bounds {
        min: kani::any(),
        max: kani::any(),
        max_step: kani::any(),
    }
}

/// A terminal device records nothing, whatever it is sent.
#[kani::proof]
fn a_terminal_device_records_nothing() {
    let status = any_status();
    let outcome = judge_batch(status, &any_progress(), &any_bounds(), &any_view());
    if status.is_terminal() {
        assert_eq!(outcome, BatchOutcome::Inactive);
    }
}

/// A recorded batch extends the recorded head — or starts the chain — and moves
/// the counter strictly forward, so the recorded history is one hash chain.
#[kani::proof]
fn recording_extends_the_head() {
    let progress = any_progress();
    let view = any_view();
    if let BatchOutcome::Recorded { anomalous } =
        judge_batch(any_status(), &progress, &any_bounds(), &view)
    {
        if progress.has_batch {
            assert!(view.previous == progress.hash);
            assert!(view.first > progress.last);
        } else {
            assert!(view.previous == GENESIS_PREVIOUS);
        }
        let next = record_batch(&progress, &view, anomalous, kani::any());
        assert!(next.has_batch && next.hash == view.hash && next.previous == view.previous);
    }
}

/// Equivocation needs different content that conflicts with the head: the same
/// predecessor, overlapping counters, or an extension that reuses counters.
/// A duplicate relay is never equivocation.
#[kani::proof]
fn equivocation_needs_real_conflict() {
    let progress = any_progress();
    let view = any_view();
    let outcome = judge_batch(DeviceStatus::Active, &progress, &any_bounds(), &view);
    if outcome == BatchOutcome::Equivocation {
        assert!(progress.has_batch);
        assert!(view.hash != progress.hash);
        let overlap = view.first <= progress.last && view.last >= progress.first;
        let fork = view.previous == progress.previous;
        let reuse = view.previous == progress.hash && view.first <= progress.last;
        assert!(overlap || fork || reuse);
    }
    if progress.has_batch && view.hash == progress.hash {
        assert_eq!(outcome, BatchOutcome::Duplicate);
    }
}
