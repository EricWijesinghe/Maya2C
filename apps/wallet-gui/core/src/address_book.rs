//! The address book (ADR-086 §7) and the poisoning defenses that read it (§6.2).
//!
//! ## Names are the user's
//!
//! A name in the book is only ever one the user typed. A payment request's
//! `label` is a claim by whoever made the QR code; it may be offered as a
//! suggestion, but [`AddressBook::add`] takes the name as an argument the GUI
//! fills from a text field, never from a request. Names are NFC-normalized and
//! stripped of bidirectional and other invisible controls, so `Alice` written
//! with a right-to-left override cannot sort or render as somebody else, and a
//! name that mixes alphabets or reads like an existing one raises a warning.
//!
//! ## At rest
//!
//! The book is a file in the wallet's data directory, ChaCha20-Poly1305 under
//! a [`BookKey`] derived from the seed. The seed itself is sealed under the
//! vault's Argon2id passphrase key in the OS keychain, so reading the book
//! needs what unlocking the wallet needs. It is not a keychain entry because
//! the Windows Credential Manager caps a secret at 2,560 bytes, which a book
//! of a few dozen contacts exceeds. An export is the same sealed bytes: it
//! opens in any wallet restored from the same recovery phrase, and nowhere
//! else.
//!
//! ```text
//! magic   8 bytes   "MAYABOK1"
//! version 1 byte    currently 1
//! nonce  12 bytes   ChaCha20-Poly1305 nonce
//! cipher N bytes    JSON contact list + 16-byte tag; magic and version are AAD
//! ```

use std::collections::BTreeSet;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use maya_clear_sign::Network;
use primitive_types::U256;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::error::{Result, WalletError};

const MAGIC: &[u8; 8] = b"MAYABOK1";
const VERSION: u8 = 1;
const NONCE_LEN: usize = 12;
const HEADER_LEN: usize = MAGIC.len() + 1 + NONCE_LEN;
/// BLAKE3 key-derivation context for [`BookKey`]; changing it orphans every book.
const KEY_CONTEXT: &str = "maya2c wallet address book v1";
/// Suffix of the temporary file a save writes before the atomic rename.
const TMP_SUFFIX: &str = "tmp";

/// Longest name, in characters after cleaning.
pub const MAX_NAME_CHARS: usize = 64;
/// Longest note, in characters after cleaning.
pub const MAX_NOTE_CHARS: usize = 280;
/// Most entries a book holds; also bounds what an import can add.
pub const MAX_CONTACTS: usize = 5_000;

/// The key that seals one wallet's address book. Wiped on drop, never printed.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct BookKey([u8; 32]);

impl BookKey {
    /// The book key for a seed.
    #[must_use]
    pub fn derive(seed: &[u8]) -> Self {
        Self(blake3::derive_key(KEY_CONTEXT, seed))
    }
}

impl std::fmt::Debug for BookKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BookKey(<redacted>)")
    }
}

/// How an entry got into the book.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Pasted or typed by the user.
    Manual,
    /// Read from a scanned QR code.
    Scanned,
    /// Saved after paying it.
    PaidTo,
}

/// One saved address.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Contact {
    /// The account id; stored as 64 hex characters, not a JSON number array.
    #[serde(with = "hex_address")]
    pub address: [u8; 32],
    /// The address prefix it was saved for (`maya`, `tmaya`, `dmaya`).
    pub network: String,
    /// The user's name for it, cleaned by [`clean_name`].
    pub name: String,
    /// Chain height when saved, for display only.
    pub added_at_height: u64,
    /// How it got here.
    pub source: Source,
    /// The user's note, cleaned like a name.
    pub note: String,
}

/// A name as stored, and what the user should hear about it.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct NameCheck {
    /// The cleaned name.
    pub name: String,
    /// Confusable or mixed-alphabet warnings. Empty when the name is plain.
    pub warnings: Vec<String>,
}

/// What a pasted address turned out to be.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct PasteCheck {
    /// The address, lowercase hex: what the transfer is built from.
    pub address: String,
    /// The address in grouped Bech32m: what the read-back shows.
    pub grouped: String,
    /// The saved name, when the address is in the book.
    pub contact: Option<String>,
    /// A saved contact this address looks like without being it.
    pub look_alike_of: Option<String>,
    /// Never saved and never paid: the new-recipient interstitial is mandatory.
    pub new_recipient: bool,
}

/// A local address book.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddressBook {
    contacts: Vec<Contact>,
}

impl AddressBook {
    /// Every entry, in the order added.
    #[must_use]
    pub fn contacts(&self) -> &[Contact] {
        &self.contacts
    }

    /// The entry for `address`, if saved.
    #[must_use]
    pub fn find(&self, address: &[u8; 32]) -> Option<&Contact> {
        self.contacts.iter().find(|c| c.address == *address)
    }

    /// Saves `address` under the user-typed `name`.
    ///
    /// # Errors
    ///
    /// [`WalletError::Contact`] for an empty or over-long name or note, an
    /// address already saved, or a full book.
    pub fn add(
        &mut self,
        address: [u8; 32],
        network: Network,
        name: &str,
        note: &str,
        source: Source,
        added_at_height: u64,
    ) -> Result<NameCheck> {
        let check = self.check_add(&address, name)?;
        let note = clean_text(note, MAX_NOTE_CHARS, "note")?;
        if self.contacts.len() >= MAX_CONTACTS {
            return Err(WalletError::Contact(format!(
                "the book holds at most {MAX_CONTACTS} entries"
            )));
        }
        self.contacts.push(Contact {
            address,
            network: network.hrp().to_string(),
            name: check.name.clone(),
            added_at_height,
            source,
            note,
        });
        Ok(check)
    }

    /// What [`AddressBook::add`] would save and warn about, without saving:
    /// the text the native confirm dialog shows before a contact is added.
    ///
    /// # Errors
    ///
    /// [`WalletError::Contact`] for an unusable name or an address already saved.
    pub fn check_add(&self, address: &[u8; 32], name: &str) -> Result<NameCheck> {
        let name = clean_name(name)?;
        if let Some(existing) = self.find(address) {
            return Err(WalletError::Contact(format!(
                "this address is already saved as '{}'",
                existing.name
            )));
        }
        let mut warnings = self.name_warnings(&name);
        // A poisoner's look-alike saved under a new name would become a
        // contact the confirm screen trusts.
        if let Some(c) = self.look_alike_of(address) {
            warnings.push(format!(
                "this address looks like the saved contact '{}' but is a different address; compare every group",
                c.name
            ));
        }
        Ok(NameCheck { name, warnings })
    }

    /// Removes the entry for `address`. Returns whether one was there.
    pub fn remove(&mut self, address: &[u8; 32]) -> bool {
        let before = self.contacts.len();
        self.contacts.retain(|c| c.address != *address);
        self.contacts.len() != before
    }

    /// The addresses `maya_clear_sign::Context::contacts` reviews against.
    #[must_use]
    pub fn review_contacts(&self) -> BTreeSet<[u8; 32]> {
        self.contacts.iter().map(|c| c.address).collect()
    }

    fn name_warnings(&self, name: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        if mixes_alphabets(name) {
            warnings.push(
                "this name mixes alphabets (for example Latin and Cyrillic), which can make it read like another".to_string(),
            );
        }
        let skeleton = confusable_skeleton(name);
        for c in &self.contacts {
            if confusable_skeleton(&c.name) == skeleton {
                warnings.push(format!("this name reads like the saved name '{}'", c.name));
            }
        }
        warnings
    }

    /// The saved contact `address` looks like without being one: the same
    /// first and last group of the grouped Bech32m display, or the same first
    /// and last two bytes.
    #[must_use]
    pub fn look_alike_of(&self, address: &[u8; 32]) -> Option<&Contact> {
        if self.find(address).is_some() {
            return None;
        }
        self.contacts.iter().find(|c| {
            (c.address[..2] == address[..2] && c.address[30..] == address[30..])
                || maya_clear_sign::text_look_alike(&c.address, address)
        })
    }

    /// Checks a pasted address before it fills the send form: it must be an
    /// address for `network`, and the result says whether it is saved, a
    /// look-alike of a saved one, or new (never saved, never in `paid`).
    ///
    /// # Errors
    ///
    /// [`WalletError::Address`] for anything that is not an address, or a
    /// Bech32m address for another network.
    pub fn check_paste(
        &self,
        text: &str,
        network: Network,
        paid: &BTreeSet<[u8; 32]>,
    ) -> Result<PasteCheck> {
        let address = crate::payment::decode_address_for(text, network)?;
        Ok(PasteCheck {
            address: hex::encode(address),
            grouped: maya_clear_sign::grouped(network, &address),
            contact: self.find(&address).map(|c| c.name.clone()),
            look_alike_of: self.look_alike_of(&address).map(|c| c.name.clone()),
            new_recipient: self.find(&address).is_none() && !paid.contains(&address),
        })
    }

    /// Seals the book under `key`.
    ///
    /// # Errors
    ///
    /// [`WalletError::Entropy`] if the OS entropy source fails.
    pub fn seal(&self, key: &BookKey) -> Result<Vec<u8>> {
        let plain = Zeroizing::new(
            serde_json::to_vec(&self.contacts)
                .map_err(|e| WalletError::Format(format!("address book: {e}")))?,
        );
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce)
            .map_err(|e| WalletError::Entropy(format!("OS entropy unavailable: {e}")))?;
        let cipher = ChaCha20Poly1305::new_from_slice(&key.0)
            .map_err(|_| WalletError::Format("book key has the wrong length".to_string()))?;
        let mut out = Vec::with_capacity(HEADER_LEN + plain.len() + 16);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        let sealed = cipher
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: &plain,
                    aad: &out,
                },
            )
            .map_err(|_| WalletError::Format("address book encryption failed".to_string()))?;
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Opens a sealed book. Names and notes are cleaned again, so an import
    /// cannot bring in a name `add` would have refused.
    ///
    /// # Errors
    ///
    /// [`WalletError::Format`] for a malformed record; [`WalletError::Unlock`]
    /// for the wrong key or a tampered record.
    pub fn open(sealed: &[u8], key: &BookKey) -> Result<Self> {
        if sealed.len() < HEADER_LEN || &sealed[..MAGIC.len()] != MAGIC {
            return Err(WalletError::Format("not an address book".to_string()));
        }
        if sealed[MAGIC.len()] != VERSION {
            return Err(WalletError::Format(format!(
                "address book version {} is not known",
                sealed[MAGIC.len()]
            )));
        }
        let aad = &sealed[..=MAGIC.len()];
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&sealed[aad.len()..HEADER_LEN]);
        let cipher = ChaCha20Poly1305::new_from_slice(&key.0)
            .map_err(|_| WalletError::Format("book key has the wrong length".to_string()))?;
        let plain = Zeroizing::new(
            cipher
                .decrypt(
                    &Nonce::from(nonce),
                    Payload {
                        msg: &sealed[HEADER_LEN..],
                        aad,
                    },
                )
                .map_err(|_| WalletError::Unlock)?,
        );
        let stored: Vec<Contact> = serde_json::from_slice(&plain)
            .map_err(|e| WalletError::Format(format!("address book: {e}")))?;
        let mut book = Self::default();
        for c in stored {
            let network = Network::from_hrp(&c.network).ok_or_else(|| {
                WalletError::Format(format!("unknown address prefix '{}'", c.network))
            })?;
            book.add(
                c.address,
                network,
                &c.name,
                &c.note,
                c.source,
                c.added_at_height,
            )?;
        }
        Ok(book)
    }

    /// Adds every entry of a sealed export that is not already here.
    /// Returns how many were added.
    ///
    /// # Errors
    ///
    /// As [`AddressBook::open`], and a full book.
    pub fn import(&mut self, sealed: &[u8], key: &BookKey) -> Result<usize> {
        let incoming = Self::open(sealed, key)?;
        let mut added = 0;
        for c in incoming.contacts {
            if self.find(&c.address).is_some() {
                continue;
            }
            let network = Network::from_hrp(&c.network)
                .ok_or_else(|| WalletError::Format("unknown address prefix".to_string()))?;
            self.add(
                c.address,
                network,
                &c.name,
                &c.note,
                c.source,
                c.added_at_height,
            )?;
            added += 1;
        }
        Ok(added)
    }

    /// Loads the book sealed at `path`; no file yet is an empty book.
    ///
    /// # Errors
    ///
    /// [`WalletError::Contact`] if the file cannot be read, and
    /// [`AddressBook::open`]'s errors.
    pub fn load_file(path: &std::path::Path, key: &BookKey) -> Result<Self> {
        match std::fs::read(path) {
            Ok(sealed) => Self::open(&sealed, key),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(WalletError::Contact(format!(
                "cannot read the address book: {e}"
            ))),
        }
    }

    /// Seals this book to `path`, through a temporary file and a rename so a
    /// crash mid-write leaves the old book, never half of a new one.
    ///
    /// # Errors
    ///
    /// Sealing failures, and [`WalletError::Contact`] if the file cannot be written.
    pub fn save_file(&self, path: &std::path::Path, key: &BookKey) -> Result<()> {
        let sealed = self.seal(key)?;
        let tmp = path.with_extension(TMP_SUFFIX);
        let io =
            |e: std::io::Error| WalletError::Contact(format!("cannot write the address book: {e}"));
        std::fs::write(&tmp, &sealed).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }
}

/// Bidirectional overrides, isolates and marks, and zero-width characters:
/// invisible, and able to make one name render as another.
fn is_invisible(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}'
        | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' | '\u{E0000}'..='\u{E007F}')
        || c.is_control()
}

/// `input` without invisible characters (see [`is_invisible`]). Payment
/// request labels and messages go through this too.
pub(crate) fn strip_invisible(input: &str) -> String {
    input.chars().filter(|c| !is_invisible(*c)).collect()
}

/// An address as 64 lowercase hex characters in the sealed JSON.
mod hex_address {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(a: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(a))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let text = String::deserialize(d)?;
        let bytes = hex::decode(&text).map_err(serde::de::Error::custom)?;
        <[u8; 32]>::try_from(bytes.as_slice())
            .map_err(|_| serde::de::Error::custom("an address is 32 bytes"))
    }
}

fn clean_text(input: &str, max: usize, what: &str) -> Result<String> {
    let cleaned: String = input.nfc().filter(|c| !is_invisible(*c)).collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.chars().count() > max {
        return Err(WalletError::Contact(format!(
            "a {what} is at most {max} characters"
        )));
    }
    Ok(cleaned)
}

/// A name as stored: NFC, invisible characters removed, trimmed, 1 to
/// [`MAX_NAME_CHARS`] characters.
///
/// # Errors
///
/// [`WalletError::Contact`] for a name that is empty after cleaning or too long.
pub fn clean_name(input: &str) -> Result<String> {
    let name = clean_text(input, MAX_NAME_CHARS, "name")?;
    if name.is_empty() {
        return Err(WalletError::Contact("a name is required".to_string()));
    }
    Ok(name)
}

/// Latin letters together with letters of another alphabet.
fn mixes_alphabets(name: &str) -> bool {
    let latin = name.chars().any(|c| c.is_ascii_alphabetic());
    let other = name.chars().any(|c| c.is_alphabetic() && !c.is_ascii());
    latin && other
}

/// A rough shape of a name: compatibility-normalized, lowercased, with the
/// commonest look-alike letters and digits folded together.
///
/// shortcut: a small hand table, not Unicode's confusables.txt (UTS #39);
/// upgrade if a name that slipped through is reported.
fn confusable_skeleton(name: &str) -> String {
    name.nfkc()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            'а' | 'α' => 'a',
            'е' | 'ε' => 'e',
            'о' | 'ο' | '0' => 'o',
            'р' | 'ρ' => 'p',
            'с' | 'ϲ' => 'c',
            'у' | 'γ' => 'y',
            'х' | 'χ' => 'x',
            'і' | 'ι' | 'ӏ' | '1' | 'l' | '|' => 'i',
            'ј' => 'j',
            'ѕ' | '5' => 's',
            'ԁ' => 'd',
            'ν' => 'v',
            'τ' => 't',
            'κ' => 'k',
            'ь' => 'b',
            other => other,
        })
        .collect()
}

/// One entry of account activity, as the history screen would list it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activity {
    /// The other party.
    pub counterparty: [u8; 32],
    /// Amount moved, base units.
    pub amount: U256,
    /// Received rather than sent.
    pub incoming: bool,
}

/// The counterparties a "recent" list may offer, newest first as given.
///
/// Every address the user paid is listed. An incoming transfer is listed only
/// when its sender is saved or was paid before, and it is at least `dust`:
/// address poisoning works by sending a look-alike a tiny amount so it lands
/// in "recent" for the victim to copy.
#[must_use]
pub fn recent(activity: &[Activity], book: &AddressBook, dust: U256) -> Vec<[u8; 32]> {
    let paid: BTreeSet<[u8; 32]> = activity
        .iter()
        .filter(|a| !a.incoming)
        .map(|a| a.counterparty)
        .collect();
    let mut out: Vec<[u8; 32]> = Vec::new();
    for a in activity {
        let known = paid.contains(&a.counterparty) || book.find(&a.counterparty).is_some();
        let shown = !a.incoming || (known && a.amount >= dust);
        if shown && !out.contains(&a.counterparty) {
            out.push(a.counterparty);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn invisible_controls_are_stripped_and_nfc_applied() {
        // "Alice" with a right-to-left override and a zero-width space.
        assert_eq!(clean_name("\u{202E}Ali\u{200B}ce ").unwrap(), "Alice");
        // e + combining acute becomes the single precomposed character.
        assert_eq!(clean_name("Rene\u{0301}").unwrap(), "Ren\u{e9}");
        assert!(clean_name("\u{202E}\u{200B}").is_err());
        assert!(clean_name(&"x".repeat(MAX_NAME_CHARS + 1)).is_err());
    }

    #[test]
    fn the_book_key_debug_is_redacted() {
        let key = BookKey::derive(&[7; 64]);
        assert_eq!(format!("{key:?}"), "BookKey(<redacted>)");
    }
}
