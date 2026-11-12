//! Disappearing timers in the frame (ADR-140 build PR 2): the `Timer`
//! control frame, the per-message timer in the header, and the envelope's
//! relay expiry capped at the timer. Expiry itself is the desktop app's
//! (`apps/chat-gui/src-tauri/src/expiry.rs`).
#![allow(clippy::unwrap_used, clippy::expect_used)] // a test that fails should panic, and say where

use maya_chat::client::{Client, ENVELOPE_TTL};
use maya_chat::frame::{
    Content, FrameError, MAGIC, PAD_BUCKET, TIMER_OPTIONS, check_timer, decode, describe, encode,
    encode_timed, quote_hash, timer_label,
};
use maya_chat::identity::Identity;

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 24 * 3_600;

fn raw(header: &str, body: &[u8]) -> Vec<u8> {
    let len = u32::try_from(header.len()).unwrap();
    [MAGIC, &len.to_le_bytes(), header.as_bytes(), body].concat()
}

#[test]
fn the_timer_frame_round_trips_for_every_offered_value() {
    for (secs, label) in TIMER_OPTIONS {
        let plain = encode(&Content::Timer { secs }, &[]).unwrap();
        assert_eq!(plain.len() % PAD_BUCKET, 0, "padded like every frame");
        let got = decode(&plain).unwrap();
        assert_eq!(got.content, Content::Timer { secs });
        assert!(got.file.is_empty());
        assert_eq!(got.timer, 0, "the control frame has no timer of its own");
        assert_eq!(timer_label(secs), Some(label));
    }
}

#[test]
fn the_timer_frame_is_bounded() {
    let plain = encode(&Content::Timer { secs: 3_600 }, &[]).unwrap();
    assert_eq!(
        plain.len(),
        PAD_BUCKET,
        "one bucket: as small as a reaction"
    );
    // A body on a control frame is refused, not ignored.
    assert_eq!(
        decode(&raw(r#"{"k":"timer","secs":3600}"#, b"x")).unwrap_err(),
        FrameError::Trailing
    );
    // A length field claiming more than the frame holds.
    let mut lying = plain.clone();
    lying[MAGIC.len()..MAGIC.len() + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(decode(&lying).unwrap_err(), FrameError::Header);
}

#[test]
fn an_unknown_timer_value_is_refused() {
    // Red team (threat model item 3): u64::MAX, values near the offered
    // ones, and a negative or textual value.
    for secs in [u64::MAX, 1, 29, 31, 301, 2_592_000, 7_776_001] {
        assert_eq!(check_timer(secs), Err(FrameError::BadTimer), "{secs}");
        assert_eq!(
            encode(&Content::Timer { secs }, &[]).unwrap_err(),
            FrameError::BadTimer
        );
        let header = format!(r#"{{"k":"timer","secs":{secs}}}"#);
        assert_eq!(
            decode(&raw(&header, &[])).unwrap_err(),
            FrameError::BadTimer
        );
        assert_eq!(
            encode_timed(&Content::Text, b"hi", secs).unwrap_err(),
            FrameError::BadTimer
        );
    }
    // Zero is "off", an offered value.
    assert_eq!(check_timer(0), Ok(()));
    for header in [r#"{"k":"timer","secs":-1}"#, r#"{"k":"timer","secs":"30"}"#] {
        assert!(matches!(
            decode(&raw(header, &[])).unwrap_err(),
            FrameError::Malformed(_)
        ));
    }
    // A per-message timer outside the set is refused on a known kind too.
    assert_eq!(
        decode(&raw(r#"{"k":"text","t":12345}"#, b"hi")).unwrap_err(),
        FrameError::BadTimer
    );
}

#[test]
fn a_message_carries_its_own_timer_in_the_header() {
    let text = decode(&encode_timed(&Content::Text, b"see you at 5", 300).unwrap()).unwrap();
    assert_eq!(
        (text.content, text.file.as_slice(), text.timer),
        (Content::Text, &b"see you at 5"[..], 300)
    );
    let reply = Content::Reply {
        target: hex::encode([1; 32]),
        quote: quote_hash("hi"),
    };
    let got = decode(&encode_timed(&reply, b"ok", 30).unwrap()).unwrap();
    assert_eq!((got.content, got.timer), (reply, 30));
    // Untimed frames read as timer 0, and older frames have no `t` at all.
    let plain = encode(&Content::Text, b"hello").unwrap();
    assert_eq!(decode(&plain).unwrap().timer, 0);
    assert!(!String::from_utf8_lossy(&plain).contains("\"t\""));
}

#[test]
fn only_messages_take_a_timer() {
    // A control frame, a reaction or a pack request with a `t` is refused:
    // a timer belongs to a line the user sees.
    assert_eq!(
        encode_timed(&Content::Timer { secs: 30 }, &[], 30).unwrap_err(),
        FrameError::BadTimer
    );
    assert_eq!(
        encode_timed(&Content::PackRequest { pack: "p".into() }, &[], 30).unwrap_err(),
        FrameError::BadTimer
    );
}

#[test]
fn a_timed_text_needs_text() {
    assert_eq!(
        encode_timed(&Content::Text, b"", 30).unwrap_err(),
        FrameError::BadText
    );
    assert_eq!(
        encode_timed(&Content::Text, b"a\0b", 30).unwrap_err(),
        FrameError::BadText
    );
    assert_eq!(
        decode(&raw(r#"{"k":"text"}"#, &[0xff, 0xfe])).unwrap_err(),
        FrameError::BadText
    );
}

#[test]
fn the_cli_describes_timers_in_plain_words() {
    assert_eq!(
        describe(&encode(&Content::Timer { secs: 3_600 }, &[]).unwrap()),
        "[disappearing messages set to 1 hour]"
    );
    assert_eq!(
        describe(&encode(&Content::Timer { secs: 0 }, &[]).unwrap()),
        "[disappearing messages turned off]"
    );
    assert_eq!(
        describe(&encode_timed(&Content::Text, b"bye", 30).unwrap()),
        "bye"
    );
}

#[test]
fn the_envelope_never_outlives_its_timer_at_the_relay() {
    let bob = Identity::from_seed([2; 32]);
    let bundle = bob.prekey_bundle(1, NOW + DAY).unwrap();
    let mut alice = Client::new(Identity::from_seed([1; 32]));
    let short = alice
        .queue_for(bob.address(), Some(&bundle), b"x", NOW, 2, 30)
        .unwrap();
    assert_eq!(alice.pending(&short).unwrap().envelope.expires_at, NOW + 30);
    // A timer longer than the relay TTL is capped at the TTL, never extended.
    let long = alice
        .queue_for(bob.address(), None, b"y", NOW, 2, 90 * DAY)
        .unwrap();
    assert_eq!(
        alice.pending(&long).unwrap().envelope.expires_at,
        NOW + ENVELOPE_TTL
    );
    // The outbox stops retrying a short-lived envelope once it has expired.
    assert!(alice.due(NOW + 29).contains(&short));
    assert!(!alice.due(NOW + 30).contains(&short));
    assert!(alice.due(NOW + 30).contains(&long));
}
