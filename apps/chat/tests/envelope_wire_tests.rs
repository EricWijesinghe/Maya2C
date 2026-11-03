//! Envelope bytes are pinned (ADR-074 decision 4, test plan item 9). The v1
//! vectors were generated from the encoder before envelope v2 existed; a v1
//! envelope must keep producing exactly these bytes, or envelope ids and
//! relay dedupe for existing peers change silently.
#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use maya_chat::relay::{Envelope, Payload, Relay};
use maya_chat::session::{Handshake, Message};

const TO: [u8; 32] = [7; 32];
const EXPIRES: u64 = 1_800_604_800;

fn message() -> Message {
    Message {
        session: [3; 32],
        n: 5,
        ciphertext: vec![9; 40],
    }
}

fn chat_envelope() -> Envelope {
    Envelope {
        to: TO,
        expires_at: EXPIRES,
        payload: Payload::Chat(message()),
        stamp: 0,
        version: 1,
    }
}

fn open_envelope() -> Envelope {
    Envelope {
        to: TO,
        expires_at: EXPIRES,
        payload: Payload::Open {
            handshake: Handshake {
                initiator_key: vec![1; 16],
                responder: TO,
                prekey_epoch: 1,
                ciphertext: vec![2; 8],
                sent_at: EXPIRES - 1,
                signature: vec![3; 12],
            },
            first: message(),
        },
        stamp: 42,
        version: 1,
    }
}

/// Hex of the v1 encoding of [`chat_envelope`], from the pre-v2 encoder.
const V1_CHAT_HEX: &str = "a462746f982007070707070707070707070707070707070707070707070707070707070707076a657870697265735f61741a6b530c80677061796c6f6164a16443686174a36773657373696f6e98200303030303030303030303030303030303030303030303030303030303030303616e056a63697068657274657874982809090909090909090909090909090909090909090909090909090909090909090909090909090909657374616d7000";
/// Hex of the v1 encoding of [`open_envelope`], from the pre-v2 encoder.
const V1_OPEN_HEX: &str = "a462746f982007070707070707070707070707070707070707070707070707070707070707076a657870697265735f61741a6b530c80677061796c6f6164a1644f70656ea26968616e647368616b65a66d696e69746961746f725f6b6579900101010101010101010101010101010169726573706f6e646572982007070707070707070707070707070707070707070707070707070707070707076c7072656b65795f65706f6368016a636970686572746578748802020202020202026773656e745f61741a6b530c7f697369676e61747572658c030303030303030303030303656669727374a36773657373696f6e98200303030303030303030303030303030303030303030303030303030303030303616e056a63697068657274657874982809090909090909090909090909090909090909090909090909090909090909090909090909090909657374616d70182a";

#[test]
fn v1_chat_envelope_bytes_are_pinned() {
    assert_eq!(hex::encode(chat_envelope().encode().unwrap()), V1_CHAT_HEX);
}

#[test]
fn v1_open_envelope_bytes_are_pinned() {
    assert_eq!(hex::encode(open_envelope().encode().unwrap()), V1_OPEN_HEX);
}

#[test]
fn v2_bytes_are_smaller_and_decode_to_the_same_envelope() {
    let mut big = chat_envelope();
    if let Payload::Chat(message) = &mut big.payload {
        message.ciphertext = vec![0xA5; 4096];
    }
    let v1 = big.encode().unwrap();
    big.version = 2;
    let v2 = big.encode().unwrap();
    assert!(v2.len() < v1.len(), "v2 {} >= v1 {}", v2.len(), v1.len());
    assert_eq!(Envelope::decode(&v2).unwrap(), big);
    big.version = 1;
    assert_eq!(Envelope::decode(&v1).unwrap(), big);
}

#[test]
fn both_versions_round_trip_the_handshake_envelope() {
    for version in [1, 2] {
        let mut envelope = open_envelope();
        envelope.version = version;
        let bytes = envelope.encode().unwrap();
        assert_eq!(
            Envelope::decode(&bytes).unwrap(),
            envelope,
            "version {version}"
        );
    }
}

#[test]
fn a_version_this_build_does_not_read_is_refused() {
    let mut envelope = chat_envelope();
    for version in [0, 3] {
        envelope.version = version;
        assert!(
            envelope.encode().is_err(),
            "version {version} is not written"
        );
        assert!(
            Relay::new(0)
                .deposit(envelope.clone(), EXPIRES - 1)
                .is_err(),
            "version {version} is not stored"
        );
    }
}

/// The envelope as it was before version existed: same fields, no version.
#[derive(serde::Serialize)]
struct PreV2Envelope {
    to: [u8; 32],
    expires_at: u64,
    payload: Payload,
    stamp: u64,
}

#[test]
fn v1_bytes_equal_the_pre_version_struct_encoding() {
    let mut old = Vec::new();
    let envelope = open_envelope();
    let pre = PreV2Envelope {
        to: envelope.to,
        expires_at: envelope.expires_at,
        payload: envelope.payload.clone(),
        stamp: envelope.stamp,
    };
    ciborium::into_writer(&pre, &mut old).unwrap();
    assert_eq!(envelope.encode().unwrap(), old);
}

#[test]
fn sealed_sizes_for_one_kib_and_a_full_sticker_under_v1_and_v2() {
    // The numbers ADR-074 test 10 asks for. Run with --nocapture to read them.
    const STICKER: usize = 120 << 10;
    for (label, len) in [("1 KiB text", 1 << 10), ("120 KiB sticker", STICKER)] {
        let mut envelope = chat_envelope();
        if let Payload::Chat(message) = &mut envelope.payload {
            message.ciphertext = vec![0x5A; len];
        }
        envelope.version = 1;
        let v1 = envelope.encode().unwrap().len();
        envelope.version = 2;
        let v2 = envelope.encode().unwrap().len();
        println!("{label}: v1 {v1} B, v2 {v2} B");
        assert!(v2 < v1);
    }
}
