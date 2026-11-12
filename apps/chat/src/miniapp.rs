//! Chat mini-apps: the signed manifest (ADR-138).
//!
//! A mini-app is one HTML file and a manifest. The manifest names the app,
//! pins its icon and entry file by BLAKE3 hash, lists the permissions it asks
//! for from a fixed allow-list, and carries its publisher's signature under
//! the chain's hybrid suite (`0x30`, ML-DSA-65 and SLH-DSA-SHA2-128s). The
//! host runs nothing whose manifest does not verify, and nothing whose entry
//! file does not hash to what the publisher signed.
//!
//! No permission lets an app sign. The most an app can do with money is ask
//! the user's wallet to show a payment for the user to confirm
//! ([`crate::miniapp_bridge`]).

use std::collections::BTreeSet;

use maya_crypto_pq::suite::{self, HybridMlDsa65SlhDsa128s, MasterSeed, SignatureSuite, SuiteId};
use serde::{Deserialize, Serialize};

use crate::{Address, ChatError};

/// Longest app name, in UTF-8 bytes. A policy bound: it has to fit a title bar.
pub const MAX_NAME_BYTES: usize = 48;
/// Largest encoded manifest. The hybrid signature (11,165 B) and key
/// (1,984 B) are most of it.
pub const MAX_MANIFEST_BYTES: usize = 16 * 1024;

const MANIFEST_DOMAIN: &str = "maya-chat 2026-11 mini-app manifest v1";
const SUITE: SuiteId = SuiteId::HybridMlDsa65SlhDsa128s;

/// What a mini-app may ask the host for. The whole allow-list: anything else
/// in a manifest refuses the manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Permission {
    /// Read the user's address. Nothing else about the account.
    ReadAddress,
    /// Ask the wallet to show a payment for the user to confirm.
    RequestPayment,
    /// Send a text message into the chat the app was opened in.
    SendMessage,
}

impl Permission {
    /// Every permission, with its manifest name.
    pub const ALL: [(Self, &'static str); 3] = [
        (Self::ReadAddress, "read-address"),
        (Self::RequestPayment, "request-payment"),
        (Self::SendMessage, "send-message"),
    ];

    /// The permission a manifest name stands for.
    ///
    /// # Errors
    ///
    /// [`ChatError::Refused`] for a name outside the allow-list.
    pub fn parse(name: &str) -> Result<Self, ChatError> {
        Self::ALL
            .iter()
            .find(|(_, n)| *n == name)
            .map(|(p, _)| *p)
            .ok_or(ChatError::Refused("permission is not in the allow-list"))
    }

    /// The manifest name.
    #[must_use]
    pub fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(p, _)| *p == self)
            .map_or("", |(_, n)| n)
    }

    /// What the user is asked to allow, in plain words.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::ReadAddress => "see your address",
            Self::RequestPayment => "ask your wallet to show payments for you to confirm",
            Self::SendMessage => "send messages in this chat, labelled with its name",
        }
    }
}

/// A manifest as published. Read with [`Manifest::from_bytes`], trusted only
/// after [`Manifest::verify`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Shown in the title bar and in every prompt.
    pub name: String,
    /// BLAKE3 of the icon file.
    pub icon_hash: [u8; 32],
    /// BLAKE3 of the entry HTML file.
    pub entry_hash: [u8; 32],
    /// Permission names, in the publisher's order.
    pub permissions: Vec<String>,
    /// The publisher's hybrid public key.
    #[serde(with = "serde_bytes")]
    pub publisher_key: Vec<u8>,
    /// The publisher's hybrid signature over [`Manifest::digest`].
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}

/// A manifest that verified: what the host may act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    /// The app name.
    pub name: String,
    /// BLAKE3 of the icon.
    pub icon_hash: [u8; 32],
    /// BLAKE3 of the entry file.
    pub entry_hash: [u8; 32],
    /// The permissions declared. The user still grants each one.
    pub permissions: BTreeSet<Permission>,
    /// The publisher's account address under the hybrid suite.
    pub publisher: Address,
}

impl Verified {
    /// Whether `html` is the entry file the publisher signed.
    #[must_use]
    pub fn entry_matches(&self, html: &[u8]) -> bool {
        *blake3::hash(html).as_bytes() == self.entry_hash
    }
}

fn put(h: &mut blake3::Hasher, bytes: &[u8]) {
    h.update(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
    h.update(bytes);
}

/// Checks the name and the permissions; returns the permission set.
fn check_fields(name: &str, permissions: &[String]) -> Result<BTreeSet<Permission>, ChatError> {
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return Err(ChatError::Refused("app name is empty or too long"));
    }
    // A name is in every prompt: it may not hide or reorder what follows it.
    if crate::pay::check_memo(name).is_err() || name.contains(['[', ']']) {
        return Err(ChatError::Refused("app name has a forbidden character"));
    }
    let mut set = BTreeSet::new();
    for p in permissions {
        if !set.insert(Permission::parse(p)?) {
            return Err(ChatError::Refused("permission listed twice"));
        }
    }
    Ok(set)
}

impl Manifest {
    /// What the publisher signs: every field but the signature.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new_derive_key(MANIFEST_DOMAIN);
        put(&mut h, self.name.as_bytes());
        put(&mut h, &self.icon_hash);
        put(&mut h, &self.entry_hash);
        put(
            &mut h,
            &u64::try_from(self.permissions.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        for p in &self.permissions {
            put(&mut h, p.as_bytes());
        }
        put(&mut h, &self.publisher_key);
        *h.finalize().as_bytes()
    }

    /// Signs a manifest with the publisher's hybrid key, derived from `seed`.
    ///
    /// # Errors
    ///
    /// [`ChatError::Refused`] for a field rule, [`ChatError::Signature`] if
    /// the signer refuses.
    pub fn sign(
        seed: &MasterSeed,
        name: &str,
        icon_hash: [u8; 32],
        entry_hash: [u8; 32],
        permissions: &[&str],
    ) -> Result<Self, ChatError> {
        let permissions: Vec<String> = permissions.iter().map(|p| (*p).to_owned()).collect();
        check_fields(name, &permissions)?;
        let key = HybridMlDsa65SlhDsa128s::signing_key_from_seed(seed);
        let mut manifest = Self {
            name: name.to_owned(),
            icon_hash,
            entry_hash,
            permissions,
            publisher_key: HybridMlDsa65SlhDsa128s::public_key(&key),
            signature: Vec::new(),
        };
        manifest.signature = HybridMlDsa65SlhDsa128s::sign(&key, &manifest.digest())
            .map_err(|_| ChatError::Signature)?;
        Ok(manifest)
    }

    /// Checks the field rules, then the signature. The allow-list runs first,
    /// so a manifest asking for an unknown permission is refused as such
    /// whoever signed it.
    ///
    /// # Errors
    ///
    /// [`ChatError::Refused`] for a field rule, [`ChatError::BadSignature`]
    /// for a key or signature that does not verify.
    pub fn verify(&self) -> Result<Verified, ChatError> {
        let permissions = check_fields(&self.name, &self.permissions)?;
        suite::verify(SUITE, &self.publisher_key, &self.digest(), &self.signature)
            .map_err(|_| ChatError::BadSignature)?;
        Ok(Verified {
            name: self.name.clone(),
            icon_hash: self.icon_hash,
            entry_hash: self.entry_hash,
            permissions,
            publisher: suite::suite_address(SUITE, &self.publisher_key),
        })
    }

    /// The CBOR encoding a publisher ships.
    ///
    /// # Errors
    ///
    /// [`ChatError::Encoding`].
    pub fn to_bytes(&self) -> Result<Vec<u8>, ChatError> {
        let mut out = Vec::new();
        ciborium::into_writer(self, &mut out).map_err(|_| ChatError::Encoding)?;
        Ok(out)
    }

    /// Reads a manifest. Shape only: call [`Manifest::verify`] before use.
    ///
    /// # Errors
    ///
    /// [`ChatError::Refused`] for an oversize or malformed manifest.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ChatError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ChatError::Refused("manifest is too large"));
        }
        ciborium::from_reader(bytes).map_err(|_| ChatError::Refused("malformed manifest"))
    }
}
