//! Kani proofs. Compiled only under `cargo kani -p maya-threat-intel`.
//!
//! What they establish is about the functions a block's execution calls, not a
//! restatement of them: the closed-form quarantine length agrees with the decay
//! at every height, the stored score never passes its cap, and every record
//! `observe` produces decodes back to itself.

use crate::evidence::OffenceKind;
use crate::indicator::ThreatIndicator;
use crate::score::{self, CONFIRM_SCORE, MAX_SCORE};

fn any_kind() -> OffenceKind {
    if kani::any() {
        OffenceKind::InvalidSignature
    } else {
        OffenceKind::TxRootMismatch
    }
}

/// A node deciding "quarantined?" from the closed form reaches the same answer
/// as one running the decay, for every score, last height and query height.
#[kani::proof]
fn the_closed_form_is_the_decay() {
    let stored: u64 = kani::any();
    let last: u64 = kani::any();
    let height: u64 = kani::any();
    assert_eq!(
        score::decay(stored, last, height) >= CONFIRM_SCORE,
        score::is_active(stored, last, height)
    );
}

/// Where the lift height does not saturate, the quarantine holds exactly below
/// it.
#[kani::proof]
fn a_quarantine_holds_exactly_below_its_lift_height() {
    let stored: u64 = kani::any();
    let last: u64 = kani::any();
    let height: u64 = kani::any();
    kani::assume(height >= last);
    match score::until_height(stored, last) {
        None => assert!(!score::is_active(stored, last, height)),
        Some(until) if until < u64::MAX => {
            assert_eq!(score::is_active(stored, last, height), height < until);
        }
        // Partial on purpose: a lift height saturated at `u64::MAX` is not
        // claimed. `the_closed_form_is_the_decay` covers `is_active` there.
        Some(_) => {}
    }
}

/// An observation keeps the record canonical: capped, counted, ordered, and
/// decodable to itself — so no sequence of blocks writes a record the node
/// then refuses to read.
#[kani::proof]
fn observe_keeps_a_record_canonical() {
    let previous = ThreatIndicator {
        score: kani::any(),
        first_height: kani::any(),
        last_height: kani::any(),
        offences: kani::any(),
    };
    kani::assume(previous.score <= MAX_SCORE);
    kani::assume(previous.offences > 0);
    kani::assume(previous.first_height <= previous.last_height);
    let height: u64 = kani::any();

    let next = ThreatIndicator::observe(Some(&previous), any_kind(), height);

    assert!(next.score <= MAX_SCORE);
    assert!(next.score >= CONFIRM_SCORE);
    assert!(next.offences >= previous.offences);
    assert!(next.first_height <= next.last_height);
    assert_eq!(ThreatIndicator::decode(&next.encode()), Ok(next));
}

/// A fresh offence always confirms, at the block that recorded it.
#[kani::proof]
fn a_first_offence_quarantines_at_its_own_height() {
    let height: u64 = kani::any();
    let record = ThreatIndicator::observe(None, any_kind(), height);
    assert!(record.is_active(height));
}
