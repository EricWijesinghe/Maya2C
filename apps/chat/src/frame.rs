//! Structured messages: stickers, sticker packs, reactions and replies.
//!
//! Shared by the CLI and the desktop app (ADR-140, build PR 1; moved here
//! from `apps/chat-gui/src-tauri/src/stickers.rs`, which re-exports it). A
//! frame is a plaintext like any text, so it travels inside the end-to-end
//! encryption and a relay sees one more envelope of a padded size, nothing
//! else:
//!
//! ```text
//! MAGIC ‖ header_len:u32 LE ‖ header (JSON, space-padded) ‖ body
//! ```
//!
//! The header is padded with trailing spaces, which JSON ignores, so the
//! whole frame is a multiple of [`PAD_BUCKET`] bytes: a reaction and a short
//! reply seal to the same size. Older apps parse the padded header unchanged.
//!
//! A sticker's body is its file, named by its BLAKE3 hash; the receiver
//! re-hashes it and refuses a mismatch, so a sticker can never be swapped in
//! transit or in a cache. A reply's body is its text. Everything else has an
//! empty body. A kind this build does not know is refused as
//! [`FrameError::UnknownKind`], never parsed further.

use serde::{Deserialize, Serialize};

/// Starts every structured message. Plain text cannot start with NUL: the
/// desktop app refuses to send one.
pub const MAGIC: &[u8] = b"\0MAYA-CHAT-1";
/// A frame body (a sticker file or a reply's text), in bytes: what fits one
/// relay envelope (256 KiB) in the worst case, a first message carrying the
/// session handshake. The envelope's CBOR writes ciphertext as an integer
/// array, about two bytes per byte, so a 120 KiB file seals to 247,427 bytes
/// (measured by `the_largest_sticker_crosses_real_encryption_within_one_envelope`
/// before padding was added; padding adds under one bucket).
/// Telegram's animated stickers are at most 64 KB.
pub const MAX_STICKER_BYTES: usize = 120 << 10;
/// The JSON header before padding, in bytes. Titles and emoji are short.
pub const MAX_HEADER: usize = 4 << 10;
/// Frames are padded to a multiple of this many bytes before sealing.
pub const PAD_BUCKET: usize = 256;
/// Stickers in one pack (Telegram allows 120).
pub const MAX_PACK_ITEMS: usize = 120;
/// Characters in a pack title.
pub const MAX_TITLE_CHARS: usize = 64;
/// The reactions the apps offer. A custom emoji is named by its 64-hex
/// sticker id instead (custom emoji packs are ADR-140 build PR 3).
pub const REACTION_EMOJI: [&str; 8] = ["👍", "❤️", "😂", "😮", "😢", "🙏", "🔥", "🎉"];
/// The longest reaction value, in bytes: a custom emoji's hex id.
pub const MAX_EMOJI_BYTES: usize = 64;
/// Reaction marks one message keeps; a new (reactor, emoji) past this is
/// dropped, so a peer cannot grow a line without bound.
pub const MAX_MARKS: usize = 32;
const QUOTE_DOMAIN: &str = "maya-chat 2026-11-13 reply quote v1";
/// The kinds this build reads; any other is [`FrameError::UnknownKind`].
const KNOWN_KINDS: [&str; 5] = ["sticker", "pack_request", "pack_item", "reaction", "reply"];

/// Why a frame was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("not a structured message")]
    NotStructured,
    #[error("truncated message")]
    Truncated,
    #[error("malformed message header")]
    Header,
    #[error("malformed message: {0}")]
    Malformed(String),
    /// A newer app's frame kind.
    #[error("this message needs a newer Maya2C Chat")]
    UnknownKind,
    #[error("the message's details are too long")]
    HeaderTooLong,
    #[error("the file or text is too large")]
    BodyTooLarge,
    #[error("unexpected bytes after the message")]
    Trailing,
    #[error("the sticker file is empty")]
    EmptyFile,
    #[error("a sticker can be at most {max} KB; this one is {0} KB", max = MAX_STICKER_BYTES >> 10)]
    FileTooLarge(usize),
    #[error("not a sticker format: use Lottie (.json or .tgs), WebM, WebP, PNG, GIF or SVG")]
    UnknownFormat,
    #[error("the sticker does not match its id")]
    StickerMismatch,
    #[error("the pack does not match its id")]
    PackMismatch,
    #[error("the pack item is not in its pack")]
    NotInPack,
    #[error("a message id is 64 hex characters")]
    BadId,
    #[error("not a reaction this app accepts")]
    BadEmoji,
    #[error("a reply needs text, without NUL characters")]
    BadText,
}

/// The desktop app's commands report errors as strings.
impl From<FrameError> for String {
    fn from(e: FrameError) -> Self {
        e.to_string()
    }
}

/// What a sticker file is, read from its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// Lottie animation, JSON.
    Lottie,
    /// Lottie, gzipped (Telegram's `.tgs`).
    Tgs,
    /// Video sticker (VP8/VP9, alpha where the platform supports it).
    Webm,
    /// Still or animated WebP.
    Webp,
    /// Still or animated (APNG) PNG.
    Png,
    /// Animated or still GIF.
    Gif,
    /// SVG, drawn only as an `<img>`, where no script in it can run.
    Svg,
}

impl Format {
    /// Reads the format from the file itself.
    #[must_use]
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        let head = &bytes[..bytes.len().min(512)];
        let text = String::from_utf8_lossy(head);
        let trimmed = text.trim_start_matches('\u{feff}').trim_start();
        if bytes.starts_with(&[0x1f, 0x8b]) {
            Some(Self::Tgs)
        } else if bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
            Some(Self::Webm)
        } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some(Self::Webp)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Some(Self::Gif)
        } else if trimmed.starts_with("<svg")
            || (trimmed.starts_with("<?xml") && text.contains("<svg"))
        {
            Some(Self::Svg)
        } else if trimmed.starts_with('{')
            && serde_json::from_slice::<serde_json::Value>(bytes)
                .is_ok_and(|v| v.get("layers").is_some())
        {
            Some(Self::Lottie)
        } else {
            None
        }
    }
}

/// A sticker's id: the hex BLAKE3 hash of its file.
#[must_use]
pub fn sticker_id(bytes: &[u8]) -> String {
    hex::encode(blake3::hash(bytes).as_bytes())
}

/// Checks a file can be a sticker, returning its id and format.
///
/// # Errors
///
/// Empty, too large, or not a format a sticker can be.
pub fn check_file(bytes: &[u8]) -> Result<(String, Format), FrameError> {
    if bytes.is_empty() {
        return Err(FrameError::EmptyFile);
    }
    if bytes.len() > MAX_STICKER_BYTES {
        return Err(FrameError::FileTooLarge(bytes.len().div_ceil(1024)));
    }
    let format = Format::sniff(bytes).ok_or(FrameError::UnknownFormat)?;
    Ok((sticker_id(bytes), format))
}

/// One sticker in a pack.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackItem {
    pub id: String,
    pub emoji: String,
    pub format: Format,
}

/// A pack: a title and its stickers, in order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pack {
    /// [`pack_id`] of the title and items.
    pub id: String,
    pub title: String,
    pub items: Vec<PackItem>,
    /// Made on this computer (rather than added from someone).
    #[serde(default)]
    pub mine: bool,
}

/// A pack's id: BLAKE3 over its title and its stickers' ids, in order. Two
/// people who hold the same pack compute the same id.
#[must_use]
pub fn pack_id(title: &str, items: &[PackItem]) -> String {
    let mut hasher = blake3::Hasher::new_derive_key("maya-chat 2026-10-13 sticker pack v1");
    hasher.update(&(title.len() as u64).to_le_bytes());
    hasher.update(title.as_bytes());
    for item in items {
        hasher.update(item.id.as_bytes());
    }
    hex::encode(hasher.finalize().as_bytes())
}

/// What a structured message says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Content {
    /// A sticker sent in a conversation; the file is the body.
    Sticker {
        id: String,
        emoji: String,
        pack: String,
        title: String,
    },
    /// "Send me the rest of this pack."
    PackRequest { pack: String },
    /// One sticker of a pack being shared; the file is the body.
    PackItem { pack: Pack, index: u16 },
    /// Sets the sender's `emoji` on the message `target` (an envelope id, hex)
    /// on or off. `at` is the sender's clock and orders that sender's own
    /// toggles ([`apply_mark`]).
    Reaction {
        target: String,
        emoji: String,
        on: bool,
        at: u64,
    },
    /// A reply to the message `target`; the reply's text is the body.
    /// `quote` is [`quote_hash`] of the text replied to, so a reply cannot
    /// misquote: the app shows its own copy only when the hash matches.
    Reply { target: String, quote: String },
}

/// Only the tag, read before the rest so an unknown kind is told apart from
/// a malformed known one.
#[derive(Deserialize)]
struct Kind {
    k: String,
}

/// A decoded structured message.
#[derive(Debug, PartialEq, Eq)]
pub struct Decoded {
    pub content: Content,
    /// The sticker file or the reply's text; empty for the other kinds.
    pub file: Vec<u8>,
}

/// Whether a plaintext is structured rather than text.
#[must_use]
pub fn is_structured(plain: &[u8]) -> bool {
    plain.starts_with(MAGIC)
}

fn is_hex_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Checks a message id: 64 lowercase hex characters.
///
/// # Errors
///
/// [`FrameError::BadId`].
pub fn check_id(id: &str) -> Result<(), FrameError> {
    is_hex_id(id).then_some(()).ok_or(FrameError::BadId)
}

/// Checks a reaction value: one of [`REACTION_EMOJI`] or a custom emoji's
/// 64-hex id.
///
/// # Errors
///
/// [`FrameError::BadEmoji`].
pub fn check_emoji(emoji: &str) -> Result<(), FrameError> {
    let ok =
        emoji.len() <= MAX_EMOJI_BYTES && (REACTION_EMOJI.contains(&emoji) || is_hex_id(emoji));
    ok.then_some(()).ok_or(FrameError::BadEmoji)
}

/// [`Content::Reply`]'s `quote` for the text replied to.
#[must_use]
pub fn quote_hash(text: &str) -> String {
    hex::encode(blake3::derive_key(QUOTE_DOMAIN, text.as_bytes()))
}

/// Checks `content` against `body`: what [`decode`] refuses, [`encode`]
/// refuses too.
fn check(content: &Content, body: &[u8]) -> Result<(), FrameError> {
    let claimed = match content {
        Content::Sticker { id, .. } => Some(id),
        Content::PackItem { pack, index } => {
            if pack.items.len() > MAX_PACK_ITEMS || pack.id != pack_id(&pack.title, &pack.items) {
                return Err(FrameError::PackMismatch);
            }
            Some(
                &pack
                    .items
                    .get(usize::from(*index))
                    .ok_or(FrameError::NotInPack)?
                    .id,
            )
        }
        Content::PackRequest { .. } => None,
        Content::Reaction { target, emoji, .. } => {
            check_id(target)?;
            check_emoji(emoji)?;
            None
        }
        Content::Reply { target, quote } => {
            check_id(target)?;
            check_id(quote)?;
            let text = std::str::from_utf8(body).map_err(|_| FrameError::BadText)?;
            if text.trim().is_empty() || text.contains('\0') {
                return Err(FrameError::BadText);
            }
            return Ok(());
        }
    };
    match claimed {
        Some(id) => {
            let (actual, _) = check_file(body)?;
            (actual == *id)
                .then_some(())
                .ok_or(FrameError::StickerMismatch)
        }
        None if body.is_empty() => Ok(()),
        None => Err(FrameError::Trailing),
    }
}

/// Frames `content` and `body` as a message plaintext, padded to a multiple
/// of [`PAD_BUCKET`].
///
/// # Errors
///
/// A header over [`MAX_HEADER`], a body over [`MAX_STICKER_BYTES`], or
/// anything [`decode`] would refuse.
pub fn encode(content: &Content, body: &[u8]) -> Result<Vec<u8>, FrameError> {
    let mut header =
        serde_json::to_vec(content).map_err(|e| FrameError::Malformed(e.to_string()))?;
    if header.len() > MAX_HEADER {
        return Err(FrameError::HeaderTooLong);
    }
    if body.len() > MAX_STICKER_BYTES {
        return Err(FrameError::BodyTooLarge);
    }
    check(content, body)?;
    let unpadded = MAGIC.len() + 4 + header.len() + body.len();
    header.resize(
        header.len() + unpadded.next_multiple_of(PAD_BUCKET) - unpadded,
        b' ',
    );
    let len = u32::try_from(header.len()).map_err(|_| FrameError::HeaderTooLong)?;
    Ok([MAGIC, &len.to_le_bytes(), &header, body].concat())
}

/// Decodes a structured plaintext and checks it, every sticker file against
/// its id. Bounded: the header is at most [`MAX_HEADER`] plus one bucket and
/// the body at most [`MAX_STICKER_BYTES`], whatever the length field claims.
///
/// # Errors
///
/// Truncated or oversized framing, a kind this build does not know, a header
/// that does not parse, or a body that does not match its header.
pub fn decode(plain: &[u8]) -> Result<Decoded, FrameError> {
    let rest = plain.strip_prefix(MAGIC).ok_or(FrameError::NotStructured)?;
    let (len, rest) = rest.split_first_chunk::<4>().ok_or(FrameError::Truncated)?;
    let len = usize::try_from(u32::from_le_bytes(*len)).map_err(|_| FrameError::Header)?;
    if len > MAX_HEADER + PAD_BUCKET || len > rest.len() {
        return Err(FrameError::Header);
    }
    let (header, body) = rest.split_at(len);
    if body.len() > MAX_STICKER_BYTES {
        return Err(FrameError::BodyTooLarge);
    }
    let malformed = |e: serde_json::Error| FrameError::Malformed(e.to_string());
    let kind: Kind = serde_json::from_slice(header).map_err(malformed)?;
    if !KNOWN_KINDS.contains(&kind.k.as_str()) {
        return Err(FrameError::UnknownKind);
    }
    let content: Content = serde_json::from_slice(header).map_err(malformed)?;
    check(&content, body)?;
    Ok(Decoded {
        content,
        file: body.to_vec(),
    })
}

/// One line of text for a plaintext, for the CLI: text as it is, a frame as
/// a short description, an unreadable frame as a plain notice.
#[must_use]
pub fn describe(plain: &[u8]) -> String {
    if !is_structured(plain) {
        return String::from_utf8_lossy(plain).into_owned();
    }
    let short = |id: &str| id.chars().take(8).collect::<String>();
    match decode(plain) {
        Ok(Decoded { content, file }) => match content {
            Content::Sticker { emoji, .. } => format!("[sticker {emoji}]"),
            Content::PackRequest { .. } => "[asks for a sticker pack]".into(),
            Content::PackItem { pack, .. } => {
                format!("[sticker from pack \u{201c}{}\u{201d}]", pack.title)
            }
            Content::Reaction {
                target,
                emoji,
                on: true,
                ..
            } => format!("[reacted {emoji} to {}]", short(&target)),
            Content::Reaction {
                target,
                emoji,
                on: false,
                ..
            } => {
                format!("[removed {emoji} from {}]", short(&target))
            }
            Content::Reply { target, .. } => {
                format!(
                    "[reply to {}] {}",
                    short(&target),
                    String::from_utf8_lossy(&file)
                )
            }
        },
        Err(FrameError::UnknownKind) => FrameError::UnknownKind.to_string(),
        Err(e) => format!("[unreadable message: {e}]"),
    }
}

/// One reactor's reaction state for one emoji on one message.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    /// Set by this user, rather than the peer.
    pub mine: bool,
    pub emoji: String,
    pub on: bool,
    /// The reactor's clock when they toggled it.
    pub at: u64,
}

/// What the app shows above a reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "text", rename_all = "lowercase")]
pub enum Quote {
    /// The local copy of the message replied to; its hash matched.
    Shown(String),
    /// This device does not hold the message replied to.
    Missing,
    /// The local copy does not hash to the quote: show no quote at all.
    Mismatch,
}

/// The quote to show for a reply whose target's local text is `target_text`
/// (`None` when this device does not hold it).
#[must_use]
pub fn resolve_quote(target_text: Option<&str>, quote: &str) -> Quote {
    match target_text {
        None => Quote::Missing,
        Some(text) if quote_hash(text) == quote => Quote::Shown(text.to_owned()),
        Some(_) => Quote::Mismatch,
    }
}

/// `marks` with `mark` applied, as a new list. A reaction is a toggle per
/// (reactor, message, emoji): the mark with the same reactor and emoji is
/// replaced when `mark` is later. On the same `at`, off wins, so toggles that
/// arrive in any order end in one state. A new (reactor, emoji) past
/// [`MAX_MARKS`] is dropped.
#[must_use]
pub fn apply_mark(marks: &[Mark], mark: Mark) -> Vec<Mark> {
    let rank = |m: &Mark| (m.at, !m.on);
    match marks
        .iter()
        .position(|m| m.mine == mark.mine && m.emoji == mark.emoji)
    {
        Some(i) if rank(&mark) > rank(&marks[i]) => {
            let mut next = marks.to_vec();
            next[i] = mark;
            next
        }
        Some(_) => marks.to_vec(),
        None if marks.len() >= MAX_MARKS => marks.to_vec(),
        None => [marks, &[mark]].concat(),
    }
}
