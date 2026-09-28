//! Sessions and clients: what ADR-031 promises about one conversation.
#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use maya_chat::ChatError;
use maya_chat::client::Client;
use maya_chat::identity::Identity;
use maya_chat::session::{self, MAX_CLOCK_SKEW, MAX_HANDSHAKE_AGE, MAX_SKIP};

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 24 * 3_600;

fn pair() -> (Identity, Identity) {
    (Identity::from_seed([1; 32]), Identity::from_seed([2; 32]))
}

fn sessions() -> (session::Session, session::Session) {
    let (alice, bob) = pair();
    let bundle = bob.prekey_bundle(1, NOW + DAY).expect("bundle");
    let (a, hs) = session::initiate(&alice, &bundle, NOW).expect("initiate");
    let (b, from) = session::accept(&bob, &hs, NOW).expect("accept");
    assert_eq!(from, alice.address());
    assert_eq!(a.id(), b.id());
    (a, b)
}

#[test]
fn messages_round_trip_both_ways() {
    let (mut a, mut b) = sessions();
    for i in 0..5u8 {
        let m = a.encrypt(&[i; 3]).unwrap();
        assert_eq!(b.decrypt(&m).unwrap(), vec![i; 3]);
        let r = b.encrypt(&[i + 100]).unwrap();
        assert_eq!(a.decrypt(&r).unwrap(), vec![i + 100]);
    }
}

#[test]
fn out_of_order_messages_decrypt_once_each() {
    let (mut a, mut b) = sessions();
    let ms: Vec<_> = (0..4u8).map(|i| a.encrypt(&[i]).unwrap()).collect();
    assert_eq!(b.decrypt(&ms[3]).unwrap(), vec![3]);
    assert_eq!(b.decrypt(&ms[1]).unwrap(), vec![1]);
    assert_eq!(b.decrypt(&ms[0]).unwrap(), vec![0]);
    assert_eq!(b.decrypt(&ms[2]).unwrap(), vec![2]);
    for m in &ms {
        assert_eq!(b.decrypt(m), Err(ChatError::Replayed));
    }
}

#[test]
fn a_replay_is_refused() {
    let (mut a, mut b) = sessions();
    let m = a.encrypt(b"once").unwrap();
    b.decrypt(&m).unwrap();
    assert_eq!(b.decrypt(&m), Err(ChatError::Replayed));
}

#[test]
fn a_tampered_message_is_refused_and_changes_nothing() {
    let (mut a, mut b) = sessions();
    let m = a.encrypt(b"hello").unwrap();
    let mut bad = m.clone();
    bad.ciphertext[0] ^= 1;
    assert_eq!(b.decrypt(&bad), Err(ChatError::Decrypt));
    // A forged number far ahead must not advance the chain either.
    let mut ahead = m.clone();
    ahead.n = 50;
    assert_eq!(b.decrypt(&ahead), Err(ChatError::Decrypt));
    assert_eq!(b.decrypt(&m).unwrap(), b"hello");
}

#[test]
fn too_far_ahead_is_refused() {
    let (mut a, mut b) = sessions();
    let mut m = a.encrypt(b"x").unwrap();
    m.n = MAX_SKIP + 1;
    assert_eq!(b.decrypt(&m), Err(ChatError::TooFarAhead));
}

#[test]
fn a_message_for_another_session_is_refused() {
    let (mut a, _) = sessions();
    let (_, mut other) = sessions();
    let m = a.encrypt(b"x").unwrap();
    // Same identities, fresh encapsulation: a different session.
    assert_eq!(other.decrypt(&m), Err(ChatError::WrongSession));
}

#[test]
fn a_handshake_for_someone_else_is_refused() {
    let (alice, bob) = pair();
    let carol = Identity::from_seed([3; 32]);
    let bundle = bob.prekey_bundle(1, NOW + DAY).unwrap();
    let (_, hs) = session::initiate(&alice, &bundle, NOW).unwrap();
    assert_eq!(
        session::accept(&carol, &hs, NOW).err(),
        Some(ChatError::NotForMe)
    );
}

#[test]
fn stale_and_future_handshakes_are_refused() {
    let (alice, bob) = pair();
    let bundle = bob.prekey_bundle(1, NOW + DAY).unwrap();
    let (_, hs) = session::initiate(&alice, &bundle, NOW).unwrap();
    let late = NOW + MAX_HANDSHAKE_AGE + 1;
    assert_eq!(
        session::accept(&bob, &hs, late).err(),
        Some(ChatError::Expired)
    );
    let early = NOW - MAX_CLOCK_SKEW - 1;
    assert_eq!(
        session::accept(&bob, &hs, early).err(),
        Some(ChatError::Expired)
    );
}

#[test]
fn a_forged_handshake_is_refused() {
    let (alice, bob) = pair();
    let mallory = Identity::from_seed([9; 32]);
    let bundle = bob.prekey_bundle(1, NOW + DAY).unwrap();
    let (_, mut hs) = session::initiate(&alice, &bundle, NOW).unwrap();
    // Claiming to be Mallory with Alice's signature.
    hs.initiator_key = mallory.public_key().to_vec();
    assert_eq!(
        session::accept(&bob, &hs, NOW).err(),
        Some(ChatError::BadSignature)
    );
}

#[test]
fn a_forged_or_expired_bundle_is_refused() {
    let (alice, bob) = pair();
    let mut bundle = bob.prekey_bundle(1, NOW + DAY).unwrap();
    assert_eq!(
        session::initiate(&alice, &bundle, NOW + DAY).err(),
        Some(ChatError::Expired)
    );
    // Swap in an attacker's prekey: the signature no longer covers it.
    bundle.prekey = Identity::from_seed([9; 32])
        .prekey_bundle(1, NOW + DAY)
        .unwrap()
        .prekey;
    assert_eq!(
        session::initiate(&alice, &bundle, NOW).err(),
        Some(ChatError::BadSignature)
    );
}

#[test]
fn identities_are_deterministic_and_prekeys_differ_by_epoch() {
    let a = Identity::from_seed([7; 32]);
    let b = Identity::from_seed([7; 32]);
    assert_eq!(a.address(), b.address());
    assert_eq!(a.prekey(1).1, b.prekey(1).1);
    assert_ne!(a.prekey(1).1, a.prekey(2).1);
}

#[test]
fn clients_converse_through_envelopes() {
    let (alice, bob) = pair();
    let bundle = bob.prekey_bundle(1, NOW + DAY).unwrap();
    let (alice_addr, bob_addr) = (alice.address(), bob.address());
    let mut a = Client::new(alice);
    let mut b = Client::new(bob);

    assert_eq!(
        a.seal(bob_addr, None, b"hi", NOW).err(),
        Some(ChatError::NoSession)
    );
    let first = a.seal(bob_addr, Some(&bundle), b"hi bob", NOW).unwrap();
    let got = b.open(&first, NOW).unwrap();
    assert_eq!(
        (got.from, got.text.as_slice()),
        (alice_addr, &b"hi bob"[..])
    );
    assert!(b.has_session(&alice_addr));

    let reply = b.seal(alice_addr, None, b"hi alice", NOW).unwrap();
    assert_eq!(a.open(&reply, NOW).unwrap().text, b"hi alice");
    let second = a.seal(bob_addr, None, b"again", NOW).unwrap();
    assert_eq!(b.open(&second, NOW).unwrap().text, b"again");

    // Not addressed to Alice.
    assert_eq!(a.open(&second, NOW).err(), Some(ChatError::NotForMe));
}

#[test]
fn a_replayed_open_cannot_reset_a_live_session() {
    let (alice, bob) = pair();
    let bundle = bob.prekey_bundle(1, NOW + DAY).unwrap();
    let bob_addr = bob.address();
    let mut a = Client::new(alice);
    let mut b = Client::new(bob);
    let first = a.seal(bob_addr, Some(&bundle), b"hi", NOW).unwrap();
    b.open(&first, NOW).unwrap();
    let second = a.seal(bob_addr, None, b"two", NOW).unwrap();
    b.open(&second, NOW).unwrap();

    assert_eq!(b.open(&first, NOW + 10).err(), Some(ChatError::Replayed));
    // The live session is untouched.
    let third = a.seal(bob_addr, None, b"three", NOW).unwrap();
    assert_eq!(b.open(&third, NOW).unwrap().text, b"three");
}

#[test]
fn a_new_handshake_rekeys_the_session() {
    let (alice, bob) = pair();
    let bundle = bob.prekey_bundle(2, NOW + DAY).unwrap();
    let bob_addr = bob.address();
    let mut b = Client::new(bob);
    let mut a1 = Client::new(Identity::from_seed(*alice.seed()));
    let mut a2 = Client::new(alice);
    b.open(&a1.seal(bob_addr, Some(&bundle), b"old", NOW).unwrap(), NOW)
        .unwrap();
    // Alice on a new device (same identity, no session state) re-keys.
    let fresh = a2.seal(bob_addr, Some(&bundle), b"new", NOW + 1).unwrap();
    assert_eq!(b.open(&fresh, NOW + 1).unwrap().text, b"new");
    // The old session's messages no longer find a session.
    let stale = a1.seal(bob_addr, None, b"old two", NOW + 2).unwrap();
    assert_eq!(b.open(&stale, NOW + 2).err(), Some(ChatError::NoSession));
}
