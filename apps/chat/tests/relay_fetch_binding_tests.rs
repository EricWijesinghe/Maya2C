//! A mailbox fetch is bound to one relay: a signature made for one relay is
//! refused by another, so a relay cannot replay it elsewhere.
#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use maya_chat::ChatError;
use maya_chat::identity::Identity;
use maya_chat::relay::{Envelope, Payload, Relay, fetch_bytes};
use maya_chat::session::Message;

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 24 * 3_600;

fn stocked(id: &[u8], to: [u8; 32]) -> Relay {
    let mut relay = Relay::new(0).with_id(id.to_vec());
    let envelope = Envelope {
        to,
        expires_at: NOW + DAY,
        payload: Payload::Chat(Message {
            session: [0; 32],
            n: 0,
            ciphertext: vec![0; 8],
        }),
        stamp: 0,
        version: 1,
    };
    relay.deposit(envelope, NOW).unwrap();
    relay
}

#[test]
fn a_signature_for_one_relay_is_refused_by_another() {
    let bob = Identity::from_seed([2; 32]);
    let mut relay_b = stocked(b"relay-b", bob.address());
    // Relay A passes relay B's nonce off as its own; Bob signs for A.
    let nonce = relay_b.challenge(bob.address(), NOW).unwrap();
    let for_a = bob
        .sign(&fetch_bytes(b"relay-a", &bob.address(), &nonce))
        .unwrap();
    assert_eq!(
        relay_b.fetch(bob.address(), &nonce, bob.public_key(), &for_a, NOW),
        Err(ChatError::BadSignature)
    );
    // Bob's mail is still at B for Bob, with a signature naming B.
    let nonce = relay_b.challenge(bob.address(), NOW).unwrap();
    let for_b = bob
        .sign(&fetch_bytes(relay_b.id(), &bob.address(), &nonce))
        .unwrap();
    assert_eq!(
        relay_b
            .fetch(bob.address(), &nonce, bob.public_key(), &for_b, NOW)
            .unwrap()
            .len(),
        1
    );
}
