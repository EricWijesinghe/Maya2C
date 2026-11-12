//! Two clients react and reply to each other through a relay, in-process
//! (ADR-140 build PR 1 acceptance). The relay only ever holds envelopes.
#![allow(clippy::unwrap_used, clippy::expect_used)] // a test that fails should panic, and say where

use maya_chat::client::{Client, Frame, Received};
use maya_chat::frame::{self, Content, Mark, Quote};
use maya_chat::identity::Identity;
use maya_chat::relay::{Envelope, Relay, fetch_bytes};

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 24 * 3_600;
const ONE_BIT: u32 = 1;
/// Version 2 writes ciphertext as a byte string, so an envelope's size
/// depends only on the plaintext's length (version 1 varies with the bytes).
const V2: u8 = 2;

fn alice() -> Identity {
    Identity::from_seed([1; 32])
}
fn bob() -> Identity {
    Identity::from_seed([2; 32])
}

/// Queues `plain` from `from` to `to` and deposits it; returns its id and
/// the size of what the relay stored.
fn send(
    relay: &mut Relay,
    from: &mut Client,
    to: &Identity,
    plain: &[u8],
    now: u64,
) -> ([u8; 32], usize) {
    let bundle = to.prekey_bundle(1, NOW + DAY).unwrap();
    let bundle = (!from.has_session(&to.address())).then_some(bundle);
    let id = from
        .queue(to.address(), bundle.as_ref(), plain, now, V2)
        .unwrap();
    let envelope = from.envelope_for_send(&id, ONE_BIT).unwrap();
    // Measured without the stamp, whose CBOR width depends on the work found.
    let size = Envelope {
        stamp: 0,
        ..envelope.clone()
    }
    .encode()
    .unwrap()
    .len();
    relay.deposit(envelope, now).unwrap();
    from.sent(&id, now).unwrap();
    (id, size)
}

/// The messages in `who`'s mailbox (receipts are applied and skipped).
fn collect(relay: &mut Relay, who: &mut Client, now: u64) -> Vec<Received> {
    let address = who.identity().address();
    let nonce = relay.challenge(address, now).unwrap();
    let signature = who.identity().sign(&fetch_bytes(&address, &nonce)).unwrap();
    let envelopes = relay
        .fetch(address, who.identity().public_key(), &signature, now)
        .unwrap();
    let got = envelopes
        .iter()
        .filter_map(|e| match who.open_frame(e, now).unwrap() {
            Frame::Text(r) => Some(r),
            Frame::Receipt { .. } => None,
        })
        .collect();
    who.end_pass(now).unwrap();
    got
}

#[test]
fn two_clients_react_and_reply_over_a_relay() {
    let mut relay = Relay::new(ONE_BIT);
    let (mut a, mut b) = (Client::new(alice()), Client::new(bob()));

    let (said, _) = send(&mut relay, &mut a, &bob(), b"see you at 5", NOW);
    let got = collect(&mut relay, &mut b, NOW + 1);
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[0].id, said,
        "both sides name a message by the same envelope id"
    );
    let target = hex::encode(got[0].id);

    let react = frame::encode(
        &Content::Reaction {
            target: target.clone(),
            emoji: "👍".into(),
            on: true,
            at: NOW + 2,
        },
        &[],
    )
    .unwrap();
    let reply = frame::encode(
        &Content::Reply {
            target: target.clone(),
            quote: frame::quote_hash("see you at 5"),
        },
        b"ok",
    )
    .unwrap();
    let (_, react_size) = send(&mut relay, &mut b, &alice(), &react, NOW + 2);
    let (_, reply_size) = send(&mut relay, &mut b, &alice(), &reply, NOW + 3);
    assert_eq!(
        react_size, reply_size,
        "a relay cannot tell a reaction from a short reply by size"
    );

    // Alice's side: her own copy of the line Bob answered.
    let mine = (hex::encode(said), "see you at 5");
    let mut marks: Vec<Mark> = Vec::new();
    let mut quotes = Vec::new();
    for r in collect(&mut relay, &mut a, NOW + 4) {
        assert_eq!(r.from, bob().address());
        match frame::decode(&r.text).unwrap().content {
            Content::Reaction {
                target,
                emoji,
                on,
                at,
            } => {
                assert_eq!(target, mine.0);
                marks = frame::apply_mark(
                    &marks,
                    Mark {
                        mine: false,
                        emoji,
                        on,
                        at,
                    },
                );
            }
            Content::Reply { target, quote } => {
                assert_eq!(target, mine.0);
                quotes.push(frame::resolve_quote(Some(mine.1), &quote));
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(
        marks,
        vec![Mark {
            mine: false,
            emoji: "👍".into(),
            on: true,
            at: NOW + 2
        }]
    );
    assert_eq!(quotes, vec![Quote::Shown("see you at 5".into())]);

    // Bob takes the reaction back; a stale "on" replayed later loses.
    let off = Content::Reaction {
        target: target.clone(),
        emoji: "👍".into(),
        on: false,
        at: NOW + 5,
    };
    send(
        &mut relay,
        &mut b,
        &alice(),
        &frame::encode(&off, &[]).unwrap(),
        NOW + 5,
    );
    for r in collect(&mut relay, &mut a, NOW + 6) {
        if let Content::Reaction { emoji, on, at, .. } = frame::decode(&r.text).unwrap().content {
            marks = frame::apply_mark(
                &marks,
                Mark {
                    mine: false,
                    emoji,
                    on,
                    at,
                },
            );
        }
    }
    let stale = frame::apply_mark(
        &marks,
        Mark {
            mine: false,
            emoji: "👍".into(),
            on: true,
            at: NOW + 2,
        },
    );
    assert_eq!(
        stale,
        vec![Mark {
            mine: false,
            emoji: "👍".into(),
            on: false,
            at: NOW + 5
        }]
    );
}

#[test]
fn a_reply_to_a_message_this_side_never_had_shows_original_not_available() {
    let mut relay = Relay::new(ONE_BIT);
    let (mut a, mut b) = (Client::new(alice()), Client::new(bob()));
    let unknown = hex::encode([9u8; 32]);
    let reply = frame::encode(
        &Content::Reply {
            target: unknown,
            quote: frame::quote_hash("?"),
        },
        b"what?",
    )
    .unwrap();
    send(&mut relay, &mut a, &bob(), &reply, NOW);
    let got = collect(&mut relay, &mut b, NOW + 1);
    let Content::Reply { quote, .. } = frame::decode(&got[0].text).unwrap().content else {
        panic!("a reply");
    };
    // Bob holds no line with that id.
    assert_eq!(frame::resolve_quote(None, &quote), Quote::Missing);
}
