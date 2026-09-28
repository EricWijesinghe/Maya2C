//! Relays: prekey directory, mailbox quotas, expiry and owner-only fetch.
#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use maya_chat::ChatError;
use maya_chat::client::Client;
use maya_chat::identity::Identity;
use maya_chat::relay::{
    CHALLENGE_TTL, Envelope, MAX_ENVELOPE_BYTES, MAX_ENVELOPES, MAX_TTL, Payload, Relay,
    fetch_bytes,
};
use maya_chat::session::Message;

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 24 * 3_600;

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
    }
}

fn fetch(relay: &mut Relay, who: &Identity, now: u64) -> Result<Vec<Envelope>, ChatError> {
    let nonce = relay.challenge(who.address(), now)?;
    let sig = who.sign(&fetch_bytes(&who.address(), &nonce))?;
    relay.fetch(who.address(), who.public_key(), &sig, now)
}

#[test]
fn a_bundle_is_published_and_served_until_expiry() {
    let bob = Identity::from_seed([2; 32]);
    let mut relay = Relay::new(0);
    let bundle = bob.prekey_bundle(1, NOW + DAY).unwrap();
    assert_eq!(relay.publish(bundle.clone(), NOW).unwrap(), bob.address());
    assert_eq!(relay.bundle(&bob.address(), NOW), Some(bundle));
    assert_eq!(relay.bundle(&bob.address(), NOW + DAY), None);
}

#[test]
fn an_older_or_forged_bundle_is_refused() {
    let bob = Identity::from_seed([2; 32]);
    let mut relay = Relay::new(0);
    relay
        .publish(bob.prekey_bundle(5, NOW + DAY).unwrap(), NOW)
        .unwrap();
    assert!(matches!(
        relay.publish(bob.prekey_bundle(4, NOW + DAY).unwrap(), NOW),
        Err(ChatError::Refused(_))
    ));
    let mut forged = bob.prekey_bundle(6, NOW + DAY).unwrap();
    forged.expires_at += 1;
    assert_eq!(relay.publish(forged, NOW), Err(ChatError::BadSignature));
}

#[test]
fn deposit_is_idempotent_and_fetch_empties_the_mailbox() {
    let bob = Identity::from_seed([2; 32]);
    let mut relay = Relay::new(0);
    let e = envelope(bob.address(), 0, 10, NOW + DAY);
    let id = relay.deposit(e.clone(), NOW).unwrap();
    assert_eq!(relay.deposit(e.clone(), NOW).unwrap(), id);
    assert_eq!(fetch(&mut relay, &bob, NOW).unwrap(), vec![e]);
    assert_eq!(fetch(&mut relay, &bob, NOW).unwrap(), vec![]);
    assert_eq!(relay.stored_bytes(), 0);
}

#[test]
fn size_and_expiry_limits_hold() {
    let to = [2; 32];
    let mut relay = Relay::new(0);
    let refused = |r: Result<[u8; 32], ChatError>| matches!(r, Err(ChatError::Refused(_)));
    assert!(refused(
        relay.deposit(envelope(to, 0, MAX_ENVELOPE_BYTES, NOW + DAY), NOW)
    ));
    assert!(refused(relay.deposit(envelope(to, 0, 1, NOW), NOW)));
    assert!(refused(
        relay.deposit(envelope(to, 0, 1, NOW + MAX_TTL + 1), NOW)
    ));
}

#[test]
fn a_full_mailbox_refuses_until_its_envelopes_expire() {
    let to = [2; 32];
    let mut relay = Relay::new(0);
    for n in 0..MAX_ENVELOPES as u64 {
        relay.deposit(envelope(to, n, 1, NOW + DAY), NOW).unwrap();
    }
    let extra = envelope(to, u64::MAX, 1, NOW + 2 * DAY);
    assert!(matches!(
        relay.deposit(extra.clone(), NOW),
        Err(ChatError::Refused(_))
    ));
    // A day later the first batch has expired and the space is back.
    relay.deposit(extra, NOW + DAY).unwrap();
}

#[test]
fn only_the_owner_can_fetch() {
    let bob = Identity::from_seed([2; 32]);
    let mallory = Identity::from_seed([9; 32]);
    let mut relay = Relay::new(0);
    relay
        .deposit(envelope(bob.address(), 0, 1, NOW + DAY), NOW)
        .unwrap();

    // No challenge issued.
    assert!(matches!(
        relay.fetch(bob.address(), bob.public_key(), &[], NOW),
        Err(ChatError::Refused(_))
    ));
    // Mallory's own key for Bob's address.
    let nonce = relay.challenge(bob.address(), NOW).unwrap();
    let sig = mallory.sign(&fetch_bytes(&bob.address(), &nonce)).unwrap();
    assert_eq!(
        relay.fetch(bob.address(), mallory.public_key(), &sig, NOW),
        Err(ChatError::BadSignature)
    );
    // Bob's key with a signature over something else; the challenge was spent.
    let nonce = relay.challenge(bob.address(), NOW).unwrap();
    let sig = bob.sign(&fetch_bytes(&bob.address(), &[0; 32])).unwrap();
    assert_ne!(nonce, [0; 32]);
    assert_eq!(
        relay.fetch(bob.address(), bob.public_key(), &sig, NOW),
        Err(ChatError::BadSignature)
    );
    // A stale challenge.
    let nonce = relay.challenge(bob.address(), NOW).unwrap();
    let sig = bob.sign(&fetch_bytes(&bob.address(), &nonce)).unwrap();
    assert_eq!(
        relay.fetch(
            bob.address(),
            bob.public_key(),
            &sig,
            NOW + CHALLENGE_TTL + 1
        ),
        Err(ChatError::Expired)
    );
    assert_eq!(fetch(&mut relay, &bob, NOW).unwrap().len(), 1);
}

#[test]
fn a_conversation_passes_through_a_relay() {
    let alice = Identity::from_seed([1; 32]);
    let bob = Identity::from_seed([2; 32]);
    let bob_id = Identity::from_seed([2; 32]);
    let mut relay = Relay::new(0);
    relay
        .publish(bob.prekey_bundle(1, NOW + DAY).unwrap(), NOW)
        .unwrap();
    let bundle = relay.bundle(&bob.address(), NOW).unwrap();

    let mut a = Client::new(alice);
    for text in ["one", "two", "three"] {
        let e = a
            .seal(bob.address(), Some(&bundle), text.as_bytes(), NOW)
            .unwrap();
        relay.deposit(e, NOW).unwrap();
    }
    let mut b = Client::new(bob);
    let texts: Vec<_> = fetch(&mut relay, &bob_id, NOW)
        .unwrap()
        .iter()
        .map(|e| String::from_utf8(b.open(e, NOW).unwrap().text).unwrap())
        .collect();
    assert_eq!(texts, ["one", "two", "three"]);
}

#[test]
fn a_deposit_without_enough_postage_is_refused() {
    let to = [2; 32];
    let bits = 12;
    let mut relay = Relay::new(bits);
    let bare = envelope(to, 0, 10, NOW + DAY);
    assert!(matches!(
        relay.deposit(bare.clone(), NOW),
        Err(ChatError::Refused(_))
    ));
    let stamped = bare.clone().mint(bits).unwrap();
    assert!(stamped.stamped(bits).unwrap());
    let id = relay.deposit(stamped, NOW).unwrap();
    // The id ignores the stamp, so a resend with new postage is a duplicate.
    assert_eq!(bare.id().unwrap(), id);
    let again = bare.mint(bits + 1).unwrap();
    assert_eq!(relay.deposit(again, NOW).unwrap(), id);
}

#[test]
fn the_default_relay_asks_for_the_default_postage() {
    assert_eq!(Relay::default().stamp_bits(), maya_chat::relay::STAMP_BITS);
}
