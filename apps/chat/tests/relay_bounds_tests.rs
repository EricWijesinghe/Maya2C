//! Resource bounds on a relay and a client: what anyone can send must not
//! hold memory or lock owners out.
#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use maya_chat::ChatError;
use maya_chat::identity::{Identity, PrekeyBundle};
use maya_chat::relay::{Envelope, Payload, Relay};
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
