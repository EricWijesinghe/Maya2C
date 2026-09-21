//! The watcher's decisions over plain values: pairing, claiming, refusing to
//! reveal late, refunding, finishing — and the journal that remembers them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_htlc_lattice::{LatticeSecret, Unlock};
use maya_htlc_watcher::{
    Action, Alert, BlockRate, ChainPoint, ChainSide, Journal, Leg, LockState, LockView, Margins,
    Observation, Outcome, PairingError, Phase, Role, Swap, WatcherError, check_pairing, decide,
};

const COMMITMENT: [u8; 32] = [7; 32];
const MAYA_LOCK: [u8; 32] = [10; 32];
const COUNTERPARTY_LOCK: [u8; 32] = [11; 32];

fn margins() -> Margins {
    Margins {
        maya_confirmations: 2,
        counterparty_confirmations: 2,
        submission_blocks: 1,
    }
}

fn opening() -> Unlock {
    Unlock::Opening(LatticeSecret::from_entropy([3; 32]).opening())
}

fn view(expiry_height: u64, state: LockState) -> LockView {
    LockView {
        sender: [1; 32],
        recipient: [2; 32],
        amount: 100,
        expiry_height,
        commitment_id: COMMITMENT,
        state,
    }
}

fn claimed(height: u64) -> LockState {
    LockState::Claimed {
        height,
        unlock: opening(),
    }
}

/// Responder: inbound on Maya (expiry 100), outbound on the counterparty (50).
fn responder() -> Swap {
    Swap {
        commitment_id: COMMITMENT,
        role: Role::Responder,
        inbound: Some(Leg {
            side: ChainSide::Maya,
            lock_id: MAYA_LOCK,
            expiry_height: 100,
        }),
        outbound: Leg {
            side: ChainSide::Counterparty,
            lock_id: COUNTERPARTY_LOCK,
            expiry_height: 50,
        },
        phase: Phase::Active,
    }
}

/// Initiator: inbound on the counterparty (expiry 50), outbound on Maya (100).
fn initiator() -> Swap {
    Swap {
        commitment_id: COMMITMENT,
        role: Role::Initiator,
        inbound: Some(Leg {
            side: ChainSide::Counterparty,
            lock_id: COUNTERPARTY_LOCK,
            expiry_height: 50,
        }),
        outbound: Leg {
            side: ChainSide::Maya,
            lock_id: MAYA_LOCK,
            expiry_height: 100,
        },
        phase: Phase::Active,
    }
}

fn observe(
    inbound_tip: u64,
    inbound: LockView,
    outbound_tip: u64,
    outbound: LockView,
) -> Observation {
    Observation {
        inbound_tip,
        outbound_tip,
        inbound: Some(inbound),
        outbound: Some(outbound),
    }
}

// ---------------------------------------------------------------------------
// pairing
// ---------------------------------------------------------------------------

fn point(tip: u64, expiry_height: u64) -> ChainPoint {
    ChainPoint {
        tip,
        expiry_height,
        confirmations: 2,
    }
}

#[test]
fn a_pairing_with_margin_reports_its_slack() {
    // 10 + (40 + 2) + 1 + 2 = 55 blocks needed, 100 available.
    assert_eq!(
        check_pairing(point(10, 100), point(10, 50), BlockRate::EQUAL, 1),
        Ok(45)
    );
}

#[test]
fn a_pairing_with_no_spare_block_is_refused() {
    assert_eq!(
        check_pairing(point(10, 55), point(10, 50), BlockRate::EQUAL, 1),
        Err(PairingError::TooTight {
            required: 55,
            expiry: 55
        })
    );
}

#[test]
fn the_block_rate_rounds_towards_refusal() {
    // 42 short blocks at 3 long per 2 short is 63, rounded up.
    let rate = BlockRate {
        long_blocks: 3,
        per_short_blocks: 2,
    };
    assert!(check_pairing(point(10, 76), point(10, 50), rate, 1).is_err());
    assert_eq!(check_pairing(point(10, 77), point(10, 50), rate, 1), Ok(1));
}

#[test]
fn degenerate_pairings_are_refused_not_computed() {
    let zero = BlockRate {
        long_blocks: 1,
        per_short_blocks: 0,
    };
    assert_eq!(
        check_pairing(point(0, 100), point(0, 50), zero, 1),
        Err(PairingError::ZeroRate)
    );
    assert_eq!(
        check_pairing(point(0, 100), point(50, 50), BlockRate::EQUAL, 1),
        Err(PairingError::ShortLegExpired)
    );
    assert_eq!(
        check_pairing(point(u64::MAX, u64::MAX), point(0, 50), BlockRate::EQUAL, 1),
        Err(PairingError::Overflow)
    );
}

// ---------------------------------------------------------------------------
// responder
// ---------------------------------------------------------------------------

#[test]
fn the_responder_claims_the_moment_the_opening_appears() {
    // One confirmation of the reveal, below the two required to finish: the
    // claim does not wait for it.
    let observation = observe(60, view(100, LockState::Locked), 40, view(50, claimed(40)));
    assert_eq!(
        decide(&responder(), &observation, None, &margins()),
        vec![Action::Claim {
            side: ChainSide::Maya,
            lock_id: MAYA_LOCK,
            unlock: opening(),
        }]
    );
}

#[test]
fn the_responder_does_nothing_while_both_locks_wait() {
    let observation = observe(
        30,
        view(100, LockState::Locked),
        30,
        view(50, LockState::Locked),
    );
    assert!(decide(&responder(), &observation, None, &margins()).is_empty());
}

#[test]
fn the_responder_refunds_once_its_lock_can_no_longer_be_claimed() {
    let before = observe(
        48,
        view(100, LockState::Locked),
        48,
        view(50, LockState::Locked),
    );
    assert!(decide(&responder(), &before, None, &margins()).is_empty());

    // The next block is height 50, the expiry: a refund there is admitted.
    let at = observe(
        49,
        view(100, LockState::Locked),
        49,
        view(50, LockState::Locked),
    );
    assert_eq!(
        decide(&responder(), &at, None, &margins()),
        vec![Action::Refund {
            side: ChainSide::Counterparty,
            lock_id: COUNTERPARTY_LOCK,
        }]
    );
}

#[test]
fn a_reveal_after_the_inbound_expiry_is_an_alert_not_a_doomed_claim() {
    let observation = observe(99, view(100, LockState::Locked), 45, view(50, claimed(45)));
    assert_eq!(
        decide(&responder(), &observation, None, &margins()),
        vec![Action::Alert(Alert::ClaimWindowClosed)]
    );
}

// ---------------------------------------------------------------------------
// initiator
// ---------------------------------------------------------------------------

#[test]
fn the_initiator_reveals_while_there_is_room_to_bury_the_claim() {
    // Lands at 41 at the earliest, buried by 41 + 1 + 2 = 44 < 50.
    let observation = observe(
        40,
        view(50, LockState::Locked),
        40,
        view(100, LockState::Locked),
    );
    let secret = opening();
    assert_eq!(
        decide(&initiator(), &observation, Some(&secret), &margins()),
        vec![Action::Claim {
            side: ChainSide::Counterparty,
            lock_id: COUNTERPARTY_LOCK,
            unlock: secret.clone(),
        }]
    );
}

#[test]
fn the_initiator_refuses_to_reveal_late() {
    // 47 + 1 + 2 = 50, not below the expiry. A claim here could land after
    // it, do nothing, and still publish the opening — costing both legs.
    let observation = observe(
        46,
        view(50, LockState::Locked),
        46,
        view(100, LockState::Locked),
    );
    let actions = decide(&initiator(), &observation, Some(&opening()), &margins());
    assert_eq!(actions, vec![Action::Alert(Alert::RevealWindowClosed)]);
    assert!(!actions.iter().any(|a| matches!(a, Action::Claim { .. })));
}

#[test]
fn an_initiator_without_its_secret_says_so() {
    let observation = observe(
        10,
        view(50, LockState::Locked),
        10,
        view(100, LockState::Locked),
    );
    assert_eq!(
        decide(&initiator(), &observation, None, &margins()),
        vec![Action::Alert(Alert::SecretMissing)]
    );
}

#[test]
fn an_unanswered_initiation_is_refunded_and_finishes_unwound() {
    let mut swap = initiator();
    swap.inbound = None;
    let waiting = Observation {
        inbound_tip: 99,
        outbound_tip: 99,
        inbound: None,
        outbound: Some(view(100, LockState::Locked)),
    };
    assert_eq!(
        decide(&swap, &waiting, Some(&opening()), &margins()),
        vec![Action::Refund {
            side: ChainSide::Maya,
            lock_id: MAYA_LOCK,
        }]
    );
    let refunded = Observation {
        outbound_tip: 101,
        outbound: Some(view(100, LockState::Refunded { height: 100 })),
        ..waiting
    };
    assert_eq!(
        decide(&swap, &refunded, None, &margins()),
        vec![Action::Finish(Outcome::Unwound)]
    );
}

// ---------------------------------------------------------------------------
// finishing and alerts
// ---------------------------------------------------------------------------

#[test]
fn outcomes_wait_for_both_legs_to_be_buried() {
    let refunded = |height| LockState::Refunded { height };
    // (inbound state, its settled height, outbound state, its settled height)
    let cases = [
        (claimed(60), 60, claimed(40), 40, Outcome::Swapped),
        (refunded(100), 100, refunded(50), 50, Outcome::Unwound),
        (claimed(60), 60, refunded(50), 50, Outcome::KeptBoth),
        (refunded(100), 100, claimed(40), 40, Outcome::Lost),
    ];
    for (inbound, inbound_height, outbound, outbound_height, outcome) in cases {
        // One confirmation each: settled, not buried.
        let shallow = observe(
            inbound_height,
            view(100, inbound.clone()),
            outbound_height,
            view(50, outbound.clone()),
        );
        assert!(
            !decide(&responder(), &shallow, None, &margins()).contains(&Action::Finish(outcome))
        );

        let deep = observe(
            inbound_height + 1,
            view(100, inbound),
            outbound_height + 1,
            view(50, outbound),
        );
        assert_eq!(
            decide(&responder(), &deep, None, &margins()),
            vec![Action::Finish(outcome)]
        );
    }
}

#[test]
fn a_missing_or_foreign_lock_is_reported_and_nothing_is_signed() {
    let missing = Observation {
        inbound_tip: 1,
        outbound_tip: 1,
        inbound: None,
        outbound: Some(view(50, LockState::Locked)),
    };
    assert_eq!(
        decide(&responder(), &missing, None, &margins()),
        vec![Action::Alert(Alert::LockMissing(ChainSide::Maya))]
    );

    let mut foreign = view(100, LockState::Locked);
    foreign.commitment_id = [8; 32];
    let observation = observe(1, foreign, 1, view(50, claimed(1)));
    assert_eq!(
        decide(&responder(), &observation, None, &margins()),
        vec![Action::Alert(Alert::CommitmentMismatch(ChainSide::Maya))]
    );
}

// ---------------------------------------------------------------------------
// journal
// ---------------------------------------------------------------------------

#[test]
fn the_journal_survives_a_restart_and_refuses_a_reused_commitment() {
    let dir = tempfile::TempDir::new().expect("dir");
    let path = dir.path().join("swaps.json");

    let mut journal = Journal::open(&path).expect("open empty");
    journal.insert(responder()).expect("insert");
    assert!(matches!(
        journal.insert(initiator()),
        Err(WatcherError::DuplicateSwap(_))
    ));
    journal
        .finish(&COMMITMENT, Outcome::Swapped)
        .expect("finish");

    let reopened = Journal::open(&path).expect("reopen");
    assert_eq!(reopened.swaps().len(), 1);
    assert_eq!(reopened.swaps()[0].phase, Phase::Finished(Outcome::Swapped));
    // A finished swap still holds its commitment: a secret is single-use.
    let mut reopened = reopened;
    assert!(reopened.insert(responder()).is_err());
}

#[test]
fn an_initiation_is_paired_exactly_once() {
    let dir = tempfile::TempDir::new().expect("dir");
    let mut journal = Journal::open(&dir.path().join("swaps.json")).expect("open");
    let mut swap = initiator();
    let leg = swap.inbound.take().expect("leg");
    journal.insert(swap).expect("insert");
    journal.pair(&COMMITMENT, leg).expect("pair");
    assert!(journal.pair(&COMMITMENT, leg).is_err());
    assert!(journal.pair(&[0; 32], leg).is_err());
}
