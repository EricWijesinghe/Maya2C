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

use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
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
/// Bytes the sticker cache may hold. Stickers are cached before the app knows
/// whether anyone asked for them, so the cache needs a ceiling of its own.
/// shortcut: a full cache refuses new stickers and never evicts; upgrade to
/// LRU eviction that keeps every sticker the profile or history still names.
pub const STICKER_CACHE_BYTES: u64 = 256 << 20;
const NONCE_LEN: usize = 12;

/// A contact: a name the user chose and a chat address.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Contact {
    pub name: String,
    pub address: String,
}

/// One message in a conversation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Line {
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
/// crash leaves the old file or the new one, never half of each. The bytes
/// are synced before the rename: without that a power cut can keep the rename
/// and lose the data, leaving an empty file that blocks the next start.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&tmp)
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
    // The directory entry for the rename is durable only once the directory is synced.
    #[cfg(unix)]
    {
        if let Some(dir) = path.parent() {
            std::fs::File::open(dir)
                .and_then(|d| d.sync_all())
                .map_err(|e| format!("{}: {e}", dir.display()))?;
        }
    }
    Ok(())
}

/// Bytes of every file in the sticker cache directory.
fn cache_bytes(dir: &Path) -> Result<u64, String> {
    std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|entry| entry.and_then(|e| e.metadata()).map(|m| m.len()))
        .sum::<std::io::Result<u64>>()
        .map_err(|e| format!("{}: {e}", dir.display()))
}

/// A sticker id as a file name: 64 lowercase hex characters and nothing else,
/// so an id from a message can never name a path outside the cache.
fn is_hex_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The app's data directory and its files.
#[derive(Clone, Debug)]
pub struct Files {
    dir: PathBuf,
}

impl Files {
    pub fn new(dir: PathBuf) -> Result<Self, String> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?;
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
        self.save_sticker_within(seed, bytes, STICKER_CACHE_BYTES)
    }

    /// [`Files::save_sticker`] with the cache's byte ceiling as a parameter.
    fn save_sticker_within(
        &self,
        seed: &[u8; 32],
        bytes: &[u8],
        budget: u64,
    ) -> Result<String, String> {
        let id = crate::stickers::sticker_id(bytes);
        let dir = self.dir.join("stickers");
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(format!("{id}.bin"));
        let sealed = seal(seed, STICKER_DOMAIN, bytes)?;
        // A rewrite of the same sticker replaces its bytes, so only the difference counts.
        let replaced = std::fs::metadata(&path).map_or(0, |m| m.len());
        let used = cache_bytes(&dir)?.saturating_sub(replaced);
        if used.saturating_add(u64::try_from(sealed.len()).unwrap_or(u64::MAX)) > budget {
            return Err("the sticker cache on this computer is full".into());
        }
        write_atomic(&path, &sealed)?;
        Ok(id)
    }

    pub fn save_profile(&self, seed: &[u8; 32], profile: &Profile) -> Result<(), String> {
        let plain = Zeroizing::new(serde_json::to_vec(profile).map_err(|e| e.to_string())?);
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|e| format!("no entropy: {e}"))?;
        let key = profile_key(seed);
        let body = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()))
            .encrypt(Nonce::from_slice(&nonce), plain.as_slice())
            .map_err(|_| "encrypting the profile failed".to_string())?;
        write_atomic(
            &self.dir.join("profile.bin"),
            &[nonce.as_slice(), &body].concat(),
        )
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
            contacts: vec![Contact {
                name: "Ann".into(),
                address: "ab".repeat(32),
            }],
            history: vec![Line {
                peer: "ab".repeat(32),
                mine: true,
                text: "hi".into(),
                at: 1,
                sticker: None,
                outbox: None,
            }],
            ..Profile::default()
        };
        files.save_profile(&[1; 32], &profile).unwrap();
        assert_eq!(files.profile(&[1; 32]).unwrap(), profile);
        assert!(
            files.profile(&[2; 32]).is_err(),
            "another identity cannot read it"
        );
        let raw = std::fs::read(dir.join("profile.bin")).unwrap();
        assert!(
            !String::from_utf8_lossy(&raw).contains("Ann"),
            "names are not in the clear"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stickers_are_cached_encrypted_and_checked_on_read() {
        let dir = std::env::temp_dir().join(format!("maya-chat-gui-stk-{}", std::process::id()));
        let files = Files::new(dir.clone()).unwrap();
        let gif = b"GIF89a-test-sticker".to_vec();
        let id = files.save_sticker(&[1; 32], &gif).unwrap();
        assert_eq!(files.sticker(&[1; 32], &id), Some(gif.clone()));
        assert_eq!(
            files.sticker(&[2; 32], &id),
            None,
            "another identity cannot read it"
        );
        assert_eq!(
            files.sticker(&[1; 32], "../profile"),
            None,
            "an id is never a path"
        );
        let raw = std::fs::read(dir.join("stickers").join(format!("{id}.bin"))).unwrap();
        assert!(!raw.windows(6).any(|w| w == b"GIF89a"), "not in the clear");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_full_sticker_cache_refuses_new_stickers_and_keeps_old_ones() {
        let dir = std::env::temp_dir().join(format!("maya-chat-gui-budget-{}", std::process::id()));
        let files = Files::new(dir.clone()).unwrap();
        // One 10-byte sticker seals to 38 bytes: a 12-byte nonce and a 16-byte tag.
        let budget = 40;
        let kept = files
            .save_sticker_within(&[1; 32], b"GIF89a-one", budget)
            .unwrap();
        assert!(
            files
                .save_sticker_within(&[1; 32], b"GIF89a-two", budget)
                .is_err(),
            "over the budget"
        );
        assert_eq!(files.sticker(&[1; 32], &kept), Some(b"GIF89a-one".to_vec()));
        assert!(
            files
                .save_sticker_within(&[1; 32], b"GIF89a-one", budget)
                .is_ok(),
            "a rewrite replaces its bytes"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn files_are_written_for_this_user_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("maya-chat-gui-mode-{}", std::process::id()));
        let files = Files::new(dir.clone()).unwrap();
        files.save_profile(&[1; 32], &Profile::default()).unwrap();
        let mode = std::fs::metadata(dir.join("profile.bin"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_older_profile_without_packs_still_loads() {
        let old = r#"{"relay":"r","contacts":[],"history":[{"peer":"p","mine":true,"text":"hi","at":1}]}"#;
        let profile: Profile = serde_json::from_str(old).unwrap();
        assert!(profile.packs.is_empty() && profile.recent.is_empty());
        assert_eq!(profile.history[0].sticker, None);
    }
}
