//! Custom emoji packs (ADR-140 section 2, build PR 3).
//!
//! A custom emoji pack is a [`Pack`] of [`PackKind::Emoji`]: at most
//! [`MAX_EMOJI_PACK_ITEMS`] small files, each named by its BLAKE3 hash (the
//! 64-hex id reactions already accept) and given a shortcode. Text carries a
//! custom emoji as `:<id>:`, never as the shortcode, so two people with
//! different packs never read one name as two pictures. A receiver that does
//! not hold the file draws [`FALLBACK_GLYPH`].
//!
//! The pack's manifest (title, items, shortcodes, formats) is signed with the
//! hybrid suite (ML-DSA-65 and SLH-DSA-SHA2-128s) under a key derived from the
//! author's chat identity seed. The pack id commits to the signer, so a pack
//! re-signed by someone else is a different pack. The signer is a separate
//! key from the chat identity, so a pack does not name its author's chat
//! address; it does link every pack one identity made (threat model T7).
//!
//! Pack files reach a peer only inside the end-to-end encryption, through
//! the same pack-sharing path stickers use. Nothing here fetches anything.

use std::collections::BTreeSet;

use maya_crypto_pq::suite::{HybridMlDsa65SlhDsa128s, MasterSeed, SignatureSuite, SuiteId};
use serde::Serialize;
use zeroize::Zeroizing;

use crate::frame::{Format, FrameError, MAX_TITLE_CHARS, Pack, PackItem, PackKind, check_id};

/// Emoji in one pack. Bounded by the frame header: a full pack with the
/// longest title, shortcodes and format names still encodes under
/// `MAX_HEADER` (`a_full_emoji_pack_fits_one_frame_header`).
pub const MAX_EMOJI_PACK_ITEMS: usize = 24;
/// Characters in a shortcode, which is `a-z`, `0-9` and `_`.
pub const MIN_CODE_CHARS: usize = 2;
pub const MAX_CODE_CHARS: usize = 32;
/// What the apps draw for a custom emoji they do not hold.
pub const FALLBACK_GLYPH: &str = "\u{25c7}";

const AUTHOR_DOMAIN: &str = "maya-chat 2026-11-13 emoji pack author key v1";
const SIGNER_DOMAIN: &str = "maya-chat 2026-11-13 emoji pack signer v1";
const PACK_DOMAIN: &str = "maya-chat 2026-11-13 emoji pack v1";
const MANIFEST_DOMAIN: &[u8] = b"maya-chat 2026-11-13 emoji pack manifest v1";
const SUITE: SuiteId = SuiteId::HybridMlDsa65SlhDsa128s;
/// Hybrid key generation and signing need more stack than a debug build's
/// main thread on Windows gives (`apps/faucet` overflowed there).
const SIGN_STACK: usize = 16 << 20;

/// Checks a shortcode: [`MIN_CODE_CHARS`] to [`MAX_CODE_CHARS`] of `a-z`,
/// `0-9` and `_`.
///
/// # Errors
///
/// [`FrameError::BadCode`].
pub fn check_code(code: &str) -> Result<(), FrameError> {
    let ok = (MIN_CODE_CHARS..=MAX_CODE_CHARS).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    ok.then_some(()).ok_or(FrameError::BadCode)
}

const fn format_tag(format: Format) -> &'static str {
    match format {
        Format::Lottie => "lottie",
        Format::Tgs => "tgs",
        Format::Webm => "webm",
        Format::Webp => "webp",
        Format::Png => "png",
        Format::Gif => "gif",
        Format::Svg => "svg",
    }
}

fn put(hasher: &mut blake3::Hasher, field: &[u8]) {
    hasher.update(&(field.len() as u64).to_le_bytes());
    hasher.update(field);
}

/// An emoji pack's id: BLAKE3 over its title, signer and every item's id,
/// shortcode and format, each length-prefixed, so no two manifests share one.
#[must_use]
pub fn emoji_pack_id(title: &str, signer: &str, items: &[PackItem]) -> String {
    let mut hasher = blake3::Hasher::new_derive_key(PACK_DOMAIN);
    put(&mut hasher, title.as_bytes());
    put(&mut hasher, signer.as_bytes());
    for item in items {
        put(&mut hasher, item.id.as_bytes());
        put(&mut hasher, item.emoji.as_bytes());
        put(&mut hasher, format_tag(item.format).as_bytes());
    }
    hex::encode(hasher.finalize().as_bytes())
}

/// The signer named in a pack: hex BLAKE3 of the hybrid public key.
#[must_use]
pub fn signer_of(public_key: &[u8]) -> String {
    hex::encode(blake3::derive_key(SIGNER_DOMAIN, public_key))
}

fn manifest_message(pack_id: &str) -> Vec<u8> {
    [MANIFEST_DOMAIN, pack_id.as_bytes()].concat()
}

fn proof_lens() -> (usize, usize) {
    let info = SUITE.info();
    (info.public_key_len, info.signature_len)
}

/// Checks an emoji pack's shape: its kind, 1 to [`MAX_EMOJI_PACK_ITEMS`]
/// items with distinct, valid shortcodes, a well-formed signer and title.
///
/// # Errors
///
/// [`FrameError::EmojiPackSize`], [`FrameError::BadCode`] or
/// [`FrameError::PackMismatch`].
pub fn check_pack(pack: &Pack) -> Result<(), FrameError> {
    if pack.kind != PackKind::Emoji || check_id(&pack.signer).is_err() {
        return Err(FrameError::PackMismatch);
    }
    if pack.title.trim().is_empty() || pack.title.chars().count() > MAX_TITLE_CHARS {
        return Err(FrameError::PackMismatch);
    }
    if pack.items.is_empty() || pack.items.len() > MAX_EMOJI_PACK_ITEMS {
        return Err(FrameError::EmojiPackSize);
    }
    let mut codes = BTreeSet::new();
    for item in &pack.items {
        check_code(&item.emoji)?;
        check_id(&item.id)?;
        if !codes.insert(item.emoji.as_str()) {
            return Err(FrameError::EmojiPackSize);
        }
    }
    Ok(())
}

pub(crate) fn on_big_stack<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
    err: FrameError,
) -> Result<T, FrameError> {
    std::thread::Builder::new()
        .stack_size(SIGN_STACK)
        .spawn(f)
        .map_err(|_| err.clone())?
        .join()
        .map_err(|_| err)
}

/// Makes and signs an emoji pack from items whose files were already checked
/// ([`crate::media::check_emoji`]). The signing key is derived from
/// `identity_seed` and wiped on drop. Returns the pack and its proof, the
/// body of the [`crate::frame::Content::EmojiPack`] that introduces it.
///
/// # Errors
///
/// What [`check_pack`] refuses, or [`FrameError::Signing`].
pub fn make_emoji_pack(
    identity_seed: &[u8; 32],
    title: &str,
    items: Vec<PackItem>,
) -> Result<(Pack, Vec<u8>), FrameError> {
    let seed = Zeroizing::new(blake3::derive_key(AUTHOR_DOMAIN, identity_seed));
    let title = title.trim().to_owned();
    on_big_stack(
        move || {
            let key =
                HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes(*seed));
            let public = HybridMlDsa65SlhDsa128s::public_key(&key);
            let signer = signer_of(&public);
            let pack = Pack {
                id: emoji_pack_id(&title, &signer, &items),
                title,
                items,
                mine: true,
                kind: PackKind::Emoji,
                signer,
            };
            check_pack(&pack)?;
            let signature = HybridMlDsa65SlhDsa128s::sign(&key, &manifest_message(&pack.id))
                .map_err(|_| FrameError::Signing)?;
            Ok::<_, FrameError>((pack, [public, signature].concat()))
        },
        FrameError::Signing,
    )?
}

/// Verifies an emoji pack's manifest against its proof (`public key ‖
/// signature`): its shape, its id, that the key is the pack's signer, and
/// both halves of the hybrid signature.
///
/// # Errors
///
/// [`FrameError::BadProof`], or what [`check_pack`] refuses, or
/// [`FrameError::PackMismatch`] for an id that does not match.
pub fn verify_manifest(pack: &Pack, proof: &[u8]) -> Result<(), FrameError> {
    check_pack(pack)?;
    if pack.id != emoji_pack_id(&pack.title, &pack.signer, &pack.items) {
        return Err(FrameError::PackMismatch);
    }
    let (pk_len, sig_len) = proof_lens();
    if proof.len() != pk_len + sig_len {
        return Err(FrameError::BadProof);
    }
    let (public, signature) = proof.split_at(pk_len);
    if signer_of(public) != pack.signer {
        return Err(FrameError::BadProof);
    }
    let (public, signature, message) = (
        public.to_vec(),
        signature.to_vec(),
        manifest_message(&pack.id),
    );
    on_big_stack(
        move || HybridMlDsa65SlhDsa128s::verify(&public, &message, &signature).is_ok(),
        FrameError::BadProof,
    )?
    .then_some(())
    .ok_or(FrameError::BadProof)
}

/// How to draw one emoji value: a reaction, or a `:<id>:` in text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "glyph", rename_all = "lowercase")]
pub enum Glyph {
    /// An ordinary emoji (or any text that is not a custom id), as it is.
    Plain { text: String },
    /// A custom emoji from an installed pack: draw the file `id`.
    Custom {
        id: String,
        code: String,
        format: Format,
    },
    /// A custom id this device holds no pack for: draw [`FALLBACK_GLYPH`].
    Fallback { id: String },
}

/// The installed emoji pack item with this id or shortcode, and its pack.
fn find<'a>(
    packs: &'a [Pack],
    hit: impl Fn(&PackItem) -> bool,
) -> Option<(&'a Pack, &'a PackItem)> {
    packs
        .iter()
        .filter(|p| p.kind == PackKind::Emoji)
        .find_map(|p| p.items.iter().find(|i| hit(i)).map(|i| (p, i)))
}

/// The id of the installed emoji pack that holds the custom emoji `id`.
#[must_use]
pub fn pack_of(id: &str, packs: &[Pack]) -> Option<String> {
    find(packs, |i| i.id == id).map(|(p, _)| p.id.clone())
}

/// Resolves a reaction value or an inline id against the installed packs.
#[must_use]
pub fn resolve(value: &str, packs: &[Pack]) -> Glyph {
    if check_id(value).is_err() {
        return Glyph::Plain {
            text: value.to_owned(),
        };
    }
    match find(packs, |i| i.id == value) {
        Some((_, item)) => Glyph::Custom {
            id: item.id.clone(),
            code: item.emoji.clone(),
            format: item.format,
        },
        None => Glyph::Fallback {
            id: value.to_owned(),
        },
    }
}

/// A piece of a line of text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Segment {
    Text { text: String },
    Emoji { glyph: Glyph },
}

fn push_text(out: &mut Vec<Segment>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(Segment::Text { text: last }) = out.last_mut() {
        last.push_str(text);
    } else {
        out.push(Segment::Text {
            text: text.to_owned(),
        });
    }
}

/// The `<id>` of a `:<id>:` that starts right after a `:`, if `after` holds
/// one.
fn inline_id(after: &str) -> Option<&str> {
    let id = after.get(..64)?;
    (after.as_bytes().get(64) == Some(&b':') && check_id(id).is_ok()).then_some(id)
}

/// Splits text into plain runs and `:<id>:` custom emoji, resolved against
/// the installed packs. Anything else stays text, colons included.
#[must_use]
pub fn segments(text: &str, packs: &[Pack]) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(':') {
        let after = &rest[i + 1..];
        if let Some(id) = inline_id(after) {
            push_text(&mut out, &rest[..i]);
            out.push(Segment::Emoji {
                glyph: resolve(id, packs),
            });
            rest = &after[65..];
        } else {
            push_text(&mut out, &rest[..=i]);
            rest = after;
        }
    }
    push_text(&mut out, rest);
    out
}

/// What the sender's text becomes: every `:shortcode:` of an installed emoji
/// pack replaced by `:<id>:`, and the ids of the packs the text then uses
/// (for sharing a pack with a peer on first use).
#[must_use]
pub fn expand_codes(text: &str, packs: &[Pack]) -> (String, BTreeSet<String>) {
    let mut out = String::with_capacity(text.len());
    let mut used = BTreeSet::new();
    let mut rest = text;
    while let Some(i) = rest.find(':') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(id) = inline_id(after) {
            if let Some(pack) = pack_of(id, packs) {
                used.insert(pack);
            }
            out.push(':');
            out.push_str(id);
            out.push(':');
            rest = &after[65..];
            continue;
        }
        let code = after.find(':').map(|end| &after[..end]);
        match code
            .filter(|c| check_code(c).is_ok())
            .and_then(|c| find(packs, |i| i.emoji == c).map(|f| (c, f)))
        {
            Some((c, (pack, item))) => {
                used.insert(pack.id.clone());
                out.push(':');
                out.push_str(&item.id);
                out.push(':');
                rest = &after[c.len() + 1..];
            }
            None => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    (out, used)
}
