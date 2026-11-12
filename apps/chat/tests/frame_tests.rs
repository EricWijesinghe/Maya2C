//! The structured-message frame (ADR-140 build PR 1): stickers moved from
//! chat-gui, reactions and replies, forward compatibility, padding, and
//! bounded decoding of hostile bytes.
#![allow(clippy::unwrap_used, clippy::expect_used)] // a test that fails should panic, and say where

use maya_chat::frame::{
    Content, FrameError, MAGIC, MAX_HEADER, MAX_MARKS, MAX_STICKER_BYTES, MAX_TITLE_CHARS, Mark,
    PAD_BUCKET, Pack, PackItem, Quote, REACTION_EMOJI, apply_mark, check_emoji, check_file, decode,
    describe, encode, is_structured, pack_id, quote_hash, resolve_quote, sticker_id,
};

const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";

fn id(byte: u8) -> String {
    hex::encode([byte; 32])
}

fn reaction(target: &str, emoji: &str, on: bool, at: u64) -> Content {
    Content::Reaction {
        target: target.into(),
        emoji: emoji.into(),
        on,
        at,
    }
}

fn mark(mine: bool, emoji: &str, on: bool, at: u64) -> Mark {
    Mark {
        mine,
        emoji: emoji.into(),
        on,
        at,
    }
}

/// A frame with a hand-written header, unpadded, as a newer or hostile app
/// could send it.
fn raw(header: &str, body: &[u8]) -> Vec<u8> {
    let len = u32::try_from(header.len()).unwrap();
    [MAGIC, &len.to_le_bytes(), header.as_bytes(), body].concat()
}

// --- stickers (moved from apps/chat-gui/src-tauri/src/stickers.rs) ---

#[test]
fn a_sticker_round_trips_and_its_id_is_its_hash() {
    let content = Content::Sticker {
        id: sticker_id(GIF),
        emoji: "😂".into(),
        pack: "p".into(),
        title: "Mine".into(),
    };
    let plain = encode(&content, GIF).unwrap();
    assert!(is_structured(&plain));
    let decoded = decode(&plain).unwrap();
    assert_eq!(decoded.content, content);
    assert_eq!(decoded.file, GIF);
}

#[test]
fn a_swapped_file_is_refused() {
    let content = Content::Sticker {
        id: sticker_id(GIF),
        emoji: "😂".into(),
        pack: "p".into(),
        title: "Mine".into(),
    };
    let mut plain = encode(&content, GIF).unwrap();
    *plain.last_mut().unwrap() = b'!';
    assert_eq!(decode(&plain).unwrap_err(), FrameError::StickerMismatch);
    let mut other = GIF.to_vec();
    *other.last_mut().unwrap() = b'!';
    assert_eq!(
        encode(&content, &other).unwrap_err(),
        FrameError::StickerMismatch,
        "encode refuses what decode would"
    );
}

#[test]
fn oversized_or_unknown_files_are_refused() {
    let big = [b"GIF89a".as_slice(), &vec![0; MAX_STICKER_BYTES]].concat();
    assert!(
        check_file(&big)
            .unwrap_err()
            .to_string()
            .contains("at most 120 KB")
    );
    assert_eq!(
        check_file(b"#!/bin/sh\nrm -rf /").unwrap_err(),
        FrameError::UnknownFormat
    );
    assert_eq!(check_file(b"").unwrap_err(), FrameError::EmptyFile);
}

#[test]
fn truncated_or_lying_frames_are_refused() {
    assert_eq!(decode(b"hello").unwrap_err(), FrameError::NotStructured);
    assert_eq!(decode(MAGIC).unwrap_err(), FrameError::Truncated);
    let huge_len = [MAGIC, &u32::MAX.to_le_bytes()].concat();
    assert_eq!(decode(&huge_len).unwrap_err(), FrameError::Header);
    let request = encode(&Content::PackRequest { pack: "p".into() }, &[]).unwrap();
    assert!(decode(&request).is_ok());
    let with_tail = [request.as_slice(), b"extra"].concat();
    assert_eq!(
        decode(&with_tail).unwrap_err(),
        FrameError::Trailing,
        "a request carries no file"
    );
}

#[test]
fn pack_items_must_belong_to_their_pack() {
    let (sid, format) = check_file(GIF).unwrap();
    let items = vec![PackItem {
        id: sid,
        emoji: "😂".into(),
        format,
    }];
    let pack = Pack {
        id: pack_id("Mine", &items),
        title: "Mine".into(),
        items,
        mine: true,
    };
    assert!(
        decode(
            &encode(
                &Content::PackItem {
                    pack: pack.clone(),
                    index: 0
                },
                GIF
            )
            .unwrap()
        )
        .is_ok()
    );
    let past_end = raw(
        &serde_json::to_string(&Content::PackItem {
            pack: pack.clone(),
            index: 1,
        })
        .unwrap(),
        GIF,
    );
    assert_eq!(decode(&past_end).unwrap_err(), FrameError::NotInPack);
    let mut forged = pack;
    forged.title = "Renamed".into();
    let forged = raw(
        &serde_json::to_string(&Content::PackItem {
            pack: forged,
            index: 0,
        })
        .unwrap(),
        GIF,
    );
    assert_eq!(decode(&forged).unwrap_err(), FrameError::PackMismatch);
}

#[test]
fn the_largest_sticker_crosses_real_encryption_within_one_envelope() {
    use maya_chat::client::Client;
    use maya_chat::identity::Identity;
    const NOW: u64 = 1_760_000_000;
    // The worst case: a full-size file (padded GIF) in a first message,
    // which also carries the session handshake.
    let file = [GIF, &vec![0u8; MAX_STICKER_BYTES - GIF.len()]].concat();
    let content = Content::Sticker {
        id: sticker_id(&file),
        emoji: "🎉".into(),
        pack: "p".repeat(64),
        title: "x".repeat(MAX_TITLE_CHARS),
    };
    let plain = encode(&content, &file).unwrap();

    let (alice, bob) = (Identity::generate().unwrap(), Identity::generate().unwrap());
    let bundle = bob.prekey_bundle(1, NOW + 3_600).unwrap();
    let bob_addr = bob.address();
    let (mut a, mut b) = (Client::new(alice), Client::new(bob));
    let envelope = a.seal(bob_addr, Some(&bundle), &plain, NOW).unwrap();
    let size = envelope.encode().unwrap().len();
    assert!(
        size <= maya_chat::relay::MAX_ENVELOPE_BYTES,
        "a full sticker must fit one relay envelope: {size} bytes"
    );
    let decoded = decode(&b.open(&envelope, NOW).unwrap().text).unwrap();
    assert_eq!(decoded.content, content);
    assert_eq!(decoded.file, file);
}

// --- reactions and replies ---

#[test]
fn a_reaction_and_a_reply_round_trip() {
    for emoji in REACTION_EMOJI.iter().copied().chain([id(7).as_str()]) {
        let content = reaction(&id(1), emoji, true, 5);
        let decoded = decode(&encode(&content, &[]).unwrap()).unwrap();
        assert_eq!(decoded.content, content);
        assert!(decoded.file.is_empty());
    }
    let content = Content::Reply {
        target: id(1),
        quote: quote_hash("hi"),
    };
    let decoded = decode(&encode(&content, "hello back".as_bytes()).unwrap()).unwrap();
    assert_eq!(decoded.content, content);
    assert_eq!(decoded.file, b"hello back");
}

#[test]
fn reactions_are_bounded_to_the_set_or_a_custom_emoji_id() {
    for bad in [
        "",
        "x",
        "👍👍",
        "🦀",
        "A".repeat(64).as_str(),
        "a".repeat(65).as_str(),
        "<script>".repeat(8).as_str(),
    ] {
        assert_eq!(
            check_emoji(bad).unwrap_err(),
            FrameError::BadEmoji,
            "{bad:?}"
        );
        let frame = raw(
            &serde_json::to_string(&reaction(&id(1), bad, true, 1)).unwrap(),
            &[],
        );
        assert_eq!(decode(&frame).unwrap_err(), FrameError::BadEmoji, "{bad:?}");
        assert_eq!(
            encode(&reaction(&id(1), bad, true, 1), &[]).unwrap_err(),
            FrameError::BadEmoji
        );
    }
}

#[test]
fn a_reaction_or_reply_must_name_a_64_hex_id() {
    for target in [
        "",
        "zz",
        "A".repeat(64).as_str(),
        "a".repeat(63).as_str(),
        "../profile",
    ] {
        let frame = raw(
            &serde_json::to_string(&reaction(target, "👍", true, 1)).unwrap(),
            &[],
        );
        assert_eq!(decode(&frame).unwrap_err(), FrameError::BadId, "{target:?}");
        let reply = Content::Reply {
            target: target.into(),
            quote: quote_hash("x"),
        };
        assert_eq!(
            decode(&raw(&serde_json::to_string(&reply).unwrap(), b"t")).unwrap_err(),
            FrameError::BadId
        );
    }
}

#[test]
fn a_reply_body_is_text_and_a_reaction_has_none() {
    let reply = serde_json::to_string(&Content::Reply {
        target: id(1),
        quote: quote_hash("x"),
    })
    .unwrap();
    for body in [
        b"".as_slice(),
        b"   ".as_slice(),
        b"\xff\xfe".as_slice(),
        b"a\0b".as_slice(),
    ] {
        assert_eq!(
            decode(&raw(&reply, body)).unwrap_err(),
            FrameError::BadText,
            "{body:?}"
        );
    }
    let react = serde_json::to_string(&reaction(&id(1), "👍", true, 1)).unwrap();
    assert_eq!(
        decode(&raw(&react, b"x")).unwrap_err(),
        FrameError::Trailing
    );
}

#[test]
fn an_unknown_kind_is_refused_cleanly_as_needing_a_newer_app() {
    // What an older app sees from a newer one: a kind it has never heard of.
    for header in [
        // `timer` was the example here until build PR 2 made it known.
        r#"{"k":"delete","targets":[]}"#,
        r#"{"k":"story"}"#,
        r#"{"k":""}"#,
    ] {
        assert_eq!(
            decode(&raw(header, b"anything")).unwrap_err(),
            FrameError::UnknownKind,
            "{header}"
        );
        assert_eq!(
            describe(&raw(header, &[])),
            "this message needs a newer Maya2C Chat"
        );
    }
    // A known kind with fields a newer app added still reads.
    let extra = format!(
        r#"{{"k":"reaction","target":"{}","emoji":"👍","on":true,"at":1,"new":7}}"#,
        id(1)
    );
    assert!(decode(&raw(&extra, &[])).is_ok());
    // No tag at all is malformed, not unknown.
    assert!(matches!(
        decode(&raw(r#"{"x":1}"#, &[])).unwrap_err(),
        FrameError::Malformed(_)
    ));
}

#[test]
fn every_frame_is_padded_so_a_reaction_and_a_short_reply_are_the_same_size() {
    let r = encode(&reaction(&id(1), "👍", true, 1_800_000_000), &[]).unwrap();
    let reply = encode(
        &Content::Reply {
            target: id(1),
            quote: quote_hash("hi"),
        },
        b"ok",
    )
    .unwrap();
    let sticker = encode(
        &Content::Sticker {
            id: sticker_id(GIF),
            emoji: "😂".into(),
            pack: "p".into(),
            title: "t".into(),
        },
        GIF,
    )
    .unwrap();
    for plain in [&r, &reply, &sticker] {
        assert_eq!(plain.len() % PAD_BUCKET, 0, "{}", plain.len());
    }
    assert_eq!(r.len(), reply.len());
    // The padding is JSON whitespace: the header still parses as it did.
    assert_eq!(
        decode(&r).unwrap().content,
        reaction(&id(1), "👍", true, 1_800_000_000)
    );
}

#[test]
fn the_quote_hash_decides_whether_a_reply_shows_its_quote() {
    let quote = quote_hash("see you at 5");
    assert_eq!(
        resolve_quote(Some("see you at 5"), &quote),
        Quote::Shown("see you at 5".into())
    );
    assert_eq!(
        resolve_quote(Some("see you at 6"), &quote),
        Quote::Mismatch,
        "a misquote shows no quote"
    );
    assert_eq!(
        resolve_quote(None, &quote),
        Quote::Missing,
        "original not available"
    );
    assert_ne!(quote_hash("a"), quote_hash("b"));
}

#[test]
fn a_later_toggle_from_the_same_reactor_and_emoji_replaces_the_earlier() {
    let marks = apply_mark(&[], mark(false, "👍", true, 10));
    let marks = apply_mark(&marks, mark(false, "👍", false, 11));
    assert_eq!(marks, vec![mark(false, "👍", false, 11)]);
    // Another emoji or the other reactor is its own toggle.
    let marks = apply_mark(&marks, mark(false, "🔥", true, 12));
    let marks = apply_mark(&marks, mark(true, "👍", true, 12));
    assert_eq!(marks.len(), 3);
}

#[test]
fn conflicting_toggles_end_in_one_state_whatever_the_arrival_order() {
    let toggles = [
        mark(false, "👍", true, 10),
        mark(false, "👍", false, 12),
        mark(false, "👍", true, 12),
        mark(false, "👍", true, 11),
    ];
    let fold = |order: &[usize]| {
        order
            .iter()
            .fold(Vec::new(), |m, &i| apply_mark(&m, toggles[i].clone()))
    };
    let want = vec![mark(false, "👍", false, 12)];
    for order in [
        [0, 1, 2, 3],
        [3, 2, 1, 0],
        [2, 0, 3, 1],
        [1, 3, 0, 2],
        [2, 1, 0, 3],
    ] {
        assert_eq!(fold(&order), want, "{order:?}: same time, off wins");
    }
}

#[test]
fn marks_on_one_message_are_capped() {
    let mut marks = Vec::new();
    for n in 0..=u8::try_from(MAX_MARKS).unwrap() {
        marks = apply_mark(&marks, mark(false, &id(n), true, 1));
    }
    assert_eq!(marks.len(), MAX_MARKS);
}

#[test]
fn describe_prints_frames_for_the_cli() {
    assert_eq!(describe(b"plain words"), "plain words");
    let r = encode(&reaction(&id(0xab), "👍", true, 1), &[]).unwrap();
    assert_eq!(describe(&r), "[reacted 👍 to abababab]");
    let reply = encode(
        &Content::Reply {
            target: id(0xab),
            quote: quote_hash("x"),
        },
        b"sure",
    )
    .unwrap();
    assert_eq!(describe(&reply), "[reply to abababab] sure");
}

// --- hostile input ---

/// Deterministic pseudo-random bytes (BLAKE3 XOF), so a failure reproduces.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut out = vec![0; len];
    blake3::Hasher::new()
        .update(&seed.to_le_bytes())
        .finalize_xof()
        .fill(&mut out);
    out
}

#[test]
fn hostile_frames_never_panic_and_are_bounded() {
    let valid = encode(&reaction(&id(1), "👍", true, 1), &[]).unwrap();
    for seed in 0..2_000u64 {
        let len = usize::try_from(seed % 600).unwrap();
        let _ = decode(&[MAGIC, &noise(seed, len)].concat());
        // Valid frames with bytes flipped or cut short.
        let mut bent = valid.clone();
        let at = usize::try_from(seed).unwrap() % bent.len();
        bent[at] ^= noise(seed, 1)[0] | 1;
        let _ = decode(&bent);
        let _ = decode(&valid[..at]);
    }
    // A length field that claims more than the bound is refused before parsing.
    let claims = u32::try_from(MAX_HEADER + PAD_BUCKET + 1).unwrap();
    let over = [
        MAGIC,
        &claims.to_le_bytes(),
        &vec![b' '; MAX_HEADER + PAD_BUCKET + 1],
    ]
    .concat();
    assert_eq!(decode(&over).unwrap_err(), FrameError::Header);
    // A body past the cap is refused before any hashing.
    let big = raw(
        r#"{"k":"pack_request","pack":"p"}"#,
        &vec![0; MAX_STICKER_BYTES + 1],
    );
    assert_eq!(decode(&big).unwrap_err(), FrameError::BodyTooLarge);
    // Deep nesting stays inside serde_json's recursion limit.
    let deep = format!(
        r#"{{"k":"sticker","id":{}1{}}}"#,
        "[".repeat(2_000),
        "]".repeat(2_000)
    );
    assert!(decode(&raw(&deep[..MAX_HEADER.min(deep.len())], &[])).is_err());
}

#[test]
fn an_oversized_header_is_refused_at_encode() {
    let content = Content::PackRequest {
        pack: "p".repeat(MAX_HEADER),
    };
    assert_eq!(
        encode(&content, &[]).unwrap_err(),
        FrameError::HeaderTooLong
    );
}
