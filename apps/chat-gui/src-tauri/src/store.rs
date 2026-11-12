//! Where the desktop app keeps things.
//!
//! - The identity seed lives in the operating system's credential store
//!   (Windows Credential Manager, macOS Keychain, Secret Service on Linux),
//!   as the wallet's does. It never touches a plain file.
//! - Sessions are `maya_chat::client::Client::save` output, already
//!   encrypted under a seed-derived key.
//! - Contacts, message history and the relay address are one profile file,
//!   encrypted the same way: who someone talks to is as private as what they
//!   say.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use maya_chat::frame::{self, Mark, Quote};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// Keychain service name; `MAYA_CHAT_KEYCHAIN_SERVICE` overrides it so a test
/// never touches a real user's identity.
const SERVICE: &str = "com.maya2c.chat";
const SERVICE_ENV: &str = "MAYA_CHAT_KEYCHAIN_SERVICE";
const SEED_ENTRY: &str = "identity-seed";
const PROFILE_DOMAIN: &str = "maya-chat-gui 2026-09-29 profile v1";
const STICKER_DOMAIN: &str = "maya-chat-gui 2026-10-13 sticker cache v1";
/// Recently sent stickers kept for the picker's first row.
pub const RECENT_LIMIT: usize = 30;
const NONCE_LEN: usize = 12;

/// A contact: a name the user chose and a chat address.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Contact {
    pub name: String,
    pub address: String,
}

/// One message in a conversation.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Line {
    /// The envelope id (hex) both sides name this message by; reactions and
    /// replies point at it (ADR-140). Empty on lines saved before it existed,
    /// which therefore cannot be reacted or replied to.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The other party's address.
    pub peer: String,
    /// Sent by this user, rather than received.
    pub mine: bool,
    pub text: String,
    /// Unix seconds.
    pub at: u64,
    /// Set when the line is a sticker; `text` then holds its emoji.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticker: Option<StickerRef>,
    /// The outbox id (hex) of a message this user sent, while its delivery is
    /// tracked. Cleared once the user dismisses an expired one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbox: Option<String>,
    /// Reaction toggles on this line, from either side ([`frame::apply_mark`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<Mark>,
    /// Set when the line is a reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<ReplyRef>,
    /// This message's disappearing timer, seconds; 0 for none. Fixed when the
    /// message is sent, so a later change of the chat's timer never reaches it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub timer: u64,
    /// When the line is deleted, on [`crate::expiry::Clock`] seconds; 0 while
    /// its clock has not started (a received line not yet shown) or with no
    /// timer. Stored, so a restart deletes it on time ([`crate::expiry`]).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub deadline: u64,
    /// Written by this app (a timer change), never by the peer: a peer who
    /// types the same words is shown as an ordinary message.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub notice: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// What a reply points at.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplyRef {
    /// The envelope id (hex) of the message replied to.
    pub target: String,
    /// [`frame::quote_hash`] of its text, as the sender saw it.
    pub quote: String,
}

/// A line as the window draws it: the stored line plus, for a reply, the
/// quote resolved against this device's own copy of the original.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Shown {
    #[serde(flatten)]
    pub line: Line,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<Quote>,
}

/// `line` resolved for display against `history` (the same chat only).
#[must_use]
pub fn show(history: &[Line], line: &Line) -> Shown {
    let quote = line.reply.as_ref().map(|r| {
        let target = find(history, &line.peer, &r.target).map(|t| t.text.as_str());
        frame::resolve_quote(target, &r.quote)
    });
    Shown {
        line: line.clone(),
        quote,
    }
}

fn is_line(l: &Line, peer: &str, id: &str) -> bool {
    !l.id.is_empty() && l.id == id && l.peer == peer
}

/// The line `id` in the chat with `peer`. A line from another chat is never
/// found, so a frame cannot reach across conversations.
#[must_use]
pub fn find<'a>(history: &'a [Line], peer: &str, id: &str) -> Option<&'a Line> {
    history.iter().find(|l| is_line(l, peer, id))
}

/// Applies a reaction toggle to the line `target` in the chat with `peer`,
/// returning that line's index. A reaction to a line this device does not
/// hold is dropped (`None`).
pub fn react(history: &mut [Line], peer: &str, target: &str, mark: Mark) -> Option<usize> {
    let i = history.iter().position(|l| is_line(l, peer, target))?;
    let line = history.get_mut(i)?;
    line.reactions = frame::apply_mark(&line.reactions, mark);
    Some(i)
}

/// The sticker a line shows.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StickerRef {
    pub id: String,
    pub format: crate::stickers::Format,
    pub pack: String,
    pub title: String,
}

/// Everything that is not the key or the sessions.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub relay: String,
    pub contacts: Vec<Contact>,
    pub history: Vec<Line>,
    /// Installed and made packs, in the picker's order.
    #[serde(default)]
    pub packs: Vec<crate::stickers::Pack>,
    /// Recently sent sticker ids, newest first.
    #[serde(default)]
    pub recent: Vec<String>,
    /// Each chat's disappearing timer by peer address, seconds; a chat not
    /// here has [`crate::expiry::DEFAULT_TIMER`].
    #[serde(default)]
    pub timers: BTreeMap<String, u64>,
    /// Peers whose app has shown it reads timers (sent one, or a timed
    /// message). Any other peer may run an app that keeps messages forever.
    #[serde(default)]
    pub timer_peers: BTreeSet<String>,
    /// The one suggestion to turn timers on has been shown or answered.
    #[serde(default)]
    pub timer_prompted: bool,
    /// The latest time [`crate::expiry::Clock`] has reported, so a clock set
    /// back, even across a restart, never lengthens a message's life.
    #[serde(default)]
    pub clock_floor: u64,
}

fn keychain() -> Result<keyring::Entry, String> {
    let service = std::env::var(SERVICE_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| SERVICE.to_owned());
    keyring::Entry::new(&service, SEED_ENTRY).map_err(|e| format!("opening the keychain: {e}"))
}

/// The stored identity seed, if there is one.
pub fn load_seed() -> Result<Option<Zeroizing<[u8; 32]>>, String> {
    match keychain()?.get_secret() {
        Ok(bytes) => {
            let bytes = Zeroizing::new(bytes);
            let seed: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| "the stored identity is not 32 bytes".to_string())?;
            Ok(Some(Zeroizing::new(seed)))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("reading the keychain: {e}")),
    }
}

/// Stores the identity seed; refuses to replace an existing one.
pub fn store_seed(seed: &[u8; 32]) -> Result<(), String> {
    if load_seed()?.is_some() {
        return Err("an identity already exists on this computer".into());
    }
    keychain()?
        .set_secret(seed)
        .map_err(|e| format!("writing the keychain: {e}"))
}

fn profile_key(seed: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(blake3::derive_key(PROFILE_DOMAIN, seed))
}

/// Encrypts `plain` under the key `domain` derives from `seed`: nonce ‖ body.
fn seal(seed: &[u8; 32], domain: &str, plain: &[u8]) -> Result<Vec<u8>, String> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|e| format!("no entropy: {e}"))?;
    let key = Zeroizing::new(blake3::derive_key(domain, seed));
    let body = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()))
        .encrypt(Nonce::from_slice(&nonce), plain)
        .map_err(|_| "encrypting failed".to_string())?;
    Ok([nonce.as_slice(), &body].concat())
}

/// Reverses [`seal`]; `None` for a damaged file or another identity's.
fn open(seed: &[u8; 32], domain: &str, sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() < NONCE_LEN {
        return None;
    }
    let (nonce, body) = sealed.split_at(NONCE_LEN);
    let key = Zeroizing::new(blake3::derive_key(domain, seed));
    ChaCha20Poly1305::new(Key::from_slice(key.as_ref()))
        .decrypt(Nonce::from_slice(nonce), body)
        .ok()
}

/// Writes `bytes` to `path` through a temporary file and a rename, so a
/// crash leaves the old file or the new one, never half of each.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A sticker id as a file name: 64 lowercase hex characters and nothing else,
/// so an id from a message can never name a path outside the cache.
fn is_hex_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The app's data directory and its files.
#[derive(Clone, Debug)]
pub struct Files {
    dir: PathBuf,
}

impl Files {
    pub fn new(dir: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Self { dir })
    }

    pub fn sessions(&self) -> Option<Vec<u8>> {
        std::fs::read(self.dir.join("sessions.bin")).ok()
    }

    pub fn save_sessions(&self, bytes: &[u8]) -> Result<(), String> {
        write_atomic(&self.dir.join("sessions.bin"), bytes)
    }

    /// The profile, decrypted; an absent file is an empty profile.
    pub fn profile(&self, seed: &[u8; 32]) -> Result<Profile, String> {
        let Ok(sealed) = std::fs::read(self.dir.join("profile.bin")) else {
            return Ok(Profile::default());
        };
        if sealed.len() < NONCE_LEN {
            return Err("the profile file is damaged".into());
        }
        let (nonce, body) = sealed.split_at(NONCE_LEN);
        let key = profile_key(seed);
        let plain = Zeroizing::new(
            ChaCha20Poly1305::new(Key::from_slice(key.as_ref()))
                .decrypt(Nonce::from_slice(nonce), body)
                .map_err(|_| "the profile file does not belong to this identity".to_string())?,
        );
        serde_json::from_slice(&plain).map_err(|e| format!("the profile file is damaged: {e}"))
    }

    /// A cached sticker file, if this computer holds it and it still hashes
    /// to its id.
    pub fn sticker(&self, seed: &[u8; 32], id: &str) -> Option<Vec<u8>> {
        if !is_hex_id(id) {
            return None;
        }
        let sealed = std::fs::read(self.dir.join("stickers").join(format!("{id}.bin"))).ok()?;
        open(seed, STICKER_DOMAIN, &sealed).filter(|bytes| crate::stickers::sticker_id(bytes) == id)
    }

    /// Caches a sticker file under its id, encrypted like the profile: what
    /// someone was sent says who they talk to.
    pub fn save_sticker(&self, seed: &[u8; 32], bytes: &[u8]) -> Result<String, String> {
        let id = crate::stickers::sticker_id(bytes);
        let dir = self.dir.join("stickers");
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        write_atomic(&dir.join(format!("{id}.bin")), &seal(seed, STICKER_DOMAIN, bytes)?)?;
        Ok(id)
    }

    /// Deletes a cached sticker file; one that is not there is already gone.
    /// The file was encrypted under the seed-derived key; flash storage may
    /// keep the old blocks, which no app can prevent (ADR-140 section 5).
    pub fn remove_sticker(&self, id: &str) -> Result<(), String> {
        if !is_hex_id(id) {
            return Err("not a sticker id".into());
        }
        let path = self.dir.join("stickers").join(format!("{id}.bin"));
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", path.display())),
            _ => Ok(()),
        }
    }

    pub fn save_profile(&self, seed: &[u8; 32], profile: &Profile) -> Result<(), String> {
        let plain = Zeroizing::new(serde_json::to_vec(profile).map_err(|e| e.to_string())?);
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|e| format!("no entropy: {e}"))?;
        let key = profile_key(seed);
        let body = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()))
            .encrypt(Nonce::from_slice(&nonce), plain.as_slice())
            .map_err(|_| "encrypting the profile failed".to_string())?;
        write_atomic(&self.dir.join("profile.bin"), &[nonce.as_slice(), &body].concat())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_round_trips_and_is_useless_without_the_seed() {
        let dir = std::env::temp_dir().join(format!("maya-chat-gui-test-{}", std::process::id()));
        let files = Files::new(dir.clone()).unwrap();
        let profile = Profile {
            relay: "/dns4/seed1.maya2c.dev/tcp/4001/p2p/x".into(),
            contacts: vec![Contact { name: "Ann".into(), address: "ab".repeat(32) }],
            history: vec![Line { peer: "ab".repeat(32), mine: true, text: "hi".into(), at: 1, ..Line::default() }],
            ..Profile::default()
        };
        files.save_profile(&[1; 32], &profile).unwrap();
        assert_eq!(files.profile(&[1; 32]).unwrap(), profile);
        assert!(files.profile(&[2; 32]).is_err(), "another identity cannot read it");
        let raw = std::fs::read(dir.join("profile.bin")).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("Ann"), "names are not in the clear");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stickers_are_cached_encrypted_and_checked_on_read() {
        let dir = std::env::temp_dir().join(format!("maya-chat-gui-stk-{}", std::process::id()));
        let files = Files::new(dir.clone()).unwrap();
        let gif = b"GIF89a-test-sticker".to_vec();
        let id = files.save_sticker(&[1; 32], &gif).unwrap();
        assert_eq!(files.sticker(&[1; 32], &id), Some(gif.clone()));
        assert_eq!(files.sticker(&[2; 32], &id), None, "another identity cannot read it");
        assert_eq!(files.sticker(&[1; 32], "../profile"), None, "an id is never a path");
        let raw = std::fs::read(dir.join("stickers").join(format!("{id}.bin"))).unwrap();
        assert!(!raw.windows(6).any(|w| w == b"GIF89a"), "not in the clear");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_older_profile_without_packs_still_loads() {
        let old = r#"{"relay":"r","contacts":[],"history":[{"peer":"p","mine":true,"text":"hi","at":1}]}"#;
        let profile: Profile = serde_json::from_str(old).unwrap();
        assert!(profile.packs.is_empty() && profile.recent.is_empty());
        assert_eq!(profile.history[0].sticker, None);
        assert!(profile.history[0].id.is_empty() && profile.history[0].reactions.is_empty());
        assert_eq!(profile.history[0].reply, None);
    }

    const PEER: &str = "aa";
    const OTHER: &str = "bb";

    fn line(peer: &str, id: u8, text: &str) -> Line {
        Line { id: hex::encode([id; 32]), peer: peer.into(), text: text.into(), ..Line::default() }
    }

    fn thumbs(on: bool, at: u64) -> Mark {
        Mark { mine: false, emoji: "\u{1f44d}".into(), on, at }
    }

    fn reply_to(id: u8, quoted: &str) -> Line {
        Line {
            peer: PEER.into(),
            text: "ok".into(),
            reply: Some(ReplyRef { target: hex::encode([id; 32]), quote: frame::quote_hash(quoted) }),
            ..Line::default()
        }
    }

    #[test]
    fn a_reaction_to_an_unknown_id_is_dropped() {
        let mut history = vec![line(PEER, 1, "hi")];
        assert_eq!(react(&mut history, PEER, &hex::encode([2; 32]), thumbs(true, 1)), None);
        assert!(history[0].reactions.is_empty());
        let mut old = vec![Line { peer: PEER.into(), ..Line::default() }];
        assert_eq!(react(&mut old, PEER, "", thumbs(true, 1)), None, "a line without an id cannot be targeted");
    }

    #[test]
    fn a_reaction_or_reply_naming_a_line_from_another_chat_finds_nothing() {
        let mut history = vec![line(OTHER, 1, "secret")];
        assert_eq!(react(&mut history, PEER, &hex::encode([1; 32]), thumbs(true, 1)), None);
        assert!(history[0].reactions.is_empty());
        assert_eq!(show(&history, &reply_to(1, "secret")).quote, Some(Quote::Missing), "no quote across chats");
    }

    #[test]
    fn a_second_reaction_from_one_sender_replaces_the_first() {
        let mut history = vec![line(PEER, 1, "hi")];
        let target = hex::encode([1; 32]);
        assert_eq!(react(&mut history, PEER, &target, thumbs(true, 1)), Some(0));
        assert_eq!(react(&mut history, PEER, &target, thumbs(false, 2)), Some(0));
        assert_eq!(history[0].reactions, vec![thumbs(false, 2)]);
    }

    #[test]
    fn a_reply_whose_quote_hash_mismatches_shows_no_quote() {
        let history = vec![line(PEER, 1, "see you at 5")];
        assert_eq!(show(&history, &reply_to(1, "see you at 5")).quote, Some(Quote::Shown("see you at 5".into())));
        assert_eq!(show(&history, &reply_to(1, "see you at 6")).quote, Some(Quote::Mismatch));
        assert_eq!(show(&[], &reply_to(1, "see you at 5")).quote, Some(Quote::Missing), "original not available");
        assert_eq!(show(&history, &history[0]).quote, None, "not a reply");
    }

    #[test]
    fn reactions_and_replies_survive_a_save() {
        let dir = std::env::temp_dir().join(format!("maya-chat-gui-react-{}", std::process::id()));
        let files = Files::new(dir.clone()).unwrap();
        let mut target = line(PEER, 1, "hi");
        target.reactions = vec![thumbs(true, 3)];
        let answer = Line { id: hex::encode([2; 32]), ..reply_to(1, "hi") };
        let profile = Profile { history: vec![target, answer], ..Profile::default() };
        files.save_profile(&[1; 32], &profile).unwrap();
        assert_eq!(files.profile(&[1; 32]).unwrap(), profile);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
