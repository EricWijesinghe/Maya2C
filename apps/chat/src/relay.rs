//! Store-and-forward relays (ADR-031): prekey directories and mailboxes.
//!
//! A relay holds signed prekey bundles and encrypted envelopes for
//! recipients who are offline. It sees ciphertext, sizes, timing and the
//! recipient address, and nothing else. A mailbox is emptied only by its
//! owner, who proves it by signing a fresh challenge. Quotas bound what any
//! one mailbox can hold, and envelopes expire, so a relay's storage stays
//! bounded without asking anyone for tokens.

use std::collections::{BTreeMap, VecDeque};

use maya_crypto_pq::suite::{self, SuiteId};
use serde::{Deserialize, Serialize};

use crate::identity::{PrekeyBundle, address_of};
use crate::session::{Handshake, Message};
use crate::{Address, ChatError};

/// Most envelopes one mailbox holds.
pub const MAX_ENVELOPES: usize = 1_000;
/// Most bytes one mailbox holds.
pub const MAX_MAILBOX_BYTES: usize = 16 << 20;
/// Largest single envelope.
pub const MAX_ENVELOPE_BYTES: usize = 256 << 10;
/// Longest an envelope may ask to be kept, seconds.
pub const MAX_TTL: u64 = 30 * 24 * 3_600;
/// How long a fetch challenge stays valid, seconds.
pub const CHALLENGE_TTL: u64 = 120;
/// Most bytes held across every mailbox. Per-mailbox quotas alone do not
/// bound a relay: addresses are free to make.
pub const MAX_RELAY_BYTES: usize = 1 << 30;
/// Most prekey bundles held.
pub const MAX_BUNDLES: usize = 100_000;
/// Most outstanding fetch challenges.
pub const MAX_CHALLENGES: usize = 10_000;
const FETCH_DOMAIN: &[u8] = b"maya-chat mailbox fetch v1";
const STAMP_DOMAIN: &str = "maya-chat 2026-09-28 postage stamp v1";
/// Leading zero bits a deposit's postage stamp needs by default: about a
/// million hashes, well under a second for one message and ~minutes of CPU
/// to fill a 1,000-envelope mailbox. Senders are anonymous to the relay, so
/// work is what a flood costs; a relay may ask for more.
pub const STAMP_BITS: u32 = 20;

/// What an envelope carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Payload {
    /// A new session: the handshake and its first message.
    Open {
        /// The handshake.
        handshake: Handshake,
        /// The first message.
        first: Message,
    },
    /// A message in an existing session.
    Chat(Message),
}

/// A sealed item for one recipient.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// Recipient address.
    pub to: Address,
    /// Unix seconds after which relays drop it.
    pub expires_at: u64,
    /// The content.
    pub payload: Payload,
    /// Postage: see [`Envelope::mint`].
    pub stamp: u64,
}

impl Envelope {
    /// The envelope's canonical bytes (CBOR).
    ///
    /// # Errors
    ///
    /// [`ChatError::Encoding`].
    pub fn encode(&self) -> Result<Vec<u8>, ChatError> {
        let mut out = Vec::new();
        ciborium::into_writer(self, &mut out).map_err(|_| ChatError::Encoding)?;
        Ok(out)
    }

    /// Its id: a hash of everything but the stamp, so a resend is
    /// recognised even with a fresh stamp.
    ///
    /// # Errors
    ///
    /// [`ChatError::Encoding`].
    pub fn id(&self) -> Result<[u8; 32], ChatError> {
        let unstamped = Self {
            stamp: 0,
            ..self.clone()
        };
        Ok(*blake3::hash(&unstamped.encode()?).as_bytes())
    }

    fn stamp_work(id: &[u8; 32], stamp: u64) -> u32 {
        let mut h = blake3::Hasher::new_derive_key(STAMP_DOMAIN);
        h.update(id);
        h.update(&stamp.to_le_bytes());
        let digest = h.finalize();
        let mut head = [0u8; 16];
        head.copy_from_slice(&digest.as_bytes()[..16]);
        u128::from_be_bytes(head).leading_zeros()
    }

    /// Finds a stamp with at least `bits` leading zero bits of work.
    ///
    /// # Errors
    ///
    /// [`ChatError::Encoding`].
    pub fn mint(self, bits: u32) -> Result<Self, ChatError> {
        let id = self.id()?;
        let stamp = (0..=u64::MAX)
            .find(|s| Self::stamp_work(&id, *s) >= bits)
            .ok_or(ChatError::Encoding)?;
        Ok(Self { stamp, ..self })
    }

    /// Whether the stamp carries at least `bits` of work.
    ///
    /// # Errors
    ///
    /// [`ChatError::Encoding`].
    pub fn stamped(&self, bits: u32) -> Result<bool, ChatError> {
        Ok(Self::stamp_work(&self.id()?, self.stamp) >= bits)
    }
}

/// What a mailbox owner signs to empty it.
#[must_use]
pub fn fetch_bytes(address: &Address, nonce: &[u8; 32]) -> Vec<u8> {
    [FETCH_DOMAIN, address.as_slice(), nonce.as_slice()].concat()
}

#[derive(Default)]
struct Mailbox {
    envelopes: VecDeque<([u8; 32], usize, Envelope)>,
    bytes: usize,
}

/// A relay's state.
pub struct Relay {
    stamp_bits: u32,
    bundles: BTreeMap<Address, PrekeyBundle>,
    mailboxes: BTreeMap<Address, Mailbox>,
    /// Ids of stored envelopes, with their expiry. An expired envelope is
    /// refused anyway, so an id is forgotten once it expires.
    seen: BTreeMap<[u8; 32], u64>,
    challenges: BTreeMap<Address, ([u8; 32], u64)>,
    bytes: usize,
}

impl Default for Relay {
    fn default() -> Self {
        Self::new(STAMP_BITS)
    }
}

impl Relay {
    /// A relay requiring `stamp_bits` of postage per deposit.
    #[must_use]
    pub fn new(stamp_bits: u32) -> Self {
        Self {
            stamp_bits,
            bundles: BTreeMap::new(),
            mailboxes: BTreeMap::new(),
            seen: BTreeMap::new(),
            challenges: BTreeMap::new(),
            bytes: 0,
        }
    }

    /// The postage this relay requires.
    #[must_use]
    pub fn stamp_bits(&self) -> u32 {
        self.stamp_bits
    }

    /// Publishes a prekey bundle after checking it; a newer epoch replaces an
    /// older one, an older one is refused.
    ///
    /// # Errors
    ///
    /// A forged or expired bundle, or one older than the one held.
    pub fn publish(&mut self, bundle: PrekeyBundle, now: u64) -> Result<Address, ChatError> {
        let address = bundle.verify(now)?;
        if !self.bundles.contains_key(&address) && self.bundles.len() >= MAX_BUNDLES {
            self.bundles.retain(|_, b| b.expires_at > now);
            if self.bundles.len() >= MAX_BUNDLES {
                return Err(ChatError::Refused("prekey directory full"));
            }
        }
        if self
            .bundles
            .get(&address)
            .is_some_and(|held| held.epoch > bundle.epoch)
        {
            return Err(ChatError::Refused("an older prekey than the one published"));
        }
        self.bundles.insert(address, bundle);
        Ok(address)
    }

    /// The published bundle for `address`, if current.
    #[must_use]
    pub fn bundle(&self, address: &Address, now: u64) -> Option<PrekeyBundle> {
        self.bundles
            .get(address)
            .filter(|b| b.expires_at > now)
            .cloned()
    }

    /// Stores an envelope for its recipient. Idempotent by id.
    ///
    /// # Errors
    ///
    /// Too large, expiring too far out or already expired, or a full mailbox.
    pub fn deposit(&mut self, envelope: Envelope, now: u64) -> Result<[u8; 32], ChatError> {
        let bytes = envelope.encode()?;
        let id = envelope.id()?;
        if self.seen.contains_key(&id) {
            return Ok(id);
        }
        if !envelope.stamped(self.stamp_bits)? {
            return Err(ChatError::Refused("postage stamp carries too little work"));
        }
        if bytes.len() > MAX_ENVELOPE_BYTES {
            return Err(ChatError::Refused("envelope too large"));
        }
        if envelope.expires_at <= now || envelope.expires_at > now + MAX_TTL {
            return Err(ChatError::Refused("expiry outside what this relay keeps"));
        }
        if self.bytes + bytes.len() > MAX_RELAY_BYTES {
            self.expire(now);
            if self.bytes + bytes.len() > MAX_RELAY_BYTES {
                return Err(ChatError::Refused("relay full"));
            }
        }
        let mailbox = self.mailboxes.entry(envelope.to).or_default();
        let before = mailbox.bytes;
        mailbox.envelopes.retain(|(_, _, e)| e.expires_at > now);
        mailbox.bytes = mailbox.envelopes.iter().map(|(_, size, _)| size).sum();
        self.bytes -= before - mailbox.bytes;
        if mailbox.envelopes.len() >= MAX_ENVELOPES
            || mailbox.bytes + bytes.len() > MAX_MAILBOX_BYTES
        {
            return Err(ChatError::Refused("mailbox full"));
        }
        mailbox.bytes += bytes.len();
        self.bytes += bytes.len();
        self.seen.insert(id, envelope.expires_at);
        mailbox.envelopes.push_back((id, bytes.len(), envelope));
        self.seen.retain(|_, expires_at| *expires_at > now);
        Ok(id)
    }

    /// Drops every expired envelope and empty mailbox.
    fn expire(&mut self, now: u64) {
        self.mailboxes.retain(|_, m| {
            m.envelopes.retain(|(_, _, e)| e.expires_at > now);
            m.bytes = m.envelopes.iter().map(|(_, size, _)| size).sum();
            !m.envelopes.is_empty()
        });
        self.bytes = self.mailboxes.values().map(|m| m.bytes).sum();
        self.seen.retain(|_, expires_at| *expires_at > now);
    }

    /// Bytes held across every mailbox.
    #[must_use]
    pub fn stored_bytes(&self) -> usize {
        self.bytes
    }

    /// A fresh challenge for emptying `address`'s mailbox.
    ///
    /// # Errors
    ///
    /// [`ChatError::Entropy`].
    pub fn challenge(&mut self, address: Address, now: u64) -> Result<[u8; 32], ChatError> {
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| ChatError::Entropy)?;
        if self.challenges.len() >= MAX_CHALLENGES {
            self.challenges
                .retain(|_, (_, issued)| now.saturating_sub(*issued) <= CHALLENGE_TTL);
            if self.challenges.len() >= MAX_CHALLENGES {
                return Err(ChatError::Refused("too many outstanding challenges"));
            }
        }
        self.challenges.insert(address, (nonce, now));
        Ok(nonce)
    }

    /// Empties a mailbox for its owner: `identity_key` must be the address's
    /// key and `signature` must sign the current challenge, which is then
    /// spent.
    ///
    /// # Errors
    ///
    /// No or a stale challenge, the wrong key, or a bad signature.
    pub fn fetch(
        &mut self,
        address: Address,
        identity_key: &[u8],
        signature: &[u8],
        now: u64,
    ) -> Result<Vec<Envelope>, ChatError> {
        let (nonce, issued) = self
            .challenges
            .remove(&address)
            .ok_or(ChatError::Refused("no challenge issued"))?;
        if now.saturating_sub(issued) > CHALLENGE_TTL {
            return Err(ChatError::Expired);
        }
        if address_of(identity_key) != address {
            return Err(ChatError::BadSignature);
        }
        suite::verify(
            SuiteId::MlDsa65,
            identity_key,
            &fetch_bytes(&address, &nonce),
            signature,
        )
        .map_err(|_| ChatError::BadSignature)?;
        let mailbox = self.mailboxes.remove(&address).unwrap_or_default();
        self.bytes -= mailbox.bytes;
        Ok(mailbox
            .envelopes
            .into_iter()
            .filter(|(_, _, e)| e.expires_at > now)
            .map(|(_, _, e)| e)
            .collect())
    }
}
