//! Themes and wallpapers (ADR-140 build PR 4): local presentation only. Tokens
//! outside the allowed set are ignored, a per-chat look overrides the global
//! one field by field, and a wallpaper image is held to the sticker caps.
#![allow(clippy::unwrap_used, clippy::expect_used)] // a test that fails should panic, and say where

use std::collections::BTreeMap;

use maya_chat::frame::FrameError;
use maya_chat::media::MediaError;
use maya_chat::theme::{
    self, ALLOWED_TOKENS, BUILT_IN, DEFAULT_THEME, Look, Looks, MAX_ANGLE, Wallpaper,
};

fn png(side: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    out.extend(side.to_be_bytes());
    out.extend(side.to_be_bytes());
    out.extend([8, 6, 0, 0, 0, 0, 0, 0, 0, 0]);
    out
}

fn tokens(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[test]
fn every_built_in_theme_uses_only_allowed_tokens_and_colours() {
    assert!(BUILT_IN.len() >= 4, "a small set to choose from");
    assert!(BUILT_IN.iter().any(|t| t.id == DEFAULT_THEME));
    for t in BUILT_IN {
        let given = tokens(t.tokens);
        assert_eq!(theme::sanitize(&given), given, "{} keeps every token", t.id);
        assert!(theme::is_theme(t.id));
    }
    assert!(!theme::is_theme("../../etc"));
    assert!(theme::is_theme(""), "empty means the default");
}

#[test]
fn tokens_outside_the_allowed_set_are_ignored() {
    let given = tokens(&[
        ("--cyan", "#00ff88"),
        ("--bg", "#000000AA"),
        ("--evil", "#ffffff"),
        ("--text", "red; background: url(https://x.example/a.png)"),
        ("--dim", "url(https://x.example/a.png)"),
        ("--panel", "#12345"),
        ("--line", "#0b161b80"),
    ]);
    let kept = theme::sanitize(&given);
    assert_eq!(
        kept,
        tokens(&[
            ("--cyan", "#00ff88"),
            ("--line", "#0b161b80"),
            ("--bg", "#000000aa")
        ]),
        "only allowed names with hex colours survive, lower-cased"
    );
    assert!(kept.keys().all(|k| ALLOWED_TOKENS.contains(&k.as_str())));
}

#[test]
fn a_per_chat_look_overrides_the_global_one_field_by_field() {
    let mut looks = Looks::default();
    looks.global = Look {
        theme: "aurora".into(),
        tokens: tokens(&[("--cyan", "#112233")]),
        wallpaper: Wallpaper::Gradient {
            from: "#000000".into(),
            to: "#101010".into(),
            angle: 135,
        },
    };
    looks.per_chat.insert(
        "bb".into(),
        Look {
            theme: "ember".into(),
            ..Look::default()
        },
    );
    let bob = looks.look_for("bb");
    assert_eq!(bob.theme, "ember", "the chat's own theme");
    assert_eq!(
        bob.tokens, looks.global.tokens,
        "global tokens where the chat sets none"
    );
    assert_eq!(
        bob.wallpaper, looks.global.wallpaper,
        "global wallpaper where the chat sets none"
    );
    assert_eq!(
        looks.look_for("cc"),
        looks.global,
        "a chat with no override"
    );
}

#[test]
fn a_saved_look_with_bad_values_is_cleaned_on_load() {
    let raw = r##"{"global":{"theme":"nope","tokens":{"--evil":"#fff","--cyan":"#abcdef"},
        "wallpaper":{"kind":"gradient","from":"url(x)","to":"#000000","angle":9999}},
        "per_chat":{"bb":{"theme":"ember","wallpaper":{"kind":"image","id":"../../profile"}}}}"##;
    let looks: Looks = serde_json::from_str(raw).unwrap();
    let clean = looks.cleaned();
    assert_eq!(clean.global.theme, "", "an unknown theme is the default");
    assert_eq!(clean.global.tokens, tokens(&[("--cyan", "#abcdef")]));
    assert_eq!(
        clean.global.wallpaper,
        Wallpaper::None,
        "a gradient with a bad colour is dropped"
    );
    assert_eq!(clean.per_chat["bb"].theme, "ember");
    assert_eq!(
        clean.per_chat["bb"].wallpaper,
        Wallpaper::None,
        "an image id is 64 hex, never a path"
    );

    let ok = Wallpaper::Gradient {
        from: "#000000".into(),
        to: "#ffffff".into(),
        angle: MAX_ANGLE + 1,
    };
    assert_eq!(
        ok.cleaned(),
        Wallpaper::Gradient {
            from: "#000000".into(),
            to: "#ffffff".into(),
            angle: MAX_ANGLE
        }
    );
}

#[test]
fn a_wallpaper_image_is_a_still_png_or_webp_under_the_sticker_caps() {
    assert!(theme::check_wallpaper(&png(512)).is_ok());
    assert_eq!(
        theme::check_wallpaper(&png(513)).unwrap_err(),
        FrameError::Media(MediaError::TooBig(512, 513, 513))
    );
    assert_eq!(
        theme::check_wallpaper(b"GIF89a\x10\0\x10\0").unwrap_err(),
        FrameError::BadCard,
        "no moving wallpaper"
    );
    assert_eq!(
        theme::check_wallpaper(b"<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap_err(),
        FrameError::BadCard
    );
    assert_eq!(
        theme::check_wallpaper(b"not an image").unwrap_err(),
        FrameError::UnknownFormat
    );
}
