//! Store-and-forward relays (ADR-031): prekey directories and mailboxes.
//!
//! A relay holds signed prekey bundles and encrypted envelopes for
//! recipients who are offline. It sees ciphertext, sizes, timing and the
//! recipient address, and nothing else. A mailbox is emptied only by its
//! owner, who proves it by signing a fresh challenge. Quotas bound what any
//! one mailbox can hold, and envelopes expire, so a relay's storage stays
//! bounded without asking anyone for tokens.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use maya_crypto_pq::suite::{self, SuiteId};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::identity::{PrekeyBundle, address_of};
use crate::session::{Handshake, Message};
use crate::wire;
use crate::{Address, ChatError};

/// Most envelopes one mailbox holds.
pub const MAX_ENVELOPES: usize = 1_000;
/// Most bytes one mailbox holds. A fetch writes the envelopes as CBOR integer
/// arrays, up to twice their stored size, and the whole mailbox goes in one
/// response, so this keeps that response under the client's response cap.
pub const MAX_MAILBOX_BYTES: usize = 9 << 20;
/// Largest single envelope.
pub const MAX_ENVELOPE_BYTES: usize = 256 << 10;
/// Longest an envelope may ask to be kept, seconds.
pub const MAX_TTL: u64 = 30 * 24 * 3_600;
/// Longest a prekey bundle may run, seconds from now. Clients publish 30 days
/// out; the spare day covers clock skew. Without a bound a bundle could hold
/// its directory slot forever.
pub const MAX_PREKEY_TTL: u64 = 31 * 24 * 3_600;
/// How long a fetch challenge stays valid, seconds.
pub const CHALLENGE_TTL: u64 = 120;
/// Most bytes held across every mailbox. Per-mailbox quotas alone do not
/// bound a relay: addresses are free to make. At the cap the largest
/// mailbox gives way to a smaller one (ADR-149).
pub const MAX_RELAY_BYTES: usize = 1 << 30;
/// Most prekey bundles held.
pub const MAX_BUNDLES: usize = 100_000;
/// Most spent fetch challenges remembered. Challenges are stateless (ADR-149),
/// so only spent ones are stored, and only for [`CHALLENGE_TTL`]; past the cap
/// the oldest is forgotten, which reopens nothing a third party can use: the
/// fetch it guards was signed for this relay and travelled encrypted.
pub const MAX_SPENT_CHALLENGES: usize = 100_000;
/// Most fetched envelope ids remembered so that a sender's resend after the
/// fetch is not delivered twice. Past the cap the soonest to expire is
/// forgotten; a resend of it is delivered again and the session refuses it as
/// a replay (ADR-149).
pub const MAX_FETCHED_IDS: usize = 1 << 16;
// v2 adds the relay id, so a signature for one relay is refused by another.
const FETCH_DOMAIN: &[u8] = b"maya-chat mailbox fetch v2";
const CHALLENGE_DOMAIN: &str = "maya-chat 2026-11-21 fetch challenge v1";
const STAMP_DOMAIN: &str = "maya-chat 2026-09-28 postage stamp v1";
/// Leading zero bits a deposit's postage stamp needs by default: about a
/// million hashes, well under a second for one message and ~minutes of CPU
/// to fill a 1,000-envelope mailbox. Senders are anonymous to the relay, so
/// work is what a flood costs; a relay may ask for more.
pub const STAMP_BITS: u32 = 20;
/// Most postage a sender will mint. A stamp costs about 2^bits hashes, and a
/// relay's quote comes from the network, so a larger one is refused rather
/// than ground out for hours. A relay that wants more than this is unusable.
pub const MAX_POSTAGE_BITS: u32 = 24;

/// The envelope encoding this build reads and writes, and the newest it
/// accepts. Version 1 is the original CBOR (integer arrays for bytes);
/// version 2 writes every byte string as a CBOR byte string (ADR-074, decision 4).
pub const MAX_ENVELOPE_VERSION: u8 = 2;

fn envelope_v1() -> u8 {
    1
}

// serde passes the field by reference, so the lint's advice does not apply.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_v1(version: &u8) -> bool {
    *version == 1
}

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
    /// Encoding version. Absent (so not written) for version 1, which keeps
    /// version-1 bytes and their ids unchanged.
    #[serde(default = "envelope_v1", skip_serializing_if = "is_v1")]
    pub version: u8,
}

impl Envelope {
    /// The envelope's canonical bytes (CBOR), in its own version's encoding.
    ///
    /// # Errors
    ///
    /// [`ChatError::Encoding`], or an unknown version.
    pub fn encode(&self) -> Result<Vec<u8>, ChatError> {
        let mut out = Vec::new();
        match self.version {
            1 => ciborium::into_writer(self, &mut out),
            2 => ciborium::into_writer(&wire::EnvelopeV2::from(self), &mut out),
            _ => return Err(ChatError::Encoding),
        }
        .map_err(|_| ChatError::Encoding)?;
        Ok(out)
    }

    /// Reads an envelope in either encoding (the version is inside it).
    ///
    /// # Errors
    ///
    /// [`ChatError::Encoding`] for bytes that are not an envelope, or
    /// [`ChatError::Refused`] for a version this build does not read.
    pub fn decode(bytes: &[u8]) -> Result<Self, ChatError> {
        let envelope: Self = ciborium::from_reader(bytes).map_err(|_| ChatError::Encoding)?;
        if !(1..=MAX_ENVELOPE_VERSION).contains(&envelope.version) {
            return Err(ChatError::Refused("unknown envelope version"));
        }
        Ok(envelope)
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
        if bits > MAX_POSTAGE_BITS {
            return Err(ChatError::Refused("postage above the most a sender mints"));
        }
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

/// What a mailbox owner signs to empty it at the relay whose id is `relay`
/// (its libp2p peer id bytes; [`Relay::id`]).
#[must_use]
pub fn fetch_bytes(relay: &[u8], address: &Address, nonce: &[u8; 32]) -> Vec<u8> {
    let len = u32::try_from(relay.len()).unwrap_or(u32::MAX).to_be_bytes();
    [
        FETCH_DOMAIN,
        len.as_slice(),
        relay,
        address.as_slice(),
        nonce.as_slice(),
    ]
    .concat()
}

#[derive(Default)]
struct Mailbox {
    envelopes: VecDeque<([u8; 32], usize, Envelope)>,
    bytes: usize,
}

/// A relay's state.
pub struct Relay {
    /// This relay's identity, bound into every fetch signature.
    id: Vec<u8>,
    stamp_bits: u32,
    bundles: BTreeMap<Address, PrekeyBundle>,
    mailboxes: BTreeMap<Address, Mailbox>,
    /// Ids of stored envelopes, with their expiry. An expired envelope is
    /// refused anyway, so an id is forgotten once it expires.
    seen: BTreeMap<[u8; 32], u64>,
    /// Stored envelopes ordered by expiry, with each one's mailbox, so expiry
    /// pops only what has expired instead of scanning everything held.
    expiries: BTreeSet<(u64, [u8; 32], Address)>,
    /// Ids in `seen` whose envelope was fetched, by expiry.
    fetched: BTreeSet<(u64, [u8; 32])>,
    /// Mailboxes by size, so the largest is found when the relay is full.
    by_size: BTreeSet<(usize, Address)>,
    /// Keys challenge nonces; made on the first challenge.
    challenge_key: Option<Zeroizing<[u8; 32]>>,
    /// Spent challenge nonces, by issue time.
    spent: BTreeSet<(u64, [u8; 32])>,
    bytes: usize,
    capacity: usize,
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
            id: Vec::new(),
            stamp_bits,
            bundles: BTreeMap::new(),
            mailboxes: BTreeMap::new(),
            seen: BTreeMap::new(),
            expiries: BTreeSet::new(),
            fetched: BTreeSet::new(),
            by_size: BTreeSet::new(),
            challenge_key: None,
            spent: BTreeSet::new(),
            bytes: 0,
            capacity: MAX_RELAY_BYTES,
        }
    }

    /// This relay holding at most `bytes` across every mailbox instead of
    /// [`MAX_RELAY_BYTES`].
    #[must_use]
    pub fn with_capacity(mut self, bytes: usize) -> Self {
        self.capacity = bytes;
        self
    }

    /// This relay with identity `id` (its libp2p peer id bytes), which every
    /// fetch signature must name.
    #[must_use]
    pub fn with_id(mut self, id: Vec<u8>) -> Self {
        self.id = id;
        self
    }

    /// The identity fetch signatures must name.
    #[must_use]
    pub fn id(&self) -> &[u8] {
        &self.id
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
        // Checked before the signature: it is a comparison, not an ML-DSA verify.
        // Signed into the bundle, so refused rather than clamped.
        if bundle.expires_at > now.saturating_add(MAX_PREKEY_TTL) {
            return Err(ChatError::Refused("prekey outlives what this relay keeps"));
        }
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
        self.expire(now);
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
        let to = envelope.to;
        let (count, held) = self
            .mailboxes
            .get(&to)
            .map_or((0, 0), |m| (m.envelopes.len(), m.bytes));
        if count >= MAX_ENVELOPES || held + bytes.len() > MAX_MAILBOX_BYTES {
            return Err(ChatError::Refused("mailbox full"));
        }
        self.make_room(held + bytes.len(), bytes.len())?;
        let mailbox = self.mailboxes.entry(to).or_default();
        self.by_size.remove(&(mailbox.bytes, to));
        mailbox.bytes += bytes.len();
        self.by_size.insert((mailbox.bytes, to));
        self.bytes += bytes.len();
        self.seen.insert(id, envelope.expires_at);
        self.expiries.insert((envelope.expires_at, id, to));
        mailbox.envelopes.push_back((id, bytes.len(), envelope));
        Ok(id)
    }

    /// Frees `len` bytes for a mailbox that will then hold `after`, by
    /// evicting the oldest envelopes of mailboxes holding more than that
    /// (ADR-149). Addresses are free, so a flood is many mailboxes; this way
    /// it displaces its own mail before anyone else's, and a deposit is only
    /// refused when no mailbox is larger than the one it joins would become.
    /// An evicted envelope drew no receipt, so its sender's outbox resends it.
    fn make_room(&mut self, after: usize, len: usize) -> Result<(), ChatError> {
        while self.bytes + len > self.capacity {
            match self.by_size.last() {
                Some(&(size, victim)) if size > after => {
                    if let Some((id, _)) = self.remove_envelope(victim, 0) {
                        // Forgotten, so the resend is stored, not taken as a duplicate.
                        self.seen.remove(&id);
                    } else {
                        self.by_size.remove(&(size, victim));
                    }
                }
                _ => return Err(ChatError::Refused("relay full")),
            }
        }
        Ok(())
    }

    /// Takes `at`'s envelope at `index` out of the mailbox, the size and expiry
    /// indexes and the relay's byte count. `seen` is the caller's to settle.
    fn remove_envelope(&mut self, at: Address, index: usize) -> Option<([u8; 32], Envelope)> {
        let mailbox = self.mailboxes.get_mut(&at)?;
        let before = mailbox.bytes;
        let (id, size, envelope) = mailbox.envelopes.remove(index)?;
        mailbox.bytes -= size;
        let (left, empty) = (mailbox.bytes, mailbox.envelopes.is_empty());
        self.bytes -= size;
        self.by_size.remove(&(before, at));
        self.expiries.remove(&(envelope.expires_at, id, at));
        if empty {
            self.mailboxes.remove(&at);
        } else {
            self.by_size.insert((left, at));
        }
        Some((id, envelope))
    }

    /// Drops every expired envelope, id, emptied mailbox and spent challenge.
    /// Each is popped once, so the cost is in what expired, not in what is held.
    fn expire(&mut self, now: u64) {
        while let Some(&(expires_at, id, to)) = self.expiries.first() {
            if expires_at > now {
                break;
            }
            self.seen.remove(&id);
            let at = self
                .mailboxes
                .get(&to)
                .and_then(|m| m.envelopes.iter().position(|(i, _, _)| *i == id));
            if at.and_then(|at| self.remove_envelope(to, at)).is_none() {
                self.expiries.pop_first();
            }
        }
        while let Some(&(expires_at, id)) = self.fetched.first() {
            if expires_at > now {
                break;
            }
            self.fetched.pop_first();
            self.seen.remove(&id);
        }
        while let Some(&(issued, _)) = self.spent.first() {
            if now.saturating_sub(issued) <= CHALLENGE_TTL {
                break;
            }
            self.spent.pop_first();
        }
    }

    /// Bytes held across every mailbox.
    #[must_use]
    pub fn stored_bytes(&self) -> usize {
        self.bytes
    }

    /// Envelope ids remembered, stored or fetched.
    #[must_use]
    pub fn remembered_ids(&self) -> usize {
        self.seen.len()
    }

    /// A fresh challenge for emptying `address`'s mailbox. Nothing is stored
    /// (ADR-149): the nonce is its issue time, a random salt and a keyed tag
    /// over both and the address, so asking costs the relay no memory and a
    /// flood of requests cannot push out an owner's pending challenge.
    ///
    /// # Errors
    ///
    /// [`ChatError::Entropy`].
    pub fn challenge(&mut self, address: Address, now: u64) -> Result<[u8; 32], ChatError> {
        if self.challenge_key.is_none() {
            let mut key = Zeroizing::new([0u8; 32]);
            getrandom::fill(key.as_mut()).map_err(|_| ChatError::Entropy)?;
            self.challenge_key = Some(key);
        }
        let mut nonce = [0u8; 32];
        nonce[..8].copy_from_slice(&now.to_be_bytes());
        getrandom::fill(&mut nonce[8..16]).map_err(|_| ChatError::Entropy)?;
        let tag = self
            .challenge_tag(&address, &nonce)
            .ok_or(ChatError::Entropy)?;
        nonce[16..].copy_from_slice(&tag);
        Ok(nonce)
    }

    /// The tag over `nonce`'s first 16 bytes, if this relay has a key yet.
    fn challenge_tag(&self, address: &Address, nonce: &[u8; 32]) -> Option<[u8; 16]> {
        let mut h = blake3::Hasher::new_keyed(self.challenge_key.as_deref()?);
        h.update(CHALLENGE_DOMAIN.as_bytes());
        h.update(address);
        h.update(&nonce[..16]);
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&h.finalize().as_bytes()[..16]);
        Some(tag)
    }

    /// Empties a mailbox for its owner: `nonce` must be a challenge this relay
    /// issued for `address` within [`CHALLENGE_TTL`] and not yet spent,
    /// `identity_key` the address's key, and `signature` must sign the nonce
    /// at this relay ([`fetch_bytes`] with [`Relay::id`]). A failed attempt
    /// spends nothing, so the owner's challenge stays usable.
    ///
    /// # Errors
    ///
    /// No, a stale or a spent challenge, the wrong key, or a bad signature.
    pub fn fetch(
        &mut self,
        address: Address,
        nonce: &[u8; 32],
        identity_key: &[u8],
        signature: &[u8],
        now: u64,
    ) -> Result<Vec<Envelope>, ChatError> {
        let tag = self
            .challenge_tag(&address, nonce)
            .ok_or(ChatError::Refused("no challenge issued"))?;
        // Constant time, so the tag cannot be guessed a byte at a time.
        if tag
            .iter()
            .zip(&nonce[16..])
            .fold(0, |d, (a, b)| d | (a ^ b))
            != 0
        {
            return Err(ChatError::Refused("no challenge issued"));
        }
        let mut issued = [0u8; 8];
        issued.copy_from_slice(&nonce[..8]);
        let issued = u64::from_be_bytes(issued);
        if issued > now || now - issued > CHALLENGE_TTL {
            return Err(ChatError::Expired);
        }
        self.expire(now);
        if self.spent.contains(&(issued, *nonce)) {
            return Err(ChatError::Refused("challenge already used"));
        }
        if address_of(identity_key) != address {
            return Err(ChatError::BadSignature);
        }
        suite::verify(
            SuiteId::MlDsa65,
            identity_key,
            &fetch_bytes(&self.id, &address, nonce),
            signature,
        )
        .map_err(|_| ChatError::BadSignature)?;
        self.spent.insert((issued, *nonce));
        while self.spent.len() > MAX_SPENT_CHALLENGES {
            self.spent.pop_first();
        }
        let Some(mailbox) = self.mailboxes.remove(&address) else {
            return Ok(Vec::new());
        };
        self.by_size.remove(&(mailbox.bytes, address));
        self.bytes -= mailbox.bytes;
        let mut out = Vec::with_capacity(mailbox.envelopes.len());
        for (id, _, envelope) in mailbox.envelopes {
            self.expiries.remove(&(envelope.expires_at, id, address));
            self.fetched.insert((envelope.expires_at, id));
            out.push(envelope);
        }
        while self.fetched.len() > MAX_FETCHED_IDS {
            if let Some((_, id)) = self.fetched.pop_first() {
                self.seen.remove(&id);
            }
        }
        Ok(out)
    }
}
