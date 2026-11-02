//! Sender outbox and delivery receipts, driven in-process through a [`Relay`]
//! (ADR-074, test plan items 2, 3, 5, 6 and 7, and the lost-reply retry).
#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use maya_chat::ChatError;
use maya_chat::client::{Client, ENVELOPE_TTL, Frame};
use maya_chat::identity::Identity;
use maya_chat::outbox::{MAX_ENTRIES, MAX_EXPIRED, RECEIPT_DELAY_MAX};
use maya_chat::receipt;
use maya_chat::relay::{Envelope, MAX_ENVELOPE_BYTES, Payload, Relay, fetch_bytes};
use maya_chat::session;

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 24 * 3_600;
const ONE_BIT: u32 = 1;

fn alice() -> Identity {
    Identity::from_seed([1; 32])
}
fn bob() -> Identity {
    Identity::from_seed([2; 32])
}

/// Alice queues `text` for Bob at version 1, opening the session if needed.
fn queue(a: &mut Client, text: &[u8], now: u64) -> [u8; 32] {
    let bundle = bob().prekey_bundle(1, NOW + DAY).unwrap();
    let bundle = (!a.has_session(&bob().address())).then_some(bundle);
    a.queue(bob().address(), bundle.as_ref(), text, now, 1)
        .unwrap()
}

/// One attempt for `id`: a deposit the relay keeps, or a lost request.
fn attempt(relay: &mut Relay, client: &mut Client, id: &[u8; 32], now: u64, lost: bool) {
    let envelope = client.envelope_for_send(id, ONE_BIT).unwrap();
    if lost {
        client.failed(id, now).unwrap();
    } else {
        relay.deposit(envelope, now).unwrap();
        client.sent(id, now).unwrap();
    }
}

/// Empties `who`'s mailbox at `relay` and opens everything in it.
fn collect(relay: &mut Relay, who: &mut Client, now: u64) -> Vec<Frame> {
    let address = who.identity().address();
    let nonce = relay.challenge(address, now).unwrap();
    let signature = who.identity().sign(&fetch_bytes(&address, &nonce)).unwrap();
    let envelopes = relay
        .fetch(address, who.identity().public_key(), &signature, now)
        .unwrap();
    let frames = envelopes
        .iter()
        .map(|e| who.open_frame(e, now).unwrap())
        .collect();
    who.end_pass(now).unwrap();
    frames
}

/// Bob's side of the session Alice opened with `open`: his own session,
/// built from the same handshake, sending what Bob would send.
fn bob_session_for(open: &Envelope) -> session::Session {
    let Payload::Open { handshake, .. } = &open.payload else {
        panic!("the first message opens a session");
    };
    session::accept(&bob(), handshake, NOW).unwrap().0
}

#[test]
fn a_lost_message_is_retried_with_the_same_bytes_and_then_receipted() {
    let mut relay = Relay::new(ONE_BIT);
    let (mut a, mut b) = (Client::new(alice()), Client::new(bob()));
    let id = queue(&mut a, b"hello", NOW);

    // First attempt: the request never reached the relay.
    attempt(&mut relay, &mut a, &id, NOW, true);
    let first = a.pending(&id).unwrap().envelope.encode().unwrap();
    assert!(a.due(NOW).is_empty(), "backoff delays the retry");

    // The retry is due after the backoff, and sends identical bytes.
    let later = NOW + 10 * 60;
    attempt(&mut relay, &mut a, &id, later, false);
    assert!(a.pending(&id).unwrap().relayed);
    assert_eq!(
        a.pending(&id).unwrap().envelope.encode().unwrap(),
        first,
        "a retry never reseals"
    );

    // Bob collects the message; his receipt is queued with a random delay.
    let got = collect(&mut relay, &mut b, later);
    assert!(matches!(&got[..], [Frame::Text(r)] if r.text == b"hello"));
    assert_eq!(b.outbox().count(), 1, "the receipt is queued");
    let receipt_due = later + RECEIPT_DELAY_MAX;
    for rid in b.due(receipt_due) {
        attempt(&mut relay, &mut b, &rid, receipt_due, false);
    }

    // Alice collects the receipt, and her entry leaves the outbox.
    let frames = collect(&mut relay, &mut a, receipt_due);
    assert_eq!(
        frames,
        vec![Frame::Receipt {
            from: bob().address(),
            ids: vec![id]
        }]
    );
    assert!(a.pending(&id).is_none(), "receipted, so out of the outbox");
}

#[test]
fn a_duplicate_envelope_is_suppressed_at_the_relay_and_the_receiver() {
    let mut relay = Relay::new(ONE_BIT);
    let (mut a, mut b) = (Client::new(alice()), Client::new(bob()));
    let id = queue(&mut a, b"once", NOW);
    let envelope = a.envelope_for_send(&id, ONE_BIT).unwrap();

    relay.deposit(envelope.clone(), NOW).unwrap();
    let once = relay.stored_bytes();
    relay.deposit(envelope.clone(), NOW).unwrap();
    assert_eq!(relay.stored_bytes(), once, "the relay keeps one copy");

    let got = collect(&mut relay, &mut b, NOW);
    assert_eq!(got.len(), 1, "one message, not two");

    // The same envelope arriving again is a replay the session refuses.
    assert_eq!(b.open_frame(&envelope, NOW), Err(ChatError::Replayed));
}

#[test]
fn a_forged_receipt_is_refused_and_changes_nothing() {
    let mut a = Client::new(alice());
    let bundle = bob().prekey_bundle(1, NOW + DAY).unwrap();
    let id = a
        .queue(bob().address(), Some(&bundle), b"hi", NOW, 1)
        .unwrap();
    let open = a.envelope_for_send(&id, ONE_BIT).unwrap();
    let mut bob_side = bob_session_for(&open);

    // Mallory signs a receipt for Alice's id; Bob's session carries it.
    let mallory = Identity::from_seed([3; 32]);
    let ids = [id];
    let session_id = bob_side.id();
    let statement = receipt::digest(&session_id, &bob().address(), &alice().address(), &ids);
    let forged = receipt::encode(&ids, &mallory.sign(&statement).unwrap());
    let envelope = chat_envelope(bob_side.encrypt(&forged).unwrap());

    assert_eq!(a.open_frame(&envelope, NOW), Err(ChatError::BadSignature));
    assert!(a.pending(&id).is_some(), "the entry is still queued");
}

#[test]
fn a_genuine_receipt_for_an_id_not_queued_is_ignored() {
    let mut a = Client::new(alice());
    let bundle = bob().prekey_bundle(1, NOW + DAY).unwrap();
    let id = a
        .queue(bob().address(), Some(&bundle), b"hi", NOW, 1)
        .unwrap();
    let open = a.envelope_for_send(&id, ONE_BIT).unwrap();
    let mut bob_side = bob_session_for(&open);

    // Bob signs a receipt for an id Alice never sent: valid, and a no-op.
    let stranger = [[9; 32]];
    let statement = receipt::digest(
        &bob_side.id(),
        &bob().address(),
        &alice().address(),
        &stranger,
    );
    let signed = receipt::encode(&stranger, &bob().sign(&statement).unwrap());
    let envelope = chat_envelope(bob_side.encrypt(&signed).unwrap());

    assert_eq!(
        a.open_frame(&envelope, NOW),
        Ok(Frame::Receipt {
            from: bob().address(),
            ids: vec![]
        })
    );
    assert!(a.pending(&id).is_some());
}

fn chat_envelope(message: maya_chat::session::Message) -> Envelope {
    Envelope {
        to: alice().address(),
        expires_at: NOW + DAY,
        payload: Payload::Chat(message),
        stamp: 0,
        version: 1,
    }
}

#[test]
fn receipts_switched_off_send_none_but_messages_still_arrive() {
    let mut relay = Relay::new(ONE_BIT);
    let (mut a, mut b) = (Client::new(alice()), Client::new(bob()));
    b.set_receipts(false);
    let id = queue(&mut a, b"quiet", NOW);
    attempt(&mut relay, &mut a, &id, NOW, false);
    let got = collect(&mut relay, &mut b, NOW);
    assert!(matches!(&got[..], [Frame::Text(r)] if r.text == b"quiet"));
    assert_eq!(b.outbox().count(), 0, "no receipt queued");
}

#[test]
fn a_per_contact_switch_suppresses_receipts_for_that_contact_only() {
    let mut relay = Relay::new(ONE_BIT);
    let (mut a, mut b) = (Client::new(alice()), Client::new(bob()));
    b.set_contact_receipts(alice().address(), false);
    let id = queue(&mut a, b"quiet", NOW);
    attempt(&mut relay, &mut a, &id, NOW, false);
    collect(&mut relay, &mut b, NOW);
    assert_eq!(b.outbox().count(), 0);
}

#[test]
fn text_that_starts_like_a_receipt_is_refused_before_anything_is_sealed() {
    let mut a = Client::new(alice());
    let bundle = bob().prekey_bundle(1, NOW + DAY).unwrap();
    let text = [receipt::MAGIC, b"x".as_slice()].concat();
    assert!(
        a.queue(bob().address(), Some(&bundle), &text, NOW, 1)
            .is_err()
    );
    assert!(!a.has_session(&bob().address()), "no session left behind");
}

#[test]
fn the_outbox_refuses_entry_1001_and_evicts_nothing() {
    let mut a = Client::new(alice());
    let first = queue(&mut a, b"0", NOW);
    for i in 1..MAX_ENTRIES {
        a.queue(bob().address(), None, format!("{i}").as_bytes(), NOW, 1)
            .unwrap();
    }
    assert_eq!(a.outbox().count(), MAX_ENTRIES);
    assert_eq!(
        a.queue(bob().address(), None, b"overflow", NOW, 1),
        Err(ChatError::OutboxFull(MAX_ENTRIES))
    );
    assert!(a.pending(&first).is_some(), "the oldest entry stays");
    assert_eq!(a.outbox().count(), MAX_ENTRIES);
}

#[test]
fn an_oversized_message_is_refused_and_leaves_no_session_behind() {
    let mut a = Client::new(alice());
    let bundle = bob().prekey_bundle(1, NOW + DAY).unwrap();
    let big = vec![b'x'; MAX_ENVELOPE_BYTES];
    assert!(
        a.queue(bob().address(), Some(&bundle), &big, NOW, 1)
            .is_err()
    );
    assert!(!a.has_session(&bob().address()));
    assert_eq!(a.outbox().count(), 0);
}

#[test]
fn a_queued_message_survives_a_restart_and_is_sent_with_the_same_id() {
    let mut relay = Relay::new(ONE_BIT);
    let mut a = Client::new(alice());
    let id = queue(&mut a, b"persist me", NOW);
    let saved = a.save().unwrap();
    drop(a);

    let mut a = Client::restore(alice(), &saved).unwrap();
    assert_eq!(a.due(NOW), vec![id]);
    attempt(&mut relay, &mut a, &id, NOW, false);
    assert!(
        a.pending(&id).unwrap().relayed,
        "still queued for its receipt"
    );
}

#[test]
fn an_entry_past_its_ttl_moves_to_the_expired_record_and_is_dismissable() {
    let mut a = Client::new(alice());
    let id = queue(&mut a, b"late", NOW);
    let week = NOW + ENVELOPE_TTL;
    assert!(a.due(week).is_empty(), "never retried once expired");
    a.sweep(week);
    assert!(a.pending(&id).is_none());
    assert_eq!(
        a.expired_entries().map(|(i, _)| *i).collect::<Vec<_>>(),
        vec![id]
    );
    assert!(a.dismiss(&id));
    assert_eq!(a.expired_entries().count(), 0);
}

#[test]
fn a_thousand_expired_entries_do_not_lock_out_sending() {
    let mut a = Client::new(alice());
    // Queued one second apart, so the expiry order is known.
    let first = queue(&mut a, b"0", NOW);
    let mut ids = vec![first];
    for i in 1..MAX_ENTRIES {
        let at = NOW + i as u64;
        ids.push(
            a.queue(bob().address(), None, format!("{i}").as_bytes(), at, 1)
                .unwrap(),
        );
    }
    assert_eq!(a.outbox().count(), MAX_ENTRIES);

    // Every one has expired by then; a new send must still go through.
    let later = NOW + ENVELOPE_TTL + MAX_ENTRIES as u64;
    let fresh = a.queue(bob().address(), None, b"after", later, 1).unwrap();
    assert_eq!(a.outbox().count(), 1, "only the new send is pending");
    assert!(a.pending(&fresh).is_some());
    assert_eq!(
        a.expired_entries().count(),
        MAX_EXPIRED,
        "the newest are kept"
    );
    assert!(
        a.expired_entries().all(|(id, _)| *id != first),
        "oldest dropped"
    );
    assert!(a.expired_entries().any(|(id, _)| id == ids.last().unwrap()));
}
