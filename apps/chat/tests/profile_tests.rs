//! Profile cards (ADR-140 build PR 4): a signed card that round-trips inside a
//! frame, a stale or replayed version that is ignored, names and bios that are
//! sanitized or refused, per-contact visibility, and avatar and frame files
//! held to the sticker caps of `media.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used)] // a test that fails should panic, and say where

use std::sync::OnceLock;

use maya_chat::frame::{Content, FrameError, MAX_STICKER_BYTES, decode, encode};
use maya_chat::media::MediaError;
use maya_chat::profile::{
    self, Card, Draft, MAX_BIO_CHARS, MAX_NAME_CHARS, Sharing, Show, accept, make_card,
    sanitize_bio, sanitize_name, verify_card,
};

const ALICE: [u8; 32] = [7; 32];
const MALLORY: [u8; 32] = [9; 32];
const BOB: &str = "bb";
const CAROL: &str = "cc";
const DAVE: &str = "dd";

/// A PNG header of `side`×`side`, made distinct by `salt`.
fn png(side: u32, salt: u8) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    out.extend(side.to_be_bytes());
    out.extend(side.to_be_bytes());
    out.extend([8, 6, 0, 0, 0, 0, 0, 0, 0, salt]);
    out
}

/// A Lottie animation of `op` frames at `fr` fps.
fn lottie(fr: u32, op: u32, side: u32) -> Vec<u8> {
    format!(r#"{{"v":"5.7.4","fr":{fr},"ip":0,"op":{op},"w":{side},"h":{side},"layers":[]}}"#)
        .into_bytes()
}

fn draft(name: &str) -> Draft {
    Draft {
        name: name.into(),
        bio: "Builds relays.\nLikes tea.".into(),
        avatar: None,
        frame: None,
        theme: "aurora".into(),
    }
}

/// One signed card with an avatar and a frame, shared by the tests: signing
/// is the slow part.
fn ann() -> &'static (Card, Vec<u8>) {
    static CARD: OnceLock<(Card, Vec<u8>)> = OnceLock::new();
    CARD.get_or_init(|| {
        make_card(
            &ALICE,
            &draft("Ann"),
            3,
            Some(&png(256, 1)),
            Some(&lottie(30, 60, 512)),
        )
        .unwrap()
    })
}

#[test]
fn a_signed_card_round_trips_inside_a_frame() {
    let (card, body) = ann();
    assert_eq!(card.version, 3);
    assert_eq!(card.name, "Ann");
    let files = verify_card(card, body).unwrap();
    assert_eq!(files.avatar.as_deref(), Some(png(256, 1).as_slice()));
    assert_eq!(files.frame.as_deref(), Some(lottie(30, 60, 512).as_slice()));

    let plain = encode(&Content::Profile { card: card.clone() }, body).unwrap();
    let decoded = decode(&plain).unwrap();
    assert_eq!(decoded.content, Content::Profile { card: card.clone() });
    assert_eq!(&decoded.file, body);
}

#[test]
fn a_changed_field_or_another_signer_breaks_the_signature() {
    let (card, body) = ann();
    let mut renamed = card.clone();
    renamed.name = "Mallory".into();
    assert_eq!(
        verify_card(&renamed, body).unwrap_err(),
        FrameError::BadProof
    );

    let mut bumped = card.clone();
    bumped.version += 1;
    assert_eq!(
        verify_card(&bumped, body).unwrap_err(),
        FrameError::BadProof,
        "the version is signed"
    );

    // Mallory signs her own card and claims Ann's signer.
    let (mut forged, forged_body) = make_card(&MALLORY, &draft("Ann"), 3, None, None).unwrap();
    assert_ne!(forged.signer, card.signer, "a signer per identity");
    forged.signer.clone_from(&card.signer);
    assert_eq!(
        verify_card(&forged, &forged_body).unwrap_err(),
        FrameError::BadProof
    );

    let mut flipped = body.clone();
    flipped[10] ^= 1;
    assert_eq!(
        verify_card(card, &flipped).unwrap_err(),
        FrameError::BadProof
    );
}

#[test]
fn a_swapped_avatar_is_refused() {
    let (card, body) = ann();
    let avatar = png(256, 1);
    let at = body
        .windows(avatar.len())
        .position(|w| w == avatar)
        .unwrap();
    let mut swapped = body.clone();
    swapped[at + avatar.len() - 1] ^= 1;
    assert_eq!(
        verify_card(card, &swapped).unwrap_err(),
        FrameError::StickerMismatch
    );
    let mut short = body.clone();
    short.pop();
    assert!(
        verify_card(card, &short).is_err(),
        "a truncated body is refused"
    );
}

#[test]
fn a_stale_replayed_or_out_of_order_version_is_ignored() {
    let (v3, _) = ann();
    let mut v2 = v3.clone();
    v2.version = 2;
    let mut v4 = v3.clone();
    v4.version = 4;
    assert!(accept(None, v3), "the first card is taken");
    assert!(
        !accept(Some(v3), &v2),
        "an older card arriving late is ignored"
    );
    assert!(
        !accept(Some(v3), v3),
        "a replay of the held version is ignored"
    );
    assert!(accept(Some(v3), &v4), "a newer one replaces it");
    let mut other = v4.clone();
    other.version = 9;
    other.signer = "ab".repeat(32);
    assert!(
        !accept(Some(v3), &other),
        "a card from another signer never replaces the held one"
    );
}

#[test]
fn names_lose_control_and_direction_characters() {
    // U+202E (right-to-left override) would draw "Ann\u{202e}gpj.exe" as "Annexe.jpg".
    assert_eq!(sanitize_name("Ann\u{202e}gpj.exe").unwrap(), "Anngpj.exe");
    for mark in [
        '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{2066}', '\u{2067}', '\u{2068}',
        '\u{2069}', '\u{200e}', '\u{200f}', '\u{061c}',
    ] {
        assert_eq!(
            sanitize_name(&format!("A{mark}nn")).unwrap(),
            "Ann",
            "{:04x}",
            u32::from(mark)
        );
    }
    assert_eq!(
        sanitize_name("  A\u{0}n\u{7}n\r\n\t ").unwrap(),
        "Ann",
        "control characters go"
    );
    assert_eq!(
        sanitize_name("A\u{200b}n\u{feff}n").unwrap(),
        "Ann",
        "zero-width characters go"
    );
    assert_eq!(
        sanitize_name("Ann   Lee").unwrap(),
        "Ann Lee",
        "runs of spaces collapse"
    );
    assert_eq!(
        sanitize_name("Ann\u{2028}Lee").unwrap(),
        "Ann Lee",
        "a line separator is a space"
    );
    assert_eq!(
        sanitize_name("Ann 🌸 Lee").unwrap(),
        "Ann 🌸 Lee",
        "emoji stay"
    );
    assert_eq!(
        sanitize_name("\u{202e}\u{200b} \u{0}").unwrap_err(),
        FrameError::BadName,
        "nothing left"
    );
}

#[test]
fn an_oversize_name_or_bio_is_refused_not_cut() {
    let at_cap = "a".repeat(MAX_NAME_CHARS);
    assert_eq!(sanitize_name(&at_cap).unwrap(), at_cap);
    assert_eq!(
        sanitize_name(&format!("{at_cap}a")).unwrap_err(),
        FrameError::BadName
    );
    assert_eq!(
        sanitize_name(&"é".repeat(MAX_NAME_CHARS))
            .unwrap()
            .chars()
            .count(),
        MAX_NAME_CHARS,
        "counted in characters"
    );

    let bio = "b".repeat(MAX_BIO_CHARS);
    assert_eq!(sanitize_bio(&bio).unwrap(), bio);
    assert_eq!(
        sanitize_bio(&format!("{bio}b")).unwrap_err(),
        FrameError::BadBio
    );
    assert_eq!(
        sanitize_bio("one\n\n\n\ntwo\u{202e}").unwrap(),
        "one\n\ntwo",
        "newlines kept, at most one blank line"
    );
    assert_eq!(sanitize_bio("").unwrap(), "", "a bio may be empty");
}

#[test]
fn a_card_carrying_an_unsanitized_name_is_refused_on_receipt() {
    let (card, body) = ann();
    let mut sneaky = card.clone();
    sneaky.name = "Ann\u{202e}".into();
    assert_eq!(verify_card(&sneaky, body).unwrap_err(), FrameError::BadName);
    // make_card sanitizes what the user typed.
    let (clean, _) = make_card(&ALICE, &draft(" Ann\u{202e} "), 1, None, None).unwrap();
    assert_eq!(clean.name, "Ann");
    assert!(
        make_card(
            &ALICE,
            &draft(&"a".repeat(MAX_NAME_CHARS + 1)),
            1,
            None,
            None
        )
        .is_err()
    );
}

#[test]
fn an_unknown_theme_id_or_version_zero_is_refused() {
    let mut bad = draft("Ann");
    bad.theme = "../../etc".into();
    assert_eq!(
        make_card(&ALICE, &bad, 1, None, None).unwrap_err(),
        FrameError::BadCard
    );
    assert_eq!(
        make_card(&ALICE, &draft("Ann"), 0, None, None).unwrap_err(),
        FrameError::BadCard
    );
}

#[test]
fn the_avatar_caps_are_the_sticker_caps_of_media_rs() {
    // 512 px passes, 513 is refused by media.rs's own side cap.
    assert!(profile::check_avatar(&png(512, 1)).is_ok());
    assert_eq!(
        profile::check_avatar(&png(513, 1)).unwrap_err(),
        FrameError::Media(MediaError::TooBig(512, 513, 513))
    );
    // A moving avatar is a frame's job: WebM and Lottie are refused as avatars.
    assert_eq!(
        profile::check_avatar(&lottie(30, 60, 256)).unwrap_err(),
        FrameError::BadCard
    );
    // Over the shared ceiling, or not an image at all.
    let mut huge = png(256, 1);
    huge.resize(MAX_STICKER_BYTES + 1, 0);
    assert!(matches!(
        profile::check_avatar(&huge).unwrap_err(),
        FrameError::FileTooLarge(_)
    ));
    assert_eq!(
        profile::check_avatar(b"MZ\x90\0").unwrap_err(),
        FrameError::UnknownFormat
    );
}

#[test]
fn an_animated_frame_is_lottie_under_the_sticker_animation_caps() {
    assert!(profile::check_frame(&lottie(60, 180, 512)).is_ok());
    // Threat model case 7: a 10,000-frame Lottie under the byte cap.
    assert!(matches!(
        profile::check_frame(&lottie(60, 10_000, 512)).unwrap_err(),
        FrameError::Media(MediaError::TooLong(_))
    ));
    assert_eq!(
        profile::check_frame(&png(256, 1)).unwrap_err(),
        FrameError::BadCard,
        "a frame moves"
    );
    assert!(
        make_card(
            &ALICE,
            &draft("Ann"),
            1,
            None,
            Some(&lottie(60, 10_000, 512))
        )
        .is_err()
    );
    assert!(make_card(&ALICE, &draft("Ann"), 1, Some(&png(513, 1)), None).is_err());
}

#[test]
fn a_card_whose_files_overflow_one_envelope_is_refused() {
    let mut big = png(256, 1);
    big.resize(MAX_STICKER_BYTES - 64, 0);
    assert!(profile::check_avatar(&big).is_ok(), "the file alone fits");
    assert_eq!(
        make_card(&ALICE, &draft("Ann"), 1, Some(&big), None).unwrap_err(),
        FrameError::BodyTooLarge,
        "with the signature it does not"
    );
}

#[test]
fn per_contact_visibility_is_respected() {
    let mut sharing = Sharing::default();
    assert_eq!(
        sharing.card_for(BOB),
        None,
        "nothing is shared until the user makes a card"
    );

    sharing.cards.insert("main".into(), draft("Ann"));
    sharing.cards.insert("work".into(), draft("A. Lee"));
    sharing.default = Some("main".into());
    sharing
        .per_contact
        .insert(CAROL.into(), Show::Card("work".into()));
    sharing.per_contact.insert(DAVE.into(), Show::Nothing);

    assert_eq!(
        sharing.card_for(BOB).unwrap().name,
        "Ann",
        "the default card"
    );
    assert_eq!(
        sharing.card_for(CAROL).unwrap().name,
        "A. Lee",
        "two contacts hold different cards"
    );
    assert_eq!(
        sharing.card_for(DAVE),
        None,
        "a contact shown nothing gets nothing"
    );

    // A per-contact choice naming a deleted card falls back to nothing, never to the default.
    sharing.cards.remove("work");
    assert_eq!(sharing.card_for(CAROL), None);
    // No default: only contacts given a card explicitly see one.
    sharing.default = None;
    assert_eq!(sharing.card_for(BOB), None);
}

#[test]
fn every_send_takes_a_higher_version_so_a_switch_replaces_the_old_card() {
    let mut sharing = Sharing::default();
    let first = sharing.next_version();
    let second = sharing.next_version();
    assert!(second > first && first > 0);
    let mut round_trip: Sharing =
        serde_json::from_str(&serde_json::to_string(&sharing).unwrap()).unwrap();
    assert!(
        round_trip.next_version() > second,
        "the counter survives a restart"
    );
}

#[test]
fn describe_names_a_card_without_its_files() {
    let (card, body) = ann();
    let plain = encode(&Content::Profile { card: card.clone() }, body).unwrap();
    assert_eq!(maya_chat::frame::describe(&plain), "[profile card: Ann]");
}
