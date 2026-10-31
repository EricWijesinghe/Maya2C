//! A failed send must never rewind an established session (ADR-074 finding).
//!
//! When a deposit fails, the relay may still hold the envelope: only the reply
//! was lost. Restoring the last saved client then seals the next message under
//! the same chain index, which means the same message key and nonce for a
//! different plaintext (a two-time pad). Keeping the advanced chain costs at
//! most one skipped key at the receiver, which `MAX_SKIP` already covers.
#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use maya_chat::client::Client;
use maya_chat::identity::Identity;
use maya_chat::relay::{Envelope, Payload};

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 24 * 3_600;

fn alice() -> Identity {
    Identity::from_seed([1; 32])
}
fn bob() -> Identity {
    Identity::from_seed([2; 32])
}

fn index(envelope: &Envelope) -> u64 {
    match &envelope.payload {
        Payload::Chat(message) => message.n,
        Payload::Open { .. } => panic!("expected an established session"),
    }
}

/// Alice and Bob with an established session, Alice's state saved after it.
fn established() -> (Client, Client, Vec<u8>) {
    let mut a = Client::new(alice());
    let mut b = Client::new(bob());
    let bundle = bob().prekey_bundle(1, NOW + DAY).unwrap();
    let open = a.seal(bob().address(), Some(&bundle), b"hi", NOW).unwrap();
    b.open(&open, NOW).unwrap();
    let saved = a.save().unwrap();
    (a, b, saved)
}

#[test]
fn rewinding_after_a_lost_reply_reuses_the_chain_index() {
    // The bug, pinned so nobody reintroduces the restore-on-failure path.
    let (mut a, _b, saved) = established();
    let lost = a.seal(bob().address(), None, b"secret one", NOW).unwrap();
    let mut rewound = Client::restore(alice(), &saved).unwrap();
    let next = rewound
        .seal(bob().address(), None, b"secret two", NOW)
        .unwrap();
    assert_eq!(
        index(&lost),
        index(&next),
        "a rewind reuses the message key"
    );
}

#[test]
fn keeping_the_advanced_chain_delivers_both_in_any_order() {
    let (mut a, mut b, _saved) = established();
    let maybe_lost = a.seal(bob().address(), None, b"one", NOW).unwrap();
    let next = a.seal(bob().address(), None, b"two", NOW).unwrap();
    assert_ne!(index(&maybe_lost), index(&next));
    // The relay did keep the first after all, and it arrives late.
    assert_eq!(b.open(&next, NOW).unwrap().text, b"two");
    assert_eq!(b.open(&maybe_lost, NOW).unwrap().text, b"one");
}

#[test]
fn a_first_message_that_failed_can_be_reopened_with_a_fresh_handshake() {
    // Rewinding is safe only before a session exists: the retry initiates
    // again with new ephemeral keys, so no key is reused.
    let mut a = Client::new(alice());
    let bundle = bob().prekey_bundle(1, NOW + DAY).unwrap();
    let saved = a.save().unwrap();
    let _lost = a.seal(bob().address(), Some(&bundle), b"hi", NOW).unwrap();
    let mut retry = Client::restore(alice(), &saved).unwrap();
    assert!(!retry.has_session(&bob().address()));
    let open = retry
        .seal(bob().address(), Some(&bundle), b"hi", NOW)
        .unwrap();
    let mut b = Client::new(bob());
    assert_eq!(b.open(&open, NOW).unwrap().text, b"hi");
}
