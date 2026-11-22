//! Resource bounds on a relay: what anyone can send must not hold memory or
//! lock owners out.
#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use maya_chat::ChatError;
use maya_chat::identity::{Identity, PrekeyBundle};
use maya_chat::relay::{
    Envelope, MAX_ENVELOPE_BYTES, MAX_ENVELOPES, MAX_FETCHED_IDS, MAX_POSTAGE_BITS,
    POSTAGE_UNIT_BYTES, Payload, Relay, STAMP_BITS, fetch_bytes, postage_bits,
};
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

/// The largest encoded size of a 1,000-byte envelope to any of `who`: the
/// address is part of the encoding, and its length varies with its bytes.
fn slot(who: &[&Identity]) -> usize {
    who.iter()
        .map(|w| {
            envelope(w.address(), 0, 1_000, NOW + DAY)
                .encode()
                .unwrap()
                .len()
        })
        .max()
        .unwrap()
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

fn fetch(relay: &mut Relay, who: &Identity, nonce: &[u8; 32], now: u64) -> Vec<Envelope> {
    let sig = who
        .sign(&fetch_bytes(relay.id(), &who.address(), nonce))
        .unwrap();
    relay
        .fetch(who.address(), nonce, who.public_key(), &sig, now)
        .unwrap()
}

fn fetch_now(relay: &mut Relay, who: &Identity, now: u64) -> Vec<Envelope> {
    let nonce = relay.challenge(who.address(), now).unwrap();
    fetch(relay, who, &nonce, now)
}

fn numbers(envelopes: &[Envelope]) -> Vec<u64> {
    envelopes
        .iter()
        .map(|e| match &e.payload {
            Payload::Chat(m) | Payload::Open { first: m, .. } => m.n,
        })
        .collect()
}

#[test]
fn a_challenge_flood_does_not_push_out_the_owners_pending_one() {
    let bob = Identity::from_seed([2; 32]);
    let mut relay = Relay::new(0);
    relay
        .deposit(envelope(bob.address(), 0, 1, NOW + DAY), NOW)
        .unwrap();
    let nonce = relay.challenge(bob.address(), NOW).unwrap();
    for i in 0..100_000u64 {
        let mut stranger = [0xEE; 32];
        stranger[..8].copy_from_slice(&i.to_le_bytes());
        relay.challenge(stranger, NOW + 1).unwrap();
    }
    assert_eq!(fetch(&mut relay, &bob, &nonce, NOW + 2).len(), 1);
}

#[test]
fn a_full_relay_takes_mail_for_a_smaller_mailbox_from_the_largest() {
    let bob = Identity::from_seed([2; 32]);
    let mallory = Identity::from_seed([9; 32]);
    let size = slot(&[&bob, &mallory]);
    let mut relay = Relay::new(0).with_capacity(3 * size);
    for n in 0..3 {
        relay
            .deposit(envelope(mallory.address(), n, 1_000, NOW + DAY), NOW)
            .unwrap();
    }
    relay
        .deposit(envelope(bob.address(), 0, 1_000, NOW + DAY), NOW)
        .unwrap();
    assert!(relay.stored_bytes() <= 3 * size);
    assert_eq!(numbers(&fetch_now(&mut relay, &bob, NOW)), [0]);
    // The flood's oldest gave way.
    assert_eq!(numbers(&fetch_now(&mut relay, &mallory, NOW)), [1, 2]);
}

#[test]
fn a_flooder_cannot_push_out_a_mailbox_no_larger_than_its_own() {
    let bob = Identity::from_seed([2; 32]);
    let mallory = Identity::from_seed([9; 32]);
    let size = slot(&[&bob, &mallory]);
    let mut relay = Relay::new(0).with_capacity(2 * size);
    relay
        .deposit(envelope(bob.address(), 0, 1_000, NOW + DAY), NOW)
        .unwrap();
    relay
        .deposit(envelope(mallory.address(), 0, 1_000, NOW + DAY), NOW)
        .unwrap();
    assert_eq!(
        relay.deposit(envelope(mallory.address(), 1, 1_000, NOW + DAY), NOW),
        Err(ChatError::Refused("relay full"))
    );
    // A fresh address is no larger than Bob's mailbox either.
    assert_eq!(
        relay.deposit(envelope([0xEE; 32], 0, 1_000, NOW + DAY), NOW),
        Err(ChatError::Refused("relay full"))
    );
    assert_eq!(numbers(&fetch_now(&mut relay, &bob, NOW)), [0]);
}

#[test]
fn an_evicted_envelope_is_stored_again_when_resent() {
    let bob = Identity::from_seed([2; 32]);
    let mallory = Identity::from_seed([9; 32]);
    let first = envelope(mallory.address(), 0, 1_000, NOW + DAY);
    let size = slot(&[&bob, &mallory]);
    let mut relay = Relay::new(0).with_capacity(2 * size);
    relay.deposit(first.clone(), NOW).unwrap();
    relay
        .deposit(envelope(mallory.address(), 1, 1_000, NOW + DAY), NOW)
        .unwrap();
    relay
        .deposit(envelope(bob.address(), 0, 1_000, NOW + DAY), NOW)
        .unwrap();
    fetch_now(&mut relay, &bob, NOW);
    // The outbox resends what drew no receipt; it must not be taken as a
    // duplicate of the envelope that was evicted.
    relay.deposit(first, NOW).unwrap();
    assert_eq!(numbers(&fetch_now(&mut relay, &mallory, NOW)), [1, 0]);
}

#[test]
fn fetched_ids_are_remembered_only_up_to_the_cap() {
    let rounds = MAX_FETCHED_IDS / MAX_ENVELOPES + 2;
    let mut relay = Relay::new(0);
    let mut last = None;
    for round in 0..rounds {
        let who = Identity::from_seed([u8::try_from(round % 256).unwrap(); 32]);
        // Later rounds expire later, so the cap forgets the earliest.
        let expires_at = NOW + DAY + round as u64;
        for n in 0..MAX_ENVELOPES as u64 {
            let e = envelope(who.address(), n, 1, expires_at);
            last = Some(e.clone());
            relay.deposit(e, NOW).unwrap();
        }
        assert_eq!(fetch_now(&mut relay, &who, NOW).len(), MAX_ENVELOPES);
    }
    assert!(relay.remembered_ids() <= MAX_FETCHED_IDS);
    // The newest fetched id is still remembered: its resend stores nothing.
    relay.deposit(last.unwrap(), NOW).unwrap();
    assert_eq!(relay.stored_bytes(), 0);
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

#[test]
fn postage_grows_with_envelope_size_up_to_the_mint_cap() {
    assert_eq!(postage_bits(STAMP_BITS, 1_000), STAMP_BITS);
    assert_eq!(postage_bits(STAMP_BITS, POSTAGE_UNIT_BYTES), STAMP_BITS);
    assert_eq!(
        postage_bits(STAMP_BITS, POSTAGE_UNIT_BYTES + 1),
        STAMP_BITS + 1
    );
    assert_eq!(
        postage_bits(STAMP_BITS, MAX_ENVELOPE_BYTES),
        MAX_POSTAGE_BITS
    );
    // A relay quoting more than the default still accepts a full envelope a
    // sender can mint; one quoting nothing charges nothing.
    assert_eq!(
        postage_bits(STAMP_BITS + 2, MAX_ENVELOPE_BYTES),
        MAX_POSTAGE_BITS
    );
    assert_eq!(postage_bits(0, MAX_ENVELOPE_BYTES), 0);
    assert_eq!(postage_bits(MAX_POSTAGE_BITS + 1, 1), MAX_POSTAGE_BITS + 1);
}

#[test]
fn a_large_envelope_stamped_at_the_base_postage_is_refused() {
    let base = 2;
    let mut relay = Relay::new(base);
    let big = envelope([9; 32], 0, 200_000, NOW + DAY);
    let needed = big.postage(base).unwrap();
    assert_eq!(needed, base + 4, "200 kB is between 8 and 16 postage units");
    // A stamp good enough for a small envelope, short for this one.
    let short = (0..u64::MAX)
        .map(|stamp| Envelope {
            stamp,
            ..big.clone()
        })
        .find(|e| e.stamped(base).unwrap() && !e.stamped(needed).unwrap())
        .unwrap();
    assert!(matches!(
        relay.deposit(short, NOW),
        Err(ChatError::Refused(_))
    ));
    relay.deposit(big.mint(needed).unwrap(), NOW).unwrap();
    // A small envelope still costs only the base.
    let small = envelope([9; 32], 1, 10, NOW + DAY);
    assert_eq!(small.postage(base).unwrap(), base);
    relay.deposit(small.mint(base).unwrap(), NOW).unwrap();
}
