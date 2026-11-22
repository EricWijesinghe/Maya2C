//! The client session table is bounded (APP-13): strangers past the cap push
//! out the least recently active session, never a contact still talking.
#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use maya_chat::client::Client;
use maya_chat::identity::{Identity, PrekeyBundle};

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 24 * 3_600;

fn bob() -> (Identity, PrekeyBundle) {
    let bob = Identity::from_seed([2; 32]);
    let bundle = bob.prekey_bundle(1, NOW + DAY).unwrap();
    (bob, bundle)
}

fn stranger(seed: u8) -> Client {
    Client::new(Identity::from_seed([seed; 32]))
}

#[test]
fn strangers_past_the_cap_evict_the_least_recent_not_a_live_contact() {
    let (bob_id, bundle) = bob();
    let to = bob_id.address();
    let mut b = Client::new(bob_id).with_max_sessions(2);
    let mut alice = stranger(1);

    b.open_frame(&alice.seal(to, Some(&bundle), b"hi", NOW).unwrap(), NOW)
        .unwrap();
    let mut s10 = stranger(10);
    b.open_frame(
        &s10.seal(to, Some(&bundle), b"spam", NOW + 1).unwrap(),
        NOW + 1,
    )
    .unwrap();
    // Alice keeps talking, so she is the more recent of the two.
    b.open_frame(&alice.seal(to, None, b"again", NOW + 2).unwrap(), NOW + 2)
        .unwrap();

    // Recency survives a save and restore.
    let saved = b.save().unwrap();
    let mut b = Client::restore(Identity::from_seed([2; 32]), &saved)
        .unwrap()
        .with_max_sessions(2);

    b.open_frame(
        &stranger(11)
            .seal(to, Some(&bundle), b"spam", NOW + 3)
            .unwrap(),
        NOW + 3,
    )
    .unwrap();
    assert_eq!(b.session_count(), 2);
    assert!(b.has_session(&alice.identity().address()));
    assert!(!b.has_session(&s10.identity().address()));

    // The known contact still gets through, and the receipt owed to the
    // evicted stranger does not fail the pass.
    b.open_frame(
        &alice.seal(to, None, b"still here", NOW + 4).unwrap(),
        NOW + 4,
    )
    .unwrap();
    b.end_pass(NOW + 4).unwrap();

    // The evicted stranger's old session is gone; a fresh handshake works.
    assert!(
        b.open_frame(&s10.seal(to, None, b"old", NOW + 5).unwrap(), NOW + 5)
            .is_err()
    );
    b.open_frame(
        &stranger(10)
            .seal(to, Some(&bundle), b"new", NOW + 6)
            .unwrap(),
        NOW + 6,
    )
    .unwrap();
    assert!(b.has_session(&alice.identity().address()));
    assert_eq!(b.session_count(), 2);
}
