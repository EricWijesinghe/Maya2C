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
}

/// Everything that is not the key or the sessions.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub relay: String,
    pub contacts: Vec<Contact>,
    pub history: Vec<Line>,
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

/// Writes `bytes` to `path` through a temporary file and a rename, so a
/// crash leaves the old file or the new one, never half of each.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
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
            history: vec![Line { peer: "ab".repeat(32), mine: true, text: "hi".into(), at: 1 }],
        };
        files.save_profile(&[1; 32], &profile).unwrap();
        assert_eq!(files.profile(&[1; 32]).unwrap(), profile);
        assert!(files.profile(&[2; 32]).is_err(), "another identity cannot read it");
        let raw = std::fs::read(dir.join("profile.bin")).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("Ann"), "names are not in the clear");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
