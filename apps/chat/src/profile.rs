//! Profile cards (ADR-140 section 2, build PR 4).
//!
//! A card is a display name, a bio, an optional avatar and animated frame,
//! and an optional theme id. It travels only as a [`crate::frame::Content::Profile`]
//! inside the end-to-end session with one contact, chosen per contact
//! ([`Sharing`]): there is no directory, no server-side profile and nothing
//! public, so two contacts can hold different cards and a contact shown
//! nothing holds nothing.
//!
//! The card is signed with the hybrid suite (ML-DSA-65 and SLH-DSA-SHA2-128s)
//! under a key derived from the identity seed in its own domain, as custom
//! emoji manifests are ([`crate::emoji`]). The signature covers the version,
//! every field and the files' hashes, so a relay or a later session cannot
//! swap a field or a file, and a contact's app pins the first signer it saw
//! ([`accept`]). It is a transferable statement: a contact can show others
//! the card it was sent. The pairwise identities of ADR-140 section 4 sign
//! with their own seeds, so their cards do not link to the main one.
//!
//! Each send carries a higher [`Card::version`]; a receiver keeps the highest
//! and ignores an equal, older or replayed one, whatever order they arrive in.
//!
//! The frame body is `public key ‖ signature ‖ avatar ‖ frame file`. Both
//! files go through [`crate::frame::check_file`] and so the sticker caps of
//! [`crate::media`]; the whole body is held to the one-envelope ceiling.
//! Nothing here fetches anything: every file arrives inside the frame.

use std::collections::BTreeMap;

use maya_crypto_pq::suite::{HybridMlDsa65SlhDsa128s, MasterSeed, SignatureSuite, SuiteId};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::frame::{Format, FrameError, MAX_STICKER_BYTES, check_file, check_id, sticker_id};

/// Characters in a display name. Chosen bound: room for a full name and an
/// emoji, short enough for one line of the contact list.
pub const MAX_NAME_CHARS: usize = 48;
/// Characters in a bio. Chosen bound: a short paragraph.
pub const MAX_BIO_CHARS: usize = 280;

const KEY_DOMAIN: &str = "maya-chat 2026-11-13 profile card key v1";
const SIGNER_DOMAIN: &str = "maya-chat 2026-11-13 profile card signer v1";
const CARD_DOMAIN: &str = "maya-chat 2026-11-13 profile card v1";
const SUITE: SuiteId = SuiteId::HybridMlDsa65SlhDsa128s;

/// A file carried in a card's body: its sticker id and length.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub len: u32,
}

/// A profile card as it travels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    /// Higher replaces lower ([`accept`]).
    pub version: u64,
    pub name: String,
    #[serde(default)]
    pub bio: String,
    /// A still or animated image, drawn as an `<img>`.
    #[serde(default)]
    pub avatar: Option<Attachment>,
    /// A Lottie animation drawn around the avatar.
    #[serde(default)]
    pub frame: Option<Attachment>,
    /// A built-in theme id ([`crate::theme::BUILT_IN`]) offered as the
    /// sender's accent; the receiver may ignore it. Empty for none.
    #[serde(default)]
    pub theme: String,
    /// Hex BLAKE3 of the card's public key ([`signer_of`]).
    pub signer: String,
}

/// A card as the user keeps it, before it is signed: files by their cached
/// sticker id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Draft {
    pub name: String,
    pub bio: String,
    pub avatar: Option<String>,
    pub frame: Option<String>,
    pub theme: String,
}

/// What one contact is shown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Show {
    /// The default card, if there is one.
    #[default]
    Default,
    /// The named card; nothing if it no longer exists.
    Card(String),
    /// No card at all.
    Nothing,
}

/// The user's cards and who sees which.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sharing {
    /// Cards by a name only the user sees ("main", "work").
    pub cards: BTreeMap<String, Draft>,
    /// The card a contact with no choice of their own is shown; `None`
    /// shares nothing by default.
    pub default: Option<String>,
    /// Per-contact choices, by peer address.
    pub per_contact: BTreeMap<String, Show>,
    /// The last version sent ([`Sharing::next_version`]).
    pub version: u64,
}

impl Sharing {
    /// The card `peer` is shown, if any.
    #[must_use]
    pub fn card_for(&self, peer: &str) -> Option<&Draft> {
        match self.per_contact.get(peer).unwrap_or(&Show::Default) {
            Show::Nothing => None,
            Show::Card(name) => self.cards.get(name),
            Show::Default => self.default.as_ref().and_then(|name| self.cards.get(name)),
        }
    }

    /// A version higher than any sent before. At least the current Unix
    /// time, so a reinstalled app with the same identity still outranks the
    /// cards it sent before.
    pub fn next_version(&mut self) -> u64 {
        self.version = self.version.saturating_add(1).max(crate::now());
        self.version
    }
}

/// Whether `incoming` replaces `held`: the first card, or a strictly higher
/// version from the same signer.
#[must_use]
pub fn accept(held: Option<&Card>, incoming: &Card) -> bool {
    held.is_none_or(|h| incoming.version > h.version && incoming.signer == h.signer)
}

/// Characters a name or bio never keeps: they draw nothing, or reorder what
/// follows them (U+202E would show `Ann\u{202e}gpj.exe` as `Annexe.jpg`).
fn is_hidden(c: char) -> bool {
    matches!(c,
        '\u{00ad}' | '\u{061c}' | '\u{180e}'
        | '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2060}'..='\u{2069}'
        | '\u{feff}'
    ) || (c.is_control() && c != '\n')
}

/// A display name with hidden and direction characters removed, every
/// whitespace run one space, trimmed. Refused when nothing is left or it is
/// longer than [`MAX_NAME_CHARS`]: cut, a name could end mid-word in a way
/// the sender never saw.
///
/// # Errors
///
/// [`FrameError::BadName`].
pub fn sanitize_name(name: &str) -> Result<String, FrameError> {
    let kept: String = name
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !is_hidden(*c))
        .collect();
    let clean = kept
        .split(' ')
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let n = clean.chars().count();
    (1..=MAX_NAME_CHARS)
        .contains(&n)
        .then_some(clean)
        .ok_or(FrameError::BadName)
}

/// A bio with hidden and direction characters removed, line breaks kept (at
/// most one blank line in a row), trimmed. May be empty.
///
/// # Errors
///
/// [`FrameError::BadBio`] when longer than [`MAX_BIO_CHARS`].
pub fn sanitize_bio(bio: &str) -> Result<String, FrameError> {
    let kept: String = bio.chars().filter(|c| !is_hidden(*c)).collect();
    let mut clean = String::with_capacity(kept.len());
    let mut breaks = 0;
    for c in kept.trim().chars() {
        breaks = if c == '\n' { breaks + 1 } else { 0 };
        if breaks <= 2 {
            clean.push(c);
        }
    }
    (clean.chars().count() <= MAX_BIO_CHARS)
        .then_some(clean)
        .ok_or(FrameError::BadBio)
}

/// Checks an avatar: a PNG, WebP, GIF or SVG within the sticker caps. A
/// moving avatar is a frame's job, so Lottie and WebM are refused.
///
/// # Errors
///
/// What [`check_file`] refuses, or [`FrameError::BadCard`].
pub fn check_avatar(bytes: &[u8]) -> Result<(), FrameError> {
    let (_, format) = check_file(bytes)?;
    matches!(
        format,
        Format::Png | Format::Webp | Format::Gif | Format::Svg
    )
    .then_some(())
    .ok_or(FrameError::BadCard)
}

/// Checks an animated frame: Lottie or TGS within the sticker animation caps
/// (64 KB, 3 s, 60 fps, 512 px), which the window plays only without the OS
/// reduced-motion setting.
///
/// # Errors
///
/// What [`check_file`] refuses, or [`FrameError::BadCard`].
pub fn check_frame(bytes: &[u8]) -> Result<(), FrameError> {
    let (_, format) = check_file(bytes)?;
    matches!(format, Format::Lottie | Format::Tgs)
        .then_some(())
        .ok_or(FrameError::BadCard)
}

/// The signer named in a card: hex BLAKE3 of its public key.
#[must_use]
pub fn signer_of(public_key: &[u8]) -> String {
    hex::encode(blake3::derive_key(SIGNER_DOMAIN, public_key))
}

fn put(hasher: &mut blake3::Hasher, field: &[u8]) {
    hasher.update(&(field.len() as u64).to_le_bytes());
    hasher.update(field);
}

fn put_file(hasher: &mut blake3::Hasher, file: Option<&Attachment>) {
    let (id, len) = file.map_or(("", 0), |a| (a.id.as_str(), a.len));
    put(hasher, id.as_bytes());
    hasher.update(&len.to_le_bytes());
}

/// What the signature covers: every field, each length-prefixed.
fn card_message(card: &Card) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new_derive_key(CARD_DOMAIN);
    hasher.update(&card.version.to_le_bytes());
    put(&mut hasher, card.name.as_bytes());
    put(&mut hasher, card.bio.as_bytes());
    put_file(&mut hasher, card.avatar.as_ref());
    put_file(&mut hasher, card.frame.as_ref());
    put(&mut hasher, card.theme.as_bytes());
    put(&mut hasher, card.signer.as_bytes());
    hasher.finalize().as_bytes().to_vec()
}

fn proof_len() -> usize {
    let info = SUITE.info();
    info.public_key_len + info.signature_len
}

fn file_len(file: Option<&Attachment>) -> usize {
    file.map_or(0, |a| usize::try_from(a.len).unwrap_or(usize::MAX))
}

/// Checks a card's fields: a version, a sanitized name and bio, a known
/// theme, a well-formed signer and file ids.
fn check_card(card: &Card) -> Result<(), FrameError> {
    if sanitize_name(&card.name)? != card.name {
        return Err(FrameError::BadName);
    }
    if sanitize_bio(&card.bio)? != card.bio {
        return Err(FrameError::BadBio);
    }
    let files_ok = [&card.avatar, &card.frame]
        .into_iter()
        .flatten()
        .all(|a| check_id(&a.id).is_ok() && a.len > 0);
    let ok = card.version > 0
        && crate::theme::is_theme(&card.theme)
        && check_id(&card.signer).is_ok()
        && files_ok;
    ok.then_some(()).ok_or(FrameError::BadCard)
}

fn attach(bytes: Option<&[u8]>) -> Result<Option<Attachment>, FrameError> {
    bytes
        .map(|b| {
            let len = u32::try_from(b.len()).map_err(|_| FrameError::BodyTooLarge)?;
            Ok(Attachment {
                id: sticker_id(b),
                len,
            })
        })
        .transpose()
}

/// Makes and signs a card from `draft` with the given files, which are
/// checked here ([`check_avatar`], [`check_frame`]). The signing key is
/// derived from `identity_seed` and wiped on drop. Returns the card and its
/// frame body.
///
/// # Errors
///
/// A name, bio, theme or version [`check_card`] refuses, a file the caps
/// refuse, [`FrameError::BodyTooLarge`] when it all would not fit one
/// envelope, or [`FrameError::Signing`].
pub fn make_card(
    identity_seed: &[u8; 32],
    draft: &Draft,
    version: u64,
    avatar: Option<&[u8]>,
    frame: Option<&[u8]>,
) -> Result<(Card, Vec<u8>), FrameError> {
    let name = sanitize_name(&draft.name)?;
    let bio = sanitize_bio(&draft.bio)?;
    if version == 0 || !crate::theme::is_theme(&draft.theme) {
        return Err(FrameError::BadCard);
    }
    avatar.map(check_avatar).transpose()?;
    frame.map(check_frame).transpose()?;
    let files = [avatar.unwrap_or_default(), frame.unwrap_or_default()].concat();
    if proof_len() + files.len() > MAX_STICKER_BYTES {
        return Err(FrameError::BodyTooLarge);
    }
    let (avatar, frame) = (attach(avatar)?, attach(frame)?);
    let theme = draft.theme.clone();
    let seed = Zeroizing::new(blake3::derive_key(KEY_DOMAIN, identity_seed));
    crate::emoji::on_big_stack(
        move || {
            let key =
                HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes(*seed));
            let public = HybridMlDsa65SlhDsa128s::public_key(&key);
            let card = Card {
                version,
                name,
                bio,
                avatar,
                frame,
                theme,
                signer: signer_of(&public),
            };
            check_card(&card)?;
            let signature = HybridMlDsa65SlhDsa128s::sign(&key, &card_message(&card))
                .map_err(|_| FrameError::Signing)?;
            Ok::<_, FrameError>((
                card,
                [public.as_slice(), signature.as_slice(), files.as_slice()].concat(),
            ))
        },
        FrameError::Signing,
    )?
}

/// A verified card's files.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Files {
    pub avatar: Option<Vec<u8>>,
    pub frame: Option<Vec<u8>>,
}

fn take_file<'a>(
    rest: &mut &'a [u8],
    file: Option<&Attachment>,
    check: fn(&[u8]) -> Result<(), FrameError>,
) -> Result<Option<Vec<u8>>, FrameError> {
    let Some(a) = file else { return Ok(None) };
    let (bytes, tail) = rest
        .split_at_checked(file_len(Some(a)))
        .ok_or(FrameError::Truncated)?;
    *rest = tail;
    if sticker_id(bytes) != a.id {
        return Err(FrameError::StickerMismatch);
    }
    check(bytes)?;
    Ok(Some(bytes.to_vec()))
}

/// Verifies a card against its frame body: its fields, the body's length,
/// that the key is the card's signer, both halves of the hybrid signature,
/// and each file against its id and caps. Returns the files.
///
/// # Errors
///
/// What [`check_card`] refuses, [`FrameError::Truncated`] for a body of the
/// wrong length, [`FrameError::BadProof`], [`FrameError::StickerMismatch`],
/// or a file the caps refuse.
pub fn verify_card(card: &Card, body: &[u8]) -> Result<Files, FrameError> {
    check_card(card)?;
    let proof = proof_len();
    let want = proof
        .saturating_add(file_len(card.avatar.as_ref()))
        .saturating_add(file_len(card.frame.as_ref()));
    if body.len() != want {
        return Err(FrameError::Truncated);
    }
    let info = SUITE.info();
    let (public, rest) = body.split_at(info.public_key_len);
    let (signature, mut files) = rest.split_at(info.signature_len);
    if signer_of(public) != card.signer {
        return Err(FrameError::BadProof);
    }
    let (public, signature, message) = (public.to_vec(), signature.to_vec(), card_message(card));
    crate::emoji::on_big_stack(
        move || HybridMlDsa65SlhDsa128s::verify(&public, &message, &signature).is_ok(),
        FrameError::BadProof,
    )?
    .then_some(())
    .ok_or(FrameError::BadProof)?;
    Ok(Files {
        avatar: take_file(&mut files, card.avatar.as_ref(), check_avatar)?,
        frame: take_file(&mut files, card.frame.as_ref(), check_frame)?,
    })
}
