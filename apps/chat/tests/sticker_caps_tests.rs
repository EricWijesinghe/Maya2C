//! Per-format sticker caps (ADR-140 build PR 3): each cap passes at its
//! limit and refuses one byte, frame, millisecond or pixel over; the format
//! is read from the bytes; a TGS bomb stops at the inflate cap; a Lottie
//! depth bomb is refused. The samples are Telegram-sized and made here.
#![allow(clippy::unwrap_used, clippy::expect_used)] // a test that fails should panic, and say where

use std::io::Write as _;

use maya_chat::frame::{Format, FrameError, MAX_STICKER_BYTES, check_file};
use maya_chat::media::{
    self, MAX_ANIMATION_BYTES, MAX_LOTTIE_DEPTH, MAX_TGS_INFLATED, MAX_WEBM_FRAMES, MediaError,
};

fn media_err(bytes: &[u8]) -> MediaError {
    match check_file(bytes).unwrap_err() {
        FrameError::Media(e) => e,
        other => panic!("expected a media cap, got {other:?}"),
    }
}

// ---- Lottie and TGS --------------------------------------------------------

/// A Lottie animation, padded with a name field to exactly `len` bytes.
fn lottie(fr: f64, op: f64, side: u32, len: usize) -> Vec<u8> {
    let head = format!(
        r#"{{"v":"5.7.4","fr":{fr},"ip":0,"op":{op},"w":{side},"h":{side},"layers":[],"nm":""#
    );
    let tail = r#""}"#;
    let pad = len
        .checked_sub(head.len() + tail.len())
        .expect("room for the header");
    [head.as_bytes(), &vec![b'x'; pad], tail.as_bytes()].concat()
}

fn telegram_lottie() -> Vec<u8> {
    lottie(60.0, 180.0, 512, 4_096)
}

/// `json` gzipped with stored blocks, so the size follows the input byte for
/// byte; padded inside the JSON until the gzip file is exactly `len` bytes.
fn tgs_of_len(fr: f64, op: f64, len: usize) -> Vec<u8> {
    for json_len in (len - 400)..len {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::none());
        gz.write_all(&lottie(fr, op, 512, json_len)).unwrap();
        let out = gz.finish().unwrap();
        if out.len() == len {
            return out;
        }
    }
    panic!("no stored-gzip size hit {len}");
}

#[test]
fn lottie_at_64_kb_passes_and_one_byte_over_is_refused() {
    let at_cap = lottie(60.0, 180.0, 512, MAX_ANIMATION_BYTES);
    assert_eq!(check_file(&at_cap).unwrap().1, Format::Lottie);
    let over = lottie(60.0, 180.0, 512, MAX_ANIMATION_BYTES + 1);
    assert!(matches!(
        media_err(&over),
        MediaError::AnimationTooLarge("Lottie", 65)
    ));
}

#[test]
fn lottie_at_three_seconds_and_60_fps_passes_and_one_frame_over_is_refused() {
    assert!(check_file(&telegram_lottie()).is_ok());
    assert!(
        check_file(&lottie(30.0, 90.0, 512, 4_096)).is_ok(),
        "3 s at 30 fps"
    );
    assert!(matches!(
        media_err(&lottie(60.0, 181.0, 512, 4_096)),
        MediaError::TooLong(_)
    ));
    assert!(matches!(
        media_err(&lottie(30.0, 91.0, 512, 4_096)),
        MediaError::TooLong(_)
    ));
    assert!(matches!(
        media_err(&lottie(61.0, 180.0, 512, 4_096)),
        MediaError::TooFast(60, _)
    ));
}

#[test]
fn lottie_at_512_px_passes_and_513_is_refused() {
    assert_eq!(
        media::check(Format::Lottie, &lottie(60.0, 60.0, 512, 4_096)).unwrap(),
        Some((512, 512))
    );
    assert!(matches!(
        media_err(&lottie(60.0, 60.0, 513, 4_096)),
        MediaError::TooBig(512, 513, 513)
    ));
}

#[test]
fn a_lottie_depth_bomb_is_refused_before_it_is_parsed() {
    // The top-level object is level 1, so `MAX_LOTTIE_DEPTH - 1` arrays fit.
    let nest = |n: usize| {
        let base = String::from_utf8(lottie(60.0, 60.0, 512, 4_096)).unwrap();
        let bomb = format!(r#","x":{}{}}}"#, "[".repeat(n), "]".repeat(n));
        format!("{}{bomb}", &base[..base.len() - 1]).into_bytes()
    };
    assert!(check_file(&nest(MAX_LOTTIE_DEPTH - 1)).is_ok());
    assert_eq!(media_err(&nest(MAX_LOTTIE_DEPTH)), MediaError::TooDeep);
    // Brackets inside strings are not nesting.
    let quoted = String::from_utf8(lottie(60.0, 60.0, 512, 4_096))
        .unwrap()
        .replacen("xxxx", &"[{".repeat(100), 1);
    assert!(check_file(quoted.as_bytes()).is_ok());
    // Deeper than serde_json's own limit is refused too, whichever check stops it.
    assert!(check_file(&nest(1_000)).is_err());
}

#[test]
fn tgs_at_64_kb_passes_and_one_byte_over_is_refused() {
    let at_cap = tgs_of_len(60.0, 180.0, MAX_ANIMATION_BYTES);
    assert_eq!(check_file(&at_cap).unwrap().1, Format::Tgs);
    // The size cap is read before anything is inflated.
    let over = [at_cap, vec![0]].concat();
    assert!(matches!(
        media_err(&over),
        MediaError::AnimationTooLarge(".tgs", 65)
    ));
}

#[test]
fn a_telegram_sized_tgs_imports_and_its_animation_caps_still_apply() {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(&telegram_lottie()).unwrap();
    let tgs = gz.finish().unwrap();
    assert!(check_file(&tgs).is_ok());

    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(&lottie(60.0, 181.0, 512, 4_096)).unwrap();
    assert!(matches!(
        media_err(&gz.finish().unwrap()),
        MediaError::TooLong(_)
    ));
}

#[test]
fn a_tgs_decompression_bomb_stops_at_the_inflate_cap() {
    // About 16 KB that would inflate to 16 MiB (deflate tops out near 1000:1).
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    let zeros = vec![b' '; 1 << 20];
    for _ in 0..16 {
        gz.write_all(&zeros).unwrap();
    }
    let bomb = gz.finish().unwrap();
    assert!(
        bomb.len() < MAX_ANIMATION_BYTES,
        "small enough to pass the byte cap: {}",
        bomb.len()
    );
    assert_eq!(media_err(&bomb), MediaError::Inflated);

    // Exactly at the cap inflates, then is judged as Lottie (spaces are not).
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(&vec![b' '; MAX_TGS_INFLATED]).unwrap();
    assert!(matches!(
        media_err(&gz.finish().unwrap()),
        MediaError::BadLottie(_)
    ));
}

#[test]
fn a_tgs_with_bytes_after_the_gzip_member_is_refused() {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(&telegram_lottie()).unwrap();
    let tgs = [gz.finish().unwrap(), b"tail".to_vec()].concat();
    assert_eq!(media_err(&tgs), MediaError::BadGzip);
}

// ---- WebM ------------------------------------------------------------------

/// One EBML element, its size written as an 8-byte vint.
fn el(id: u32, body: &[u8]) -> Vec<u8> {
    let id_bytes: Vec<u8> = id
        .to_be_bytes()
        .into_iter()
        .skip_while(|&b| b == 0)
        .collect();
    let mut size = (body.len() as u64).to_be_bytes();
    size[0] = 0x01;
    [id_bytes, size.to_vec(), body.to_vec()].concat()
}

fn uint_el(id: u32, v: u64) -> Vec<u8> {
    el(id, &v.to_be_bytes())
}

struct Clip {
    ms: f64,
    frames: usize,
    side: u64,
    audio: bool,
    frame_ns: Option<u64>,
    /// Bytes of a Void element, to size the file.
    void: usize,
}

impl Clip {
    fn telegram() -> Self {
        Self {
            ms: 3_000.0,
            frames: 90,
            side: 512,
            audio: false,
            frame_ns: Some(33_333_333),
            void: 0,
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let header = el(0x1A45_DFA3, &el(0x4282, b"webm"));
        let info = el(
            0x1549_A966,
            &[
                uint_el(0x2A_D7B1, 1_000_000),
                el(0x4489, &self.ms.to_be_bytes()),
            ]
            .concat(),
        );
        let mut video = [uint_el(0xD7, 1), uint_el(0x83, 1)].concat();
        if let Some(ns) = self.frame_ns {
            video.extend(uint_el(0x23_E383, ns));
        }
        video.extend(el(
            0xE0,
            &[uint_el(0xB0, self.side), uint_el(0xBA, self.side)].concat(),
        ));
        let mut tracks = el(0xAE, &video);
        if self.audio {
            tracks.extend(el(0xAE, &[uint_el(0xD7, 2), uint_el(0x83, 2)].concat()));
        }
        let mut cluster = uint_el(0xE7, 0);
        for i in 0..self.frames {
            // track 1 (vint 0x81), timecode, flags (keyframe, no lacing), a byte of "frame"
            cluster.extend(el(
                0xA3,
                &[0x81, 0, u8::try_from(i % 256).unwrap(), 0x80, 0],
            ));
        }
        let mut segment = [info, el(0x1654_AE6B, &tracks), el(0x1F43_B675, &cluster)].concat();
        if self.void > 0 {
            segment.extend(el(0xEC, &vec![0; self.void]));
        }
        [header, el(0x1853_8067, &segment)].concat()
    }

    /// The file padded with a Void element to exactly `len` bytes.
    fn of_len(mut self, len: usize) -> Vec<u8> {
        self.void = 1;
        let base = self.bytes().len();
        self.void = len - base + 1;
        let out = self.bytes();
        assert_eq!(out.len(), len);
        out
    }
}

#[test]
fn a_telegram_sized_webm_imports() {
    assert_eq!(
        check_file(&Clip::telegram().bytes()).unwrap().1,
        Format::Webm
    );
    assert_eq!(
        media::check(Format::Webm, &Clip::telegram().bytes()).unwrap(),
        Some((512, 512))
    );
}

#[test]
fn webm_at_120_kb_passes_and_one_byte_over_is_refused() {
    assert!(check_file(&Clip::telegram().of_len(MAX_STICKER_BYTES)).is_ok());
    assert!(matches!(
        check_file(&Clip::telegram().of_len(MAX_STICKER_BYTES + 1)).unwrap_err(),
        FrameError::FileTooLarge(_)
    ));
}

#[test]
fn webm_at_three_seconds_passes_and_one_millisecond_over_is_refused() {
    assert!(
        check_file(
            &Clip {
                ms: 3_000.0,
                ..Clip::telegram()
            }
            .bytes()
        )
        .is_ok()
    );
    assert!(matches!(
        media_err(
            &Clip {
                ms: 3_001.0,
                ..Clip::telegram()
            }
            .bytes()
        ),
        MediaError::TooLong(_)
    ));
}

#[test]
fn webm_at_90_frames_and_30_fps_passes_and_one_frame_over_is_refused() {
    let frames = usize::try_from(MAX_WEBM_FRAMES).unwrap();
    assert!(
        check_file(
            &Clip {
                frames,
                ..Clip::telegram()
            }
            .bytes()
        )
        .is_ok()
    );
    assert!(matches!(
        media_err(
            &Clip {
                frames: frames + 1,
                ..Clip::telegram()
            }
            .bytes()
        ),
        MediaError::TooManyFrames(91)
    ));
    // A per-frame duration under 1/30 s is over 30 fps.
    assert!(matches!(
        media_err(
            &Clip {
                frame_ns: Some(33_333_332),
                ..Clip::telegram()
            }
            .bytes()
        ),
        MediaError::TooFast(30, _)
    ));
    // Without one, the average: 31 frames in a second pass, 32 do not.
    assert!(
        check_file(
            &Clip {
                ms: 1_000.0,
                frames: 31,
                frame_ns: None,
                ..Clip::telegram()
            }
            .bytes()
        )
        .is_ok()
    );
    assert!(matches!(
        media_err(
            &Clip {
                ms: 1_000.0,
                frames: 32,
                frame_ns: None,
                ..Clip::telegram()
            }
            .bytes()
        ),
        MediaError::TooFast(30, _)
    ));
}

#[test]
fn webm_with_an_audio_track_is_refused() {
    assert_eq!(
        media_err(
            &Clip {
                audio: true,
                ..Clip::telegram()
            }
            .bytes()
        ),
        MediaError::Audio
    );
}

#[test]
fn webm_at_512_px_passes_and_513_is_refused() {
    assert!(matches!(
        media_err(
            &Clip {
                side: 513,
                ..Clip::telegram()
            }
            .bytes()
        ),
        MediaError::TooBig(512, 513, 513)
    ));
}

#[test]
fn truncated_or_lying_webm_is_refused_without_panicking() {
    let full = Clip::telegram().bytes();
    for cut in [5, 40, 60, full.len() / 2, full.len() - 1] {
        assert!(check_file(&full[..cut]).is_err(), "cut at {cut}");
    }
    // A size field claiming more than the file holds.
    let mut lying = full.clone();
    let at = lying
        .windows(4)
        .position(|w| w == [0x18, 0x53, 0x80, 0x67])
        .unwrap()
        + 4;
    lying[at..at + 8].copy_from_slice(&[0x01, 0, 0, 0, 0, 0xff, 0xff, 0xff]);
    assert!(check_file(&lying).is_err());
}

// ---- PNG, WebP, GIF ----------------------------------------------------------

fn png(w: u32, h: u32, len: usize) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    out.extend(w.to_be_bytes());
    out.extend(h.to_be_bytes());
    out.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    out.resize(len, 0);
    out
}

fn webp_vp8x(w: u32, h: u32) -> Vec<u8> {
    let mut out = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
    out.extend(&(w - 1).to_le_bytes()[..3]);
    out.extend(&(h - 1).to_le_bytes()[..3]);
    out
}

fn gif(w: u16, h: u16) -> Vec<u8> {
    [
        b"GIF89a".as_slice(),
        &w.to_le_bytes(),
        &h.to_le_bytes(),
        &[0, 0, 0],
        b";",
    ]
    .concat()
}

#[test]
fn png_at_120_kb_and_512_px_passes_and_one_over_is_refused() {
    assert_eq!(
        check_file(&png(512, 512, MAX_STICKER_BYTES)).unwrap().1,
        Format::Png
    );
    assert!(matches!(
        check_file(&png(512, 512, MAX_STICKER_BYTES + 1)).unwrap_err(),
        FrameError::FileTooLarge(_)
    ));
    assert!(matches!(
        media_err(&png(513, 512, 100)),
        MediaError::TooBig(512, 513, 512)
    ));
    assert_eq!(media_err(&png(0, 512, 100)), MediaError::BadImage);
}

#[test]
fn webp_and_gif_at_512_px_pass_and_513_is_refused() {
    assert_eq!(check_file(&webp_vp8x(512, 512)).unwrap().1, Format::Webp);
    assert!(matches!(
        media_err(&webp_vp8x(512, 513)),
        MediaError::TooBig(512, 512, 513)
    ));
    assert!(check_file(&gif(512, 512)).is_ok());
    assert!(matches!(
        media_err(&gif(513, 1)),
        MediaError::TooBig(512, 513, 1)
    ));
}

#[test]
fn a_disguised_extension_is_read_as_what_the_bytes_are() {
    // There is no file name anywhere in the check: a WebM someone named
    // `cute.png` is judged as WebM, and its audio track still refuses it.
    let webm_named_png = Clip {
        audio: true,
        ..Clip::telegram()
    }
    .bytes();
    assert_eq!(Format::sniff(&webm_named_png), Some(Format::Webm));
    assert_eq!(media_err(&webm_named_png), MediaError::Audio);
    // A program named `.png` is no sticker format at all.
    assert_eq!(
        check_file(b"MZ\x90\0\x03\0\0\0 a program").unwrap_err(),
        FrameError::UnknownFormat
    );
    // A PNG signature with no IHDR behind it is refused, not trusted.
    assert_eq!(
        media_err(b"\x89PNG\r\n\x1a\nnot a png at all"),
        MediaError::BadImage
    );
}

#[test]
fn a_custom_emoji_is_at_most_100_px_a_side() {
    assert!(media::check_emoji(Format::Png, &png(100, 100, 64)).is_ok());
    assert_eq!(
        media::check_emoji(Format::Png, &png(101, 100, 64)).unwrap_err(),
        MediaError::TooBig(100, 101, 100)
    );
    assert!(media::check_emoji(Format::Lottie, &lottie(60.0, 60.0, 101, 2_048)).is_err());
    assert!(
        media::check_emoji(
            Format::Webm,
            &Clip {
                side: 100,
                ..Clip::telegram()
            }
            .bytes()
        )
        .is_ok()
    );
    assert!(
        media::check_emoji(Format::Svg, br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#).is_ok(),
        "vector: no pixel size"
    );
}

#[test]
fn every_refusal_says_why_in_words() {
    let msg = |b: &[u8]| check_file(b).unwrap_err().to_string();
    assert!(
        msg(&Clip {
            audio: true,
            ..Clip::telegram()
        }
        .bytes())
        .contains("audio track")
    );
    assert!(msg(&lottie(60.0, 181.0, 512, 4_096)).contains("3 seconds"));
    assert!(msg(&png(600, 600, 64)).contains("512×512 pixels"));
    assert!(msg(&lottie(60.0, 60.0, 512, MAX_ANIMATION_BYTES + 1)).contains("64 KB"));
}
