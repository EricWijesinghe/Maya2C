//! Per-format sticker caps (ADR-140 section 2, build PR 3).
//!
//! [`crate::frame::check_file`] keeps the 120 KB ceiling every format shares
//! (set by the 256 KiB relay envelope) and calls [`check`] for the rest. The
//! format comes from [`Format::sniff`], which reads the bytes: a WebM named
//! `.png` is checked as WebM, and a file that is no known format is refused
//! before this runs.
//!
//! The Lottie and WebM figures mirror Telegram's published sticker limits
//! (Lottie/TGS 64 KB, 3 s, 60 fps; WebM 3 s, 30 fps, no audio; 512 px), so
//! files made for Telegram import unchanged. The ones Telegram does not
//! publish (inflated size, nesting depth) are bounds chosen here, named as
//! such below.
//!
//! Everything here is bounded: a TGS is inflated through a reader that stops
//! one byte past [`MAX_TGS_INFLATED`], Lottie nesting is counted before any
//! parse, and the WebM walk only ever moves forward through a slice.

use std::io::Read as _;

use crate::frame::Format;

/// Lottie JSON, and a TGS before it is inflated (Telegram: 64 KB).
pub const MAX_ANIMATION_BYTES: usize = 64 << 10;
/// What a TGS may inflate to. Chosen bound, not a Telegram figure: sixteen
/// times the compressed cap, so a well-compressed real animation fits and a
/// decompression bomb stops at a size the app can hold.
pub const MAX_TGS_INFLATED: usize = 1 << 20;
/// Nesting of objects and arrays in a Lottie file. Chosen bound: serde_json
/// stops at 128 on its own, and lottie-web walks the tree recursively.
pub const MAX_LOTTIE_DEPTH: usize = 64;
/// Length of an animated or video sticker, milliseconds (Telegram: 3 s).
pub const MAX_DURATION_MS: u64 = 3_000;
/// Lottie frame rate (Telegram: 60).
pub const MAX_LOTTIE_FPS: u64 = 60;
/// WebM frame rate (Telegram: 30).
pub const MAX_WEBM_FPS: u64 = 30;
// The same caps as floats, for the arithmetic on Lottie's and Matroska's
// float fields. This is app code, not consensus code.
const MAX_DURATION_MS_F: f64 = 3_000.0;
const MAX_LOTTIE_FPS_F: f64 = 60.0;
const MAX_WEBM_FPS_F: f64 = 30.0;
const NS_PER_S: u64 = 1_000_000_000;
/// WebM frames: [`MAX_WEBM_FPS`] for [`MAX_DURATION_MS`].
pub const MAX_WEBM_FRAMES: u64 = MAX_WEBM_FPS * MAX_DURATION_MS / 1_000;
/// The longest side of a sticker, pixels (Telegram: 512).
pub const MAX_SIDE: u64 = 512;
/// The longest side of a custom emoji, pixels (Telegram: 100).
pub const MAX_EMOJI_SIDE: u64 = 100;

const NS_PER_MS: f64 = 1_000_000.0;
const DEFAULT_TIMECODE_SCALE: u64 = 1_000_000;

/// Why a sticker file was refused, in words a person can act on.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MediaError {
    #[error("a {0} sticker can be at most {max} KB; this one is {1} KB", max = MAX_ANIMATION_BYTES >> 10)]
    AnimationTooLarge(&'static str, usize),
    #[error("the .tgs file unpacks to more than {} KB, so it was not opened", MAX_TGS_INFLATED >> 10)]
    Inflated,
    #[error("the .tgs file is not readable gzip")]
    BadGzip,
    #[error("the animation is nested more than {MAX_LOTTIE_DEPTH} levels deep")]
    TooDeep,
    #[error("not a usable Lottie animation: {0}")]
    BadLottie(&'static str),
    #[error("a moving sticker can last at most 3 seconds; this one lasts {0}")]
    TooLong(String),
    #[error("a moving sticker can play at most {0} frames per second; this one plays {1}")]
    TooFast(u64, String),
    #[error("a video sticker can have at most {MAX_WEBM_FRAMES} frames; this one has {0}")]
    TooManyFrames(u64),
    #[error("a sticker can be at most {0}×{0} pixels; this one is {1}×{2}")]
    TooBig(u64, u64, u64),
    #[error("a video sticker plays without sound; this one has an audio track")]
    Audio,
    #[error("not a usable WebM video: {0}")]
    BadWebm(&'static str),
    #[error("the image's size could not be read")]
    BadImage,
}

/// Checks `bytes`, already sniffed as `format` and under the 120 KB ceiling,
/// against that format's caps. Returns its pixel size; `None` for SVG, which
/// has none.
///
/// # Errors
///
/// The first cap the file breaks.
pub fn check(format: Format, bytes: &[u8]) -> Result<Option<(u64, u64)>, MediaError> {
    let size = match format {
        Format::Lottie => {
            within_animation_cap("Lottie", bytes)?;
            lottie(bytes)?
        }
        Format::Tgs => {
            within_animation_cap(".tgs", bytes)?;
            lottie(&inflate(bytes)?)?
        }
        Format::Webm => webm(bytes)?,
        Format::Png => png(bytes)?,
        Format::Gif => gif(bytes)?,
        Format::Webp => webp(bytes)?,
        Format::Svg => return Ok(None),
    };
    within_side(size, MAX_SIDE)?;
    Ok(Some(size))
}

/// [`check`], and the custom emoji cap: at most [`MAX_EMOJI_SIDE`] a side.
/// SVG has no pixel size and is drawn at the text's size.
///
/// # Errors
///
/// As [`check`], or [`MediaError::TooBig`].
pub fn check_emoji(format: Format, bytes: &[u8]) -> Result<(), MediaError> {
    match check(format, bytes)? {
        Some(size) => within_side(size, MAX_EMOJI_SIDE),
        None => Ok(()),
    }
}

fn within_animation_cap(name: &'static str, bytes: &[u8]) -> Result<(), MediaError> {
    if bytes.len() > MAX_ANIMATION_BYTES {
        return Err(MediaError::AnimationTooLarge(
            name,
            bytes.len().div_ceil(1024),
        ));
    }
    Ok(())
}

fn within_side((w, h): (u64, u64), max: u64) -> Result<(), MediaError> {
    if w == 0 || h == 0 {
        return Err(MediaError::BadImage);
    }
    if w > max || h > max {
        return Err(MediaError::TooBig(max, w, h));
    }
    Ok(())
}

/// Inflates a TGS, stopping one byte past [`MAX_TGS_INFLATED`]. Bytes after
/// the gzip member are refused: the window's `DecompressionStream` refuses
/// them too, so a file this accepts is one the window can draw.
fn inflate(bytes: &[u8]) -> Result<Vec<u8>, MediaError> {
    let limit = u64::try_from(MAX_TGS_INFLATED + 1).unwrap_or(u64::MAX);
    let mut out = Vec::new();
    let mut decoder = flate2::bufread::GzDecoder::new(bytes);
    (&mut decoder)
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|_| MediaError::BadGzip)?;
    if out.len() > MAX_TGS_INFLATED {
        return Err(MediaError::Inflated);
    }
    if !decoder.into_inner().is_empty() {
        return Err(MediaError::BadGzip);
    }
    Ok(out)
}

/// The deepest nesting of objects and arrays, counted outside strings,
/// refusing as soon as it passes [`MAX_LOTTIE_DEPTH`].
fn check_depth(json: &[u8]) -> Result<(), MediaError> {
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    for &b in json {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > MAX_LOTTIE_DEPTH {
                    return Err(MediaError::TooDeep);
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

/// A Lottie animation's frame rate, length and size.
fn lottie(json: &[u8]) -> Result<(u64, u64), MediaError> {
    check_depth(json)?;
    let v: serde_json::Value =
        serde_json::from_slice(json).map_err(|_| MediaError::BadLottie("not JSON"))?;
    if !v.get("layers").is_some_and(serde_json::Value::is_array) {
        return Err(MediaError::BadLottie("no layers"));
    }
    let num = |key: &str| v.get(key).and_then(serde_json::Value::as_f64);
    let (Some(fr), Some(ip), Some(op)) = (num("fr"), num("ip"), num("op")) else {
        return Err(MediaError::BadLottie(
            "no frame rate, in point or out point",
        ));
    };
    if fr <= 0.0 || op <= ip {
        return Err(MediaError::BadLottie(
            "the frame rate or the in and out points are not usable",
        ));
    }
    if fr > MAX_LOTTIE_FPS_F {
        return Err(MediaError::TooFast(MAX_LOTTIE_FPS, format!("{fr}")));
    }
    let frames = op - ip;
    // `frames <= 3 s × fr`: 180 frames at 60 fps pass, 181 do not.
    if frames * 1_000.0 > fr * MAX_DURATION_MS_F {
        return Err(MediaError::TooLong(format!("{:.3} s", frames / fr)));
    }
    let side = |key: &str| v.get(key).and_then(serde_json::Value::as_u64);
    match (side("w"), side("h")) {
        (Some(w), Some(h)) => Ok((w, h)),
        _ => Err(MediaError::BadLottie("no whole-pixel width and height")),
    }
}

fn be32(bytes: &[u8], at: usize) -> Option<u64> {
    let b: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(u64::from(u32::from_be_bytes(b)))
}

fn le16(bytes: &[u8], at: usize) -> Option<u64> {
    let b: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
    Some(u64::from(u16::from_le_bytes(b)))
}

fn le24(bytes: &[u8], at: usize) -> Option<u64> {
    let b = bytes.get(at..at + 3)?;
    Some(u64::from(b[0]) | (u64::from(b[1]) << 8) | (u64::from(b[2]) << 16))
}

/// PNG and APNG: the IHDR chunk, which the format requires first.
fn png(bytes: &[u8]) -> Result<(u64, u64), MediaError> {
    if bytes.get(12..16) != Some(b"IHDR") {
        return Err(MediaError::BadImage);
    }
    be32(bytes, 16)
        .zip(be32(bytes, 20))
        .ok_or(MediaError::BadImage)
}

/// GIF: the logical screen size.
fn gif(bytes: &[u8]) -> Result<(u64, u64), MediaError> {
    le16(bytes, 6)
        .zip(le16(bytes, 8))
        .ok_or(MediaError::BadImage)
}

/// WebP: the canvas of an extended file, or the frame of a simple one.
fn webp(bytes: &[u8]) -> Result<(u64, u64), MediaError> {
    let size = match bytes.get(12..16) {
        Some(b"VP8X") => le24(bytes, 24)
            .zip(le24(bytes, 27))
            .map(|(w, h)| (w + 1, h + 1)),
        Some(b"VP8 ") if bytes.get(23..26) == Some(&[0x9d, 0x01, 0x2a][..]) => le16(bytes, 26)
            .zip(le16(bytes, 28))
            .map(|(w, h)| (w & 0x3fff, h & 0x3fff)),
        Some(b"VP8L") if bytes.get(20) == Some(&0x2f) => bytes.get(21..25).map(|b| {
            let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            (
                u64::from(bits & 0x3fff) + 1,
                u64::from((bits >> 14) & 0x3fff) + 1,
            )
        }),
        _ => None,
    };
    size.ok_or(MediaError::BadImage)
}

// ---- WebM -------------------------------------------------------------------

const ID_SEGMENT: u32 = 0x1853_8067;
const ID_INFO: u32 = 0x1549_A966;
const ID_TIMECODE_SCALE: u32 = 0x2A_D7B1;
const ID_DURATION: u32 = 0x4489;
const ID_TRACKS: u32 = 0x1654_AE6B;
const ID_TRACK_ENTRY: u32 = 0xAE;
const ID_TRACK_NUMBER: u32 = 0xD7;
const ID_TRACK_TYPE: u32 = 0x83;
const ID_DEFAULT_DURATION: u32 = 0x23_E383;
const ID_VIDEO: u32 = 0xE0;
const ID_PIXEL_WIDTH: u32 = 0xB0;
const ID_PIXEL_HEIGHT: u32 = 0xBA;
const ID_CLUSTER: u32 = 0x1F43_B675;
const ID_SIMPLE_BLOCK: u32 = 0xA3;
const ID_BLOCK_GROUP: u32 = 0xA0;
const ID_BLOCK: u32 = 0xA1;
const TRACK_VIDEO: u64 = 1;
const TRACK_AUDIO: u64 = 2;
const LACING_BITS: u8 = 0x06;

/// One EBML element: its id, where its data starts and ends.
struct Element {
    id: u32,
    start: usize,
    end: usize,
    /// Its size field was "unknown" (all ones), as a live writer leaves it.
    open: bool,
}

/// A variable-length integer at `at`: (value with the marker removed, its
/// length, whether every value bit was one).
fn vint(data: &[u8], at: usize) -> Option<(u64, usize, bool)> {
    let first = *data.get(at)?;
    let len = usize::try_from(first.leading_zeros()).ok()? + 1;
    if len > 8 {
        return None;
    }
    let mut value = u64::from(first) & (0xff >> len);
    for &b in data.get(at + 1..at + len)? {
        value = (value << 8) | u64::from(b);
    }
    let all_ones = value == (1u64 << (7 * len)) - 1;
    Some((value, len, all_ones))
}

/// The element at `at`, inside a parent ending at `limit`.
fn element(data: &[u8], at: usize, limit: usize) -> Result<Element, MediaError> {
    let bad = || MediaError::BadWebm("an element runs past its container");
    let first = *data.get(at).ok_or_else(bad)?;
    let id_len = usize::try_from(first.leading_zeros()).map_err(|_| bad())? + 1;
    if id_len > 4 {
        return Err(MediaError::BadWebm("an element id is malformed"));
    }
    let id = data
        .get(at..at + id_len)
        .ok_or_else(bad)?
        .iter()
        .fold(0u32, |acc, &b| (acc << 8) | u32::from(b));
    let (size, size_len, open) = vint(data, at + id_len).ok_or_else(bad)?;
    let start = at + id_len + size_len;
    let end = if open {
        limit
    } else {
        start
            .checked_add(usize::try_from(size).map_err(|_| bad())?)
            .ok_or_else(bad)?
    };
    if start > limit || end > limit {
        return Err(bad());
    }
    Ok(Element {
        id,
        start,
        end,
        open,
    })
}

fn uint(data: &[u8], e: &Element) -> Result<u64, MediaError> {
    let body = &data[e.start..e.end];
    if body.len() > 8 {
        return Err(MediaError::BadWebm("an integer is longer than 8 bytes"));
    }
    Ok(body.iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b)))
}

fn float(data: &[u8], e: &Element) -> Result<f64, MediaError> {
    let body = &data[e.start..e.end];
    match body.len() {
        4 => Ok(f64::from(f32::from_be_bytes(
            body.try_into()
                .map_err(|_| MediaError::BadWebm("bad float"))?,
        ))),
        8 => Ok(f64::from_be_bytes(
            body.try_into()
                .map_err(|_| MediaError::BadWebm("bad float"))?,
        )),
        _ => Err(MediaError::BadWebm(
            "the duration is not a 4- or 8-byte float",
        )),
    }
}

/// What the walk found.
#[derive(Default)]
struct Webm {
    scale: Option<u64>,
    duration: Option<f64>,
    video: Option<u64>,
    default_duration: Option<u64>,
    size: Option<(u64, u64)>,
    frames: u64,
}

/// Children of the element spanning `start..end`, in order.
fn children(
    data: &[u8],
    start: usize,
    end: usize,
    mut each: impl FnMut(&Element) -> Result<(), MediaError>,
) -> Result<(), MediaError> {
    let mut at = start;
    while at < end {
        let e = element(data, at, end)?;
        if e.open {
            return Err(MediaError::BadWebm(
                "an element of unknown size inside the header",
            ));
        }
        each(&e)?;
        at = e.end;
    }
    Ok(())
}

fn read_track(data: &[u8], entry: &Element, out: &mut Webm) -> Result<(), MediaError> {
    let (mut number, mut kind, mut dd, mut size) = (None, None, None, (None, None));
    children(data, entry.start, entry.end, |e| {
        match e.id {
            ID_TRACK_NUMBER => number = Some(uint(data, e)?),
            ID_TRACK_TYPE => kind = Some(uint(data, e)?),
            ID_DEFAULT_DURATION => dd = Some(uint(data, e)?),
            ID_VIDEO => children(data, e.start, e.end, |v| {
                match v.id {
                    ID_PIXEL_WIDTH => size.0 = Some(uint(data, v)?),
                    ID_PIXEL_HEIGHT => size.1 = Some(uint(data, v)?),
                    _ => {}
                }
                Ok(())
            })?,
            _ => {}
        }
        Ok(())
    })?;
    match kind {
        Some(TRACK_AUDIO) => Err(MediaError::Audio),
        Some(TRACK_VIDEO) => {
            if out.video.is_some() {
                return Err(MediaError::BadWebm("more than one video track"));
            }
            out.video = Some(number.ok_or(MediaError::BadWebm("a track without a number"))?);
            out.default_duration = dd;
            out.size = size.0.zip(size.1);
            Ok(())
        }
        _ => Err(MediaError::BadWebm(
            "a track that is neither video nor audio",
        )),
    }
}

/// Counts one block if it belongs to the video track; laced video is refused,
/// since its frame count is not in the block header.
fn count_block(data: &[u8], block: &Element, out: &mut Webm) -> Result<(), MediaError> {
    let (track, len, _) =
        vint(data, block.start).ok_or(MediaError::BadWebm("a block without a track"))?;
    let flags = *data
        .get(block.start + len + 2)
        .filter(|_| block.start + len + 2 < block.end)
        .ok_or(MediaError::BadWebm("a block is too short"))?;
    if Some(track) == out.video {
        if flags & LACING_BITS != 0 {
            return Err(MediaError::BadWebm("laced video frames"));
        }
        out.frames += 1;
    }
    Ok(())
}

/// One cluster's blocks. A cluster of unknown size ends where the next
/// top-level element starts; returns where the walk resumes.
fn read_cluster(data: &[u8], cluster: &Element, out: &mut Webm) -> Result<usize, MediaError> {
    let mut at = cluster.start;
    while at < cluster.end {
        let e = element(data, at, cluster.end)?;
        match e.id {
            ID_CLUSTER | ID_INFO | ID_TRACKS if cluster.open => return Ok(at),
            ID_SIMPLE_BLOCK => count_block(data, &e, out)?,
            ID_BLOCK_GROUP => children(data, e.start, e.end, |b| {
                if b.id == ID_BLOCK {
                    count_block(data, b, out)?;
                }
                Ok(())
            })?,
            _ if e.open => {
                return Err(MediaError::BadWebm(
                    "an element of unknown size inside a cluster",
                ));
            }
            _ => {}
        }
        at = e.end;
    }
    Ok(at)
}

fn read_segment(data: &[u8], segment: &Element, out: &mut Webm) -> Result<(), MediaError> {
    let mut at = segment.start;
    while at < segment.end {
        let e = element(data, at, segment.end)?;
        at = match e.id {
            ID_CLUSTER => read_cluster(data, &e, out)?,
            _ if e.open => {
                return Err(MediaError::BadWebm(
                    "an element of unknown size in the segment",
                ));
            }
            ID_INFO => {
                children(data, e.start, e.end, |i| {
                    match i.id {
                        ID_TIMECODE_SCALE => out.scale = Some(uint(data, i)?),
                        ID_DURATION => out.duration = Some(float(data, i)?),
                        _ => {}
                    }
                    Ok(())
                })?;
                e.end
            }
            ID_TRACKS => {
                children(data, e.start, e.end, |t| {
                    if t.id == ID_TRACK_ENTRY {
                        read_track(data, t, out)?;
                    }
                    Ok(())
                })?;
                e.end
            }
            _ => e.end,
        };
    }
    Ok(())
}

/// WebM: no audio track, one video track, at most 3 s, 30 fps and
/// [`MAX_WEBM_FRAMES`] frames. Returns the video's pixel size.
fn webm(data: &[u8]) -> Result<(u64, u64), MediaError> {
    let mut out = Webm::default();
    let mut at = 0;
    while at < data.len() {
        let e = element(data, at, data.len())?;
        if e.id == ID_SEGMENT {
            read_segment(data, &e, &mut out)?;
        }
        at = e.end;
    }
    if out.video.is_none() {
        return Err(MediaError::BadWebm("no video track"));
    }
    let duration = out.duration.ok_or(MediaError::BadWebm("no duration"))?;
    let scale = out.scale.unwrap_or(DEFAULT_TIMECODE_SCALE);
    let scale_f = f64::from(
        u32::try_from(scale).map_err(|_| MediaError::BadWebm("timecode scale too large"))?,
    );
    let ms = duration * scale_f / NS_PER_MS;
    if ms.is_nan() || ms <= 0.0 {
        return Err(MediaError::BadWebm("the duration is not positive"));
    }
    if ms > MAX_DURATION_MS_F {
        return Err(MediaError::TooLong(format!("{ms:.0} ms")));
    }
    // A frame shorter than 1/30 s is over 30 fps. Writers store 30 fps as
    // 33,333,333 ns, so one nanosecond per frame of rounding is allowed.
    // `dd` is then under 33,333,334 ns and fits a u32.
    if let Some(dd) = out.default_duration
        && dd.saturating_add(1).saturating_mul(MAX_WEBM_FPS) < NS_PER_S
    {
        let fps = 1e9 / f64::from(u32::try_from(dd.max(1)).unwrap_or(u32::MAX));
        return Err(MediaError::TooFast(MAX_WEBM_FPS, format!("{fps:.1}")));
    }
    if out.frames > MAX_WEBM_FRAMES {
        return Err(MediaError::TooManyFrames(out.frames));
    }
    // Without a per-frame duration, the average rate: frames over length,
    // one frame of slack for the frame at time zero.
    let frames = f64::from(u32::try_from(out.frames).unwrap_or(u32::MAX));
    if frames - 1.0 > ms * MAX_WEBM_FPS_F / 1_000.0 {
        return Err(MediaError::TooFast(
            MAX_WEBM_FPS,
            format!("{:.1}", frames * 1_000.0 / ms),
        ));
    }
    out.size
        .ok_or(MediaError::BadWebm("no pixel width and height"))
}
