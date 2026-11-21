//! Resource bounds on a relay: what anyone can send must not hold memory or
//! lock owners out.
#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use maya_chat::ChatError;
use maya_chat::identity::{Identity, PrekeyBundle};
use maya_chat::relay::{Envelope, MAX_CHALLENGES, Payload, Relay, fetch_bytes};
use maya_chat::session::Message;

const NOW: u64 = 1_800_000_000;
const HOUR: u64 = 3_600;
const DAY: u64 = 24 * HOUR;

fn envelope(to: [u8; 32], n: u64, len: usize, expires_at: u64) -> Envelope {
    Envelope {
        to,
        expires_at,
        payload: Payload::Chat(Message {
            session: [0; 32],
            n,
            ciphertext: vec![0; len],
        }),
        stamp: 0,
        version: 1,
    }
}

/// A bundle signed correctly but carrying `prekey` instead of a real one.
fn bundle_with_prekey(who: &Identity, prekey: Vec<u8>, expires_at: u64) -> PrekeyBundle {
    let put = |out: &mut Vec<u8>, bytes: &[u8]| {
        out.extend_from_slice(&u32::try_from(bytes.len()).unwrap().to_le_bytes());
        out.extend_from_slice(bytes);
    };
    let mut signing = b"maya-chat prekey bundle v1".to_vec();
    put(&mut signing, who.public_key());
    signing.extend_from_slice(&1u32.to_le_bytes());
    put(&mut signing, &prekey);
    signing.extend_from_slice(&expires_at.to_le_bytes());
    PrekeyBundle {
        identity_key: who.public_key().to_vec(),
        epoch: 1,
        prekey,
        expires_at,
        signature: who.sign(&signing).unwrap(),
    }
}

#[test]
fn an_oversized_prekey_is_refused_even_when_signed() {
    let bob = Identity::from_seed([2; 32]);
    let mut relay = Relay::new(0);
    let big = bundle_with_prekey(&bob, vec![7; 500_000], NOW + DAY);
    assert_eq!(relay.publish(big, NOW), Err(ChatError::BadKey));
    assert_eq!(relay.bundle(&bob.address(), NOW), None);
    // The same construction with the real prekey is accepted, so the
    // refusal above is the length and not the test's signing.
    let (_, real) = bob.prekey(1);
    relay
        .publish(bundle_with_prekey(&bob, real, NOW + DAY), NOW)
        .unwrap();
}

#[test]
fn a_full_challenge_table_does_not_lock_the_owner_out() {
    let bob = Identity::from_seed([2; 32]);
    let mut relay = Relay::new(0);
    relay
        .deposit(envelope(bob.address(), 0, 1, NOW + DAY), NOW)
        .unwrap();
    for i in 0..MAX_CHALLENGES {
        let mut stranger = [0xEE; 32];
        stranger[..8].copy_from_slice(&(i as u64).to_le_bytes());
        relay.challenge(stranger, NOW).unwrap();
    }
    let nonce = relay.challenge(bob.address(), NOW).unwrap();
    let sig = bob
        .sign(&fetch_bytes(relay.id(), &bob.address(), &nonce))
        .unwrap();
    let got = relay
        .fetch(bob.address(), bob.public_key(), &sig, NOW)
        .unwrap();
    assert_eq!(got.len(), 1);
}

#[test]
fn reissuing_a_challenge_in_a_full_table_evicts_nobody() {
    let first = Identity::from_seed([3; 32]);
    let bob = Identity::from_seed([2; 32]);
    let mut relay = Relay::new(0);
    relay.challenge(first.address(), NOW).unwrap();
    for i in 1..MAX_CHALLENGES - 1 {
        let mut stranger = [0xEE; 32];
        stranger[..8].copy_from_slice(&(i as u64).to_le_bytes());
        relay.challenge(stranger, NOW + 1).unwrap();
    }
    relay.challenge(bob.address(), NOW + 1).unwrap();
    // The table is full; Bob asking again must not push out the oldest.
    relay.challenge(bob.address(), NOW + 2).unwrap();
    let wrong = first.sign(b"not the challenge").unwrap();
    assert_eq!(
        relay.fetch(first.address(), first.public_key(), &wrong, NOW + 2),
        Err(ChatError::BadSignature),
        "the oldest challenge must still be held"
    );
}

#[test]
fn expired_envelopes_in_every_mailbox_stop_counting_at_the_next_deposit() {
    let mut relay = Relay::new(0);
    for i in 0..50u8 {
        relay
            .deposit(envelope([i; 32], 0, 1_000, NOW + HOUR), NOW)
            .unwrap();
    }
    assert!(relay.stored_bytes() > 50_000);
    let late = envelope([200; 32], 0, 10, NOW + DAY);
    let late_len = late.encode().unwrap().len();
    relay.deposit(late, NOW + 2 * HOUR).unwrap();
    assert_eq!(relay.stored_bytes(), late_len);
}
