//! Custom emoji packs (ADR-140 build PR 3): a signed manifest that refuses a
//! bad signature, content-hashed ids, the pack cap, the 100 px item cap, and
//! inline and reaction rendering with a fallback for an unknown id.
#![allow(clippy::unwrap_used, clippy::expect_used)] // a test that fails should panic, and say where

use std::sync::OnceLock;

use maya_chat::emoji::{
    self, FALLBACK_GLYPH, Glyph, MAX_EMOJI_PACK_ITEMS, Segment, emoji_pack_id, expand_codes,
    resolve, segments, verify_manifest,
};
use maya_chat::frame::{
    Content, FrameError, MAX_HEADER, MAX_TITLE_CHARS, Pack, PackItem, PackKind, check_emoji,
    check_file, decode, encode, pack_id,
};
use maya_chat::media::MediaError;

const ALICE: [u8; 32] = [7; 32];
const MALLORY: [u8; 32] = [9; 32];

/// A PNG header of `side`×`side`, made distinct by `salt`.
fn png(side: u32, salt: u8) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    out.extend(side.to_be_bytes());
    out.extend(side.to_be_bytes());
    out.extend([8, 6, 0, 0, 0, 0, 0, 0, 0, salt]);
    out
}

fn item(file: &[u8], code: &str) -> PackItem {
    let (id, format) = check_file(file).unwrap();
    PackItem {
        id,
        emoji: code.into(),
        format,
    }
}

/// One signed pack shared by the tests: signing is the slow part.
fn waves() -> &'static (Pack, Vec<u8>) {
    static PACK: OnceLock<(Pack, Vec<u8>)> = OnceLock::new();
    PACK.get_or_init(|| {
        let items = vec![item(&png(100, 1), "wave"), item(&png(64, 2), "orbi_ok")];
        emoji::make_emoji_pack(&ALICE, " Waves ", items).unwrap()
    })
}

#[test]
fn a_signed_emoji_pack_round_trips_inside_a_frame() {
    let (pack, proof) = waves();
    assert_eq!(pack.kind, PackKind::Emoji);
    assert_eq!(pack.title, "Waves");
    assert_eq!(
        pack.id,
        emoji_pack_id(&pack.title, &pack.signer, &pack.items)
    );
    assert_ne!(
        pack.id,
        pack_id(&pack.title, &pack.items),
        "not a sticker pack id"
    );
    verify_manifest(pack, proof).unwrap();

    let frame = encode(&Content::EmojiPack { pack: pack.clone() }, proof).unwrap();
    let decoded = decode(&frame).unwrap();
    assert_eq!(decoded.content, Content::EmojiPack { pack: pack.clone() });
    assert_eq!(&decoded.file, proof);

    // Its items travel as ordinary pack items, checked against the pack id.
    let file = png(100, 1);
    assert!(
        decode(
            &encode(
                &Content::PackItem {
                    pack: pack.clone(),
                    index: 0
                },
                &file
            )
            .unwrap()
        )
        .is_ok()
    );
}

#[test]
fn a_manifest_with_a_bad_signature_is_refused() {
    let (pack, proof) = waves();
    let content = Content::EmojiPack { pack: pack.clone() };

    let mut flipped = proof.clone();
    *flipped.last_mut().unwrap() ^= 1;
    assert_eq!(
        verify_manifest(pack, &flipped).unwrap_err(),
        FrameError::BadProof
    );
    // `encode` refuses what `decode` would, so build the frame by hand.
    let raw = |body: &[u8]| {
        let header = serde_json::to_vec(&content).unwrap();
        let len = u32::try_from(header.len()).unwrap().to_le_bytes();
        [maya_chat::frame::MAGIC, &len, &header, body].concat()
    };
    assert_eq!(decode(&raw(&flipped)).unwrap_err(), FrameError::BadProof);
    // A flipped byte in the lattice half is caught too.
    let mut lattice = proof.clone();
    lattice[3_000] ^= 1;
    assert_eq!(decode(&raw(&lattice)).unwrap_err(), FrameError::BadProof);
    // Truncated, or nothing at all.
    assert_eq!(
        decode(&raw(&proof[..proof.len() - 1])).unwrap_err(),
        FrameError::BadProof
    );
    assert_eq!(decode(&raw(&[])).unwrap_err(), FrameError::BadProof);
}

#[test]
fn a_manifest_signed_by_someone_else_or_altered_is_refused() {
    let (pack, proof) = waves();
    // Mallory's valid signature over Alice's pack: the key is not the signer.
    let (mallory_pack, mallory_proof) =
        emoji::make_emoji_pack(&MALLORY, "Waves", pack.items.clone()).unwrap();
    assert_ne!(
        mallory_pack.signer, pack.signer,
        "a different author is a different signer"
    );
    assert_ne!(mallory_pack.id, pack.id, "and a different pack");
    assert_eq!(
        verify_manifest(pack, &mallory_proof).unwrap_err(),
        FrameError::BadProof
    );

    // A shortcode renamed after signing changes the id.
    let mut renamed = pack.clone();
    renamed.items[0].emoji = "hello".into();
    assert_eq!(
        verify_manifest(&renamed, proof).unwrap_err(),
        FrameError::PackMismatch
    );
    // Renamed and re-hashed: the signature no longer covers the id.
    renamed.id = emoji_pack_id(&renamed.title, &renamed.signer, &renamed.items);
    assert_eq!(
        verify_manifest(&renamed, proof).unwrap_err(),
        FrameError::BadProof
    );
    // Claimed to be a sticker pack.
    let mut sticker = pack.clone();
    sticker.kind = PackKind::Stickers;
    assert_eq!(
        verify_manifest(&sticker, proof).unwrap_err(),
        FrameError::PackMismatch
    );
}

#[test]
fn an_emoji_pack_is_capped_and_its_names_are_checked() {
    let many: Vec<PackItem> = (0..=MAX_EMOJI_PACK_ITEMS)
        .map(|i| item(&png(32, u8::try_from(i).unwrap()), &format!("e{i:02}")))
        .collect();
    assert_eq!(
        emoji::make_emoji_pack(&ALICE, "Many", many).unwrap_err(),
        FrameError::EmojiPackSize
    );
    assert_eq!(
        emoji::make_emoji_pack(&ALICE, "None", vec![]).unwrap_err(),
        FrameError::EmojiPackSize
    );
    let twice = vec![item(&png(32, 1), "same"), item(&png(32, 2), "same")];
    assert_eq!(
        emoji::make_emoji_pack(&ALICE, "Twice", twice).unwrap_err(),
        FrameError::EmojiPackSize
    );
    for bad in [
        "a",
        "Wave",
        "wa ve",
        "wave!",
        ":wave:",
        &"x".repeat(33),
        "😀😀",
    ] {
        assert_eq!(
            emoji::check_code(bad).unwrap_err(),
            FrameError::BadCode,
            "{bad}"
        );
    }
    for good in ["ok", "orbi_wave", "e01", &"x".repeat(32)] {
        emoji::check_code(good).unwrap();
    }
}

#[test]
fn an_emoji_item_over_100_px_is_refused_in_transit() {
    let big = png(101, 3);
    let (pack, _) = emoji::make_emoji_pack(&ALICE, "Big", vec![item(&big, "big")]).unwrap();
    // The same file is a fine sticker: the emoji cap is the emoji pack's.
    assert!(check_file(&big).is_ok());
    let frame = |body: &[u8]| {
        let header = serde_json::to_vec(&Content::PackItem {
            pack: pack.clone(),
            index: 0,
        })
        .unwrap();
        let len = u32::try_from(header.len()).unwrap().to_le_bytes();
        [maya_chat::frame::MAGIC, &len, &header, body].concat()
    };
    assert_eq!(
        decode(&frame(&big)).unwrap_err(),
        FrameError::Media(MediaError::TooBig(100, 101, 101))
    );
}

#[test]
fn a_full_emoji_pack_fits_one_frame_header() {
    // The worst case the caps allow: the longest title in 4-byte characters,
    // the longest names and format, every item.
    let items: Vec<PackItem> = (0..MAX_EMOJI_PACK_ITEMS)
        .map(|i| {
            let mut it = item(
                &png(100, u8::try_from(i).unwrap()),
                &format!("{i:02}{}", "x".repeat(30)),
            );
            it.format = maya_chat::frame::Format::Lottie;
            it
        })
        .collect();
    let title = "😀".repeat(MAX_TITLE_CHARS);
    let (pack, proof) = emoji::make_emoji_pack(&ALICE, &title, items).unwrap();
    let manifest = serde_json::to_vec(&Content::EmojiPack { pack: pack.clone() }).unwrap();
    let item_frame = serde_json::to_vec(&Content::PackItem {
        pack: pack.clone(),
        index: 23,
    })
    .unwrap();
    assert!(
        manifest.len() <= MAX_HEADER,
        "manifest header {} bytes",
        manifest.len()
    );
    assert!(
        item_frame.len() <= MAX_HEADER,
        "item header {} bytes",
        item_frame.len()
    );
    assert!(encode(&Content::EmojiPack { pack }, &proof).is_ok());
}

#[test]
fn a_sticker_pack_serializes_as_before_so_older_apps_read_it() {
    let items = vec![item(&png(100, 1), "😀")];
    let pack = Pack {
        id: pack_id("Old", &items),
        title: "Old".into(),
        items,
        mine: false,
        ..Pack::default()
    };
    let json = serde_json::to_string(&pack).unwrap();
    assert!(!json.contains("kind") && !json.contains("signer"), "{json}");
    // And an older app's pack, without the new fields, still reads.
    let old: Pack = serde_json::from_str(&json).unwrap();
    assert_eq!(old.kind, PackKind::Stickers);
}

#[test]
fn an_unknown_emoji_id_renders_the_fallback() {
    let unknown = "ab".repeat(32);
    assert_eq!(
        resolve(&unknown, &[]),
        Glyph::Fallback {
            id: unknown.clone()
        }
    );
    let (pack, _) = waves();
    assert_eq!(
        resolve(&unknown, std::slice::from_ref(pack)),
        Glyph::Fallback {
            id: unknown.clone()
        }
    );
    assert_eq!(
        segments(&format!("hi :{unknown}: there"), &[]),
        vec![
            Segment::Text { text: "hi ".into() },
            Segment::Emoji {
                glyph: Glyph::Fallback {
                    id: unknown.clone()
                }
            },
            Segment::Text {
                text: " there".into()
            },
        ]
    );
    // Not a custom id: plain text, colons and all.
    assert_eq!(
        segments("12:30 :wave: :ABC:", &[]),
        vec![Segment::Text {
            text: "12:30 :wave: :ABC:".into()
        }]
    );
    assert_eq!(FALLBACK_GLYPH, "\u{25c7}");
}

#[test]
fn a_known_emoji_id_renders_its_pack_glyph_inline() {
    let (pack, _) = waves();
    let wave = &pack.items[0];
    let got = segments(&format!(":{}:!", wave.id), std::slice::from_ref(pack));
    assert_eq!(
        got,
        vec![
            Segment::Emoji {
                glyph: Glyph::Custom {
                    id: wave.id.clone(),
                    code: "wave".into(),
                    format: wave.format
                }
            },
            Segment::Text { text: "!".into() },
        ]
    );
}

#[test]
fn a_reaction_with_a_custom_emoji_id_resolves_to_the_pack_glyph() {
    let (pack, _) = waves();
    let ok = &pack.items[1];
    check_emoji(&ok.id).unwrap();
    let reaction = Content::Reaction {
        target: "cd".repeat(32),
        emoji: ok.id.clone(),
        on: true,
        at: 1,
    };
    let Content::Reaction { emoji, .. } = decode(&encode(&reaction, &[]).unwrap()).unwrap().content
    else {
        panic!("a reaction");
    };
    assert_eq!(
        resolve(&emoji, std::slice::from_ref(pack)),
        Glyph::Custom {
            id: ok.id.clone(),
            code: "orbi_ok".into(),
            format: ok.format
        }
    );
    // An ordinary reaction stays itself.
    assert_eq!(
        resolve("👍", std::slice::from_ref(pack)),
        Glyph::Plain {
            text: "👍".into()
        }
    );
    // The same file in a sticker pack is not a custom emoji.
    let sticker = Pack {
        kind: PackKind::Stickers,
        ..pack.clone()
    };
    assert_eq!(
        resolve(&ok.id, &[sticker]),
        Glyph::Fallback { id: ok.id.clone() }
    );
}

#[test]
fn shortcodes_are_sent_as_ids_and_name_the_packs_they_use() {
    let (pack, _) = waves();
    let packs = std::slice::from_ref(pack);
    let wave = &pack.items[0].id;
    let (text, used) = expand_codes("hi :wave: and :nope: at 12:30", packs);
    assert_eq!(text, format!("hi :{wave}: and :nope: at 12:30"));
    assert_eq!(used.into_iter().collect::<Vec<_>>(), vec![pack.id.clone()]);
    // An id already in the text stays, and still names its pack.
    let (again, used) = expand_codes(&text, packs);
    assert_eq!(again, text);
    assert_eq!(used.len(), 1);
    // Nothing installed: the text is unchanged.
    let (plain, used) = expand_codes(":wave:", &[]);
    assert_eq!(plain, ":wave:");
    assert!(used.is_empty());
}
