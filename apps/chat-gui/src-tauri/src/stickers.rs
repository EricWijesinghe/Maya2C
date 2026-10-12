//! Stickers and sticker packs.
//!
//! A sticker is a file named by its own BLAKE3 hash. It travels inside the
//! end-to-end encryption like any message, framed so a receiver can tell it
//! from text:
//!
//! ```text
//! MAGIC ‖ header_len:u32 LE ‖ header (JSON, ≤ MAX_HEADER) ‖ file bytes
//! ```
//!
//! The receiver re-hashes the file and refuses it unless the hash is the id
//! the header names. So a sticker can never be swapped in transit or in a
//! cache. The file type is read from its own bytes, never from the header or
//! a file name. One message carries one file, at most [`MAX_STICKER_BYTES`],
//! which fits the relay's 256 KiB envelope with room for the encryption.
//!
//! Packs travel the same way. Anyone who received one sticker can ask the
//! sender for the whole pack ([`Content::PackRequest`]). The sender's app
//! answers with one [`Content::PackItem`] per sticker, but only for a pack the
//! sender owns or installed, and the asker's app checks every item against
//! the pack's id.

use serde::{Deserialize, Serialize};

/// Starts every structured message. Plain text cannot start with NUL: the
/// window refuses to send one.
const MAGIC: &[u8] = b"\0MAYA-CHAT-1";
/// A sticker file, in bytes: what fits one relay envelope (256 KiB) in the
/// worst case, a first message carrying the session handshake. The envelope's
/// CBOR writes ciphertext as an integer array, about two bytes per byte, so a
/// 120 KiB file seals to 247,427 bytes (measured by
/// `the_largest_sticker_crosses_real_encryption_within_one_envelope`).
/// Telegram's animated stickers are at most 64 KB.
pub const MAX_STICKER_BYTES: usize = 120 << 10;
/// The JSON header, in bytes. Titles and emoji are short.
const MAX_HEADER: usize = 4 << 10;
/// Stickers in one pack (Telegram allows 120).
pub const MAX_PACK_ITEMS: usize = 120;
/// Characters in a pack title.
pub const MAX_TITLE_CHARS: usize = 64;

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
        } else if trimmed.starts_with("<svg") || (trimmed.starts_with("<?xml") && text.contains("<svg")) {
            Some(Self::Svg)
        } else if trimmed.starts_with('{') && serde_json::from_slice::<serde_json::Value>(bytes).is_ok_and(|v| v.get("layers").is_some()) {
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
pub fn check_file(bytes: &[u8]) -> Result<(String, Format), String> {
    if bytes.is_empty() {
        return Err("the sticker file is empty".into());
    }
    if bytes.len() > MAX_STICKER_BYTES {
        return Err(format!(
            "a sticker can be at most {} KB; this one is {} KB",
            MAX_STICKER_BYTES >> 10,
            bytes.len().div_ceil(1024)
        ));
    }
    let format = Format::sniff(bytes).ok_or(
        "not a sticker format: use Lottie (.json or .tgs), WebM, WebP, PNG, GIF or SVG",
    )?;
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

/// Builds a pack, refusing an empty, oversized or untitled one.
///
/// # Errors
///
/// No title, a title over [`MAX_TITLE_CHARS`], no stickers or more than
/// [`MAX_PACK_ITEMS`], or a sticker without an emoji.
pub fn make_pack(title: &str, items: Vec<PackItem>, mine: bool) -> Result<Pack, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("give the pack a name".into());
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(format!("a pack name is at most {MAX_TITLE_CHARS} characters"));
    }
    if items.is_empty() || items.len() > MAX_PACK_ITEMS {
        return Err(format!("a pack holds 1 to {MAX_PACK_ITEMS} stickers"));
    }
    if items.iter().any(|i| i.emoji.trim().is_empty()) {
        return Err("give every sticker an emoji, so it can be suggested".into());
    }
    Ok(Pack {
        id: pack_id(title, &items),
        title: title.to_owned(),
        items,
        mine,
    })
}

/// What a structured message says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Content {
    /// A sticker sent in a conversation; the file follows the header.
    Sticker {
        id: String,
        emoji: String,
        pack: String,
        title: String,
    },
    /// "Send me the rest of this pack."
    PackRequest { pack: String },
    /// One sticker of a pack being shared; the file follows the header.
    PackItem {
        pack: Pack,
        index: u16,
    },
}

/// Frames `content` and `file` as a message plaintext.
///
/// # Errors
///
/// A header over [`MAX_HEADER`] or a file over [`MAX_STICKER_BYTES`].
pub fn encode(content: &Content, file: &[u8]) -> Result<Vec<u8>, String> {
    let header = serde_json::to_vec(content).map_err(|e| e.to_string())?;
    if header.len() > MAX_HEADER {
        return Err("the sticker's details are too long".into());
    }
    if file.len() > MAX_STICKER_BYTES {
        return Err("the sticker file is too large".into());
    }
    let len = u32::try_from(header.len()).map_err(|_| "header too long".to_string())?;
    Ok([MAGIC, &len.to_le_bytes(), &header, file].concat())
}

/// A decoded structured message.
#[derive(Debug, PartialEq, Eq)]
pub struct Decoded {
    pub content: Content,
    pub file: Vec<u8>,
}

/// Whether a plaintext is structured rather than text.
#[must_use]
pub fn is_structured(plain: &[u8]) -> bool {
    plain.starts_with(MAGIC)
}

/// Decodes a structured plaintext and checks every file against its id.
///
/// # Errors
///
/// Truncated or oversized framing, a header that does not parse, a file that
/// does not hash to the id it claims, or a pack item that is not in its pack.
pub fn decode(plain: &[u8]) -> Result<Decoded, String> {
    let rest = plain.strip_prefix(MAGIC).ok_or("not a structured message")?;
    let (len, rest) = rest.split_first_chunk::<4>().ok_or("truncated message")?;
    let len = u32::from_le_bytes(*len) as usize;
    if len > MAX_HEADER || len > rest.len() {
        return Err("malformed message header".into());
    }
    let (header, file) = rest.split_at(len);
    let content: Content = serde_json::from_slice(header).map_err(|e| format!("malformed message: {e}"))?;
    let claimed = match &content {
        Content::Sticker { id, .. } => Some(id.clone()),
        Content::PackItem { pack, index } => {
            if pack.id != pack_id(&pack.title, &pack.items) || pack.items.len() > MAX_PACK_ITEMS {
                return Err("the pack does not match its id".into());
            }
            let item = pack.items.get(usize::from(*index)).ok_or("the pack item is not in its pack")?;
            Some(item.id.clone())
        }
        Content::PackRequest { .. } => None,
    };
    if let Some(id) = claimed {
        let (actual, _) = check_file(file)?;
        if actual != id {
            return Err("the sticker does not match its id".into());
        }
    } else if !file.is_empty() {
        return Err("unexpected bytes after the message".into());
    }
    Ok(Decoded {
        content,
        file: file.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";

    fn item(bytes: &[u8], emoji: &str) -> PackItem {
        let (id, format) = check_file(bytes).unwrap();
        PackItem { id, emoji: emoji.into(), format }
    }

    #[test]
    fn formats_are_read_from_the_bytes() {
        assert_eq!(Format::sniff(GIF), Some(Format::Gif));
        assert_eq!(Format::sniff(b"\x89PNG\r\n\x1a\nrest"), Some(Format::Png));
        assert_eq!(Format::sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(Format::Webp));
        assert_eq!(Format::sniff(&[0x1a, 0x45, 0xdf, 0xa3, 1]), Some(Format::Webm));
        assert_eq!(Format::sniff(&[0x1f, 0x8b, 8, 0]), Some(Format::Tgs));
        assert_eq!(Format::sniff(br#"{"v":"5.7","layers":[]}"#), Some(Format::Lottie));
        assert_eq!(Format::sniff(br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#), Some(Format::Svg));
        assert_eq!(Format::sniff(br#"{"not":"lottie"}"#), None, "JSON without layers");
        assert_eq!(Format::sniff(b"MZ\x90\x00 an exe"), None);
    }

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
        let mut other = GIF.to_vec();
        *other.last_mut().unwrap() = b'!';
        let plain = encode(&content, &other).unwrap();
        assert_eq!(decode(&plain).unwrap_err(), "the sticker does not match its id");
    }

    #[test]
    fn oversized_or_unknown_files_are_refused() {
        let big = [b"GIF89a".as_slice(), &vec![0; MAX_STICKER_BYTES]].concat();
        assert!(check_file(&big).unwrap_err().contains("at most 120 KB"));
        assert!(check_file(b"#!/bin/sh\nrm -rf /").is_err());
        assert!(check_file(b"").is_err());
    }

    #[test]
    fn truncated_or_lying_frames_are_refused() {
        assert!(decode(b"hello").is_err());
        assert!(decode(MAGIC).is_err());
        let huge_len = [MAGIC, &u32::MAX.to_le_bytes()].concat();
        assert!(decode(&huge_len).is_err());
        let request = encode(&Content::PackRequest { pack: "p".into() }, &[]).unwrap();
        assert!(decode(&request).is_ok());
        let with_tail = [request.as_slice(), b"extra"].concat();
        assert!(decode(&with_tail).is_err(), "a request carries no file");
    }

    #[test]
    fn pack_ids_are_stable_and_pack_items_must_belong() {
        let items = vec![item(GIF, "😂")];
        let pack = make_pack(" Mine ", items.clone(), true).unwrap();
        assert_eq!(pack.title, "Mine");
        assert_eq!(pack.id, pack_id("Mine", &items), "same pack, same id anywhere");
        assert_ne!(pack.id, pack_id("Yours", &items));

        let ok = encode(&Content::PackItem { pack: pack.clone(), index: 0 }, GIF).unwrap();
        assert!(decode(&ok).is_ok());
        let past_end = encode(&Content::PackItem { pack: pack.clone(), index: 1 }, GIF).unwrap();
        assert!(decode(&past_end).is_err());
        let mut forged = pack;
        forged.title = "Renamed".into();
        let forged = encode(&Content::PackItem { pack: forged, index: 0 }, GIF).unwrap();
        assert_eq!(decode(&forged).unwrap_err(), "the pack does not match its id");
    }

    #[test]
    fn packs_need_a_name_stickers_and_emoji() {
        assert!(make_pack("", vec![item(GIF, "😂")], true).is_err());
        assert!(make_pack("x", vec![], true).is_err());
        assert!(make_pack("x", vec![item(GIF, " ")], true).is_err());
        assert!(make_pack(&"x".repeat(MAX_TITLE_CHARS + 1), vec![item(GIF, "😂")], true).is_err());
    }

    #[test]
    fn the_largest_sticker_crosses_real_encryption_within_one_envelope() {
        use maya_chat::client::Client;
        use maya_chat::identity::Identity;
        const NOW: u64 = 1_760_000_000;
        // The worst case: a full-size file (padded GIF) in a first message,
        // which also carries the session handshake.
        let file = [GIF, &vec![0u8; MAX_STICKER_BYTES - GIF.len()]].concat();
        assert_eq!(file.len(), MAX_STICKER_BYTES);
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
        let opened = b.open(&envelope, NOW).unwrap();
        let decoded = decode(&opened.text).unwrap();
        assert_eq!(decoded.content, content);
        assert_eq!(decoded.file, file);
    }
}

