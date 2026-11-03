//! A user's side of chat: sessions keyed by peer, turned into envelopes and
//! back, with the sender's outbox and delivery receipts (ADR-074).
//! Transport-agnostic, so the same client runs over an in-process relay in
//! tests and over libp2p in [`crate::net`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::identity::{Identity, PrekeyBundle};
use crate::outbox::{self, Expired, Pending, Standing};
use crate::receipt;
use crate::relay::{Envelope, MAX_ENVELOPE_VERSION, Payload};
use crate::session::{self, MAX_HANDSHAKE_AGE, Session};
use crate::{Address, ChatError};

/// How long an envelope asks relays to keep it, seconds.
pub const ENVELOPE_TTL: u64 = 7 * 24 * 3_600;

/// A received message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Received {
    /// Who sent it.
    pub from: Address,
    /// The plaintext.
    pub text: Vec<u8>,
}

/// What an envelope turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// A message for the user.
    Text(Received),
    /// A receipt: these envelope ids of ours reached `from`'s device. Already
    /// applied to the outbox; the list is for the caller's display.
    Receipt {
        /// Who signed it.
        from: Address,
        /// The ids that left the outbox.
        ids: Vec<[u8; 32]>,
    },
}

const STATE_DOMAIN: &str = "maya-chat 2026-09-28 local state v1";
const STATE_NONCE: usize = 12;
/// The state layout this build writes. Version 0 (absent) is the layout
/// before the outbox, which loads with empty outbox and receipts on.
const STATE_VERSION: u8 = 2;

fn on() -> bool {
    true
}

fn v1() -> u8 {
    1
}

/// What [`Client::save`] keeps: without it every run starts with no
/// sessions, so follow-up messages cannot be read and a replayed handshake
/// is accepted again.
#[derive(Serialize, Deserialize)]
struct State {
    #[serde(default)]
    version: u8,
    sessions: BTreeMap<Address, Session>,
    accepted: BTreeMap<[u8; 32], u64>,
    #[serde(default)]
    outbox: BTreeMap<[u8; 32], Pending>,
    #[serde(default)]
    owed: BTreeMap<Address, Vec<[u8; 32]>>,
    #[serde(default = "on")]
    receipts: bool,
    #[serde(default)]
    receipts_off: BTreeSet<Address>,
    #[serde(default = "v1")]
    send_version: u8,
    #[serde(default)]
    expired: VecDeque<([u8; 32], Expired)>,
}

/// One identity's sessions, outbox and receipt state.
pub struct Client {
    identity: Identity,
    sessions: BTreeMap<Address, Session>,
    /// Handshakes already accepted, by session id, with their send time.
    /// Without this, replaying an old `Open` would reset a live session to
    /// stale keys and deliver its first message again. [`session::accept`] refuses a
    /// handshake older than [`MAX_HANDSHAKE_AGE`], so an id can be forgotten
    /// after that.
    accepted: BTreeMap<[u8; 32], u64>,
    /// Sealed envelopes awaiting a receipt, by envelope id.
    outbox: BTreeMap<[u8; 32], Pending>,
    /// Envelope ids received in the current pass, by sender, to be receipted
    /// in one batch by [`Client::end_pass`].
    owed: BTreeMap<Address, Vec<[u8; 32]>>,
    /// Global switch for sending receipts (ADR-074, decision 3).
    receipts: bool,
    /// Contacts whose receipts are switched off.
    receipts_off: BTreeSet<Address>,
    /// Envelope version for new messages, 1 unless raised.
    send_version: u8,
    /// Messages that expired unreceipted, newest last, at most
    /// [`outbox::MAX_EXPIRED`]. Kept for the user; not counted in the outbox.
    expired: VecDeque<([u8; 32], Expired)>,
}

impl Client {
    /// A client for `identity` with no sessions yet.
    #[must_use]
    pub fn new(identity: Identity) -> Self {
        Self {
            identity,
            sessions: BTreeMap::new(),
            accepted: BTreeMap::new(),
            outbox: BTreeMap::new(),
            owed: BTreeMap::new(),
            receipts: true,
            receipts_off: BTreeSet::new(),
            send_version: v1(),
            expired: VecDeque::new(),
        }
    }

    fn state_key(identity: &Identity) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(blake3::derive_key(STATE_DOMAIN, identity.seed()))
    }

    /// The client's state, encrypted under a key derived from the identity
    /// seed, for storing between runs.
    ///
    /// # Errors
    ///
    /// [`ChatError::Encoding`] or [`ChatError::Entropy`].
    pub fn save(&self) -> Result<Vec<u8>, ChatError> {
        let state = State {
            version: STATE_VERSION,
            sessions: self.sessions.clone(),
            accepted: self.accepted.clone(),
            outbox: self.outbox.clone(),
            owed: self.owed.clone(),
            receipts: self.receipts,
            receipts_off: self.receipts_off.clone(),
            send_version: self.send_version,
            expired: self.expired.clone(),
        };
        let mut plain = Zeroizing::new(Vec::new());
        ciborium::into_writer(&state, &mut *plain).map_err(|_| ChatError::Encoding)?;
        let mut nonce = [0u8; STATE_NONCE];
        getrandom::fill(&mut nonce).map_err(|_| ChatError::Entropy)?;
        let key = Self::state_key(&self.identity);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
        let sealed = cipher
            .encrypt(Nonce::from_slice(&nonce), plain.as_slice())
            .map_err(|_| ChatError::Encoding)?;
        Ok([nonce.as_slice(), &sealed].concat())
    }

    /// A client for `identity` restored from [`Client::save`] output.
    ///
    /// # Errors
    ///
    /// [`ChatError::Decrypt`] if the bytes are not this identity's state, or
    /// [`ChatError::Encoding`] for a state written by a newer build.
    pub fn restore(identity: Identity, saved: &[u8]) -> Result<Self, ChatError> {
        if saved.len() < STATE_NONCE {
            return Err(ChatError::Decrypt);
        }
        let (nonce, sealed) = saved.split_at(STATE_NONCE);
        let key = Self::state_key(&identity);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
        let plain = Zeroizing::new(
            cipher
                .decrypt(Nonce::from_slice(nonce), sealed)
                .map_err(|_| ChatError::Decrypt)?,
        );
        let state: State =
            ciborium::from_reader(plain.as_slice()).map_err(|_| ChatError::Encoding)?;
        if state.version > STATE_VERSION {
            return Err(ChatError::Encoding);
        }
        Ok(Self {
            identity,
            sessions: state.sessions,
            accepted: state.accepted,
            outbox: state.outbox,
            owed: state.owed,
            receipts: state.receipts,
            receipts_off: state.receipts_off,
            send_version: state.send_version,
            expired: state.expired,
        })
    }

    /// The identity.
    #[must_use]
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Seals `text` for `to`. With no session yet, `bundle` (the recipient's
    /// prekey bundle) opens one and the envelope carries the handshake.
    ///
    /// # Errors
    ///
    /// No session and no bundle, a bad bundle, or a crypto failure.
    pub fn seal(
        &mut self,
        to: Address,
        bundle: Option<&PrekeyBundle>,
        text: &[u8],
        now: u64,
    ) -> Result<Envelope, ChatError> {
        self.seal_as(to, bundle, text, now, self.send_version)
    }

    /// [`Client::seal`] at an explicit envelope version.
    pub(crate) fn seal_as(
        &mut self,
        to: Address,
        bundle: Option<&PrekeyBundle>,
        text: &[u8],
        now: u64,
        version: u8,
    ) -> Result<Envelope, ChatError> {
        // Refused before any state moves: receipts share the session's chain.
        if receipt::is_receipt(text) {
            return Err(ChatError::Refused(
                "text cannot start with the receipt marker",
            ));
        }
        let payload = if let Some(session) = self.sessions.get_mut(&to) {
            Payload::Chat(session.encrypt(text)?)
        } else {
            let bundle = bundle.ok_or(ChatError::NoSession)?;
            let (mut session, handshake) = session::initiate(&self.identity, to, bundle, now)?;
            let first = session.encrypt(text)?;
            self.sessions.insert(to, session);
            Payload::Open { handshake, first }
        };
        Ok(Envelope {
            to,
            expires_at: now + ENVELOPE_TTL,
            payload,
            stamp: 0,
            version,
        })
    }

    /// Opens an envelope addressed to this identity. A receipt is an error
    /// here; use [`Client::open_frame`] to see receipts.
    ///
    /// # Errors
    ///
    /// Not for this identity, an unknown session, a message that fails to
    /// authenticate, or [`ChatError::NotText`] for a receipt.
    pub fn open(&mut self, envelope: &Envelope, now: u64) -> Result<Received, ChatError> {
        match self.open_frame(envelope, now)? {
            Frame::Text(received) => Ok(received),
            Frame::Receipt { .. } => Err(ChatError::NotText),
        }
    }

    /// Opens an envelope and says whether it is a message or a receipt.
    /// Received messages join this pass's receipt batch
    /// ([`Client::end_pass`]).
    ///
    /// # Errors
    ///
    /// As [`Client::open`], plus [`ChatError::BadSignature`] for a receipt
    /// that does not verify, which changes nothing.
    pub fn open_frame(&mut self, envelope: &Envelope, now: u64) -> Result<Frame, ChatError> {
        if envelope.to != self.identity.address() {
            return Err(ChatError::NotForMe);
        }
        match &envelope.payload {
            Payload::Open { handshake, first } => {
                let id = handshake.session_id();
                if self.accepted.contains_key(&id) {
                    return Err(ChatError::Replayed);
                }
                let (mut session, from) = session::accept(&self.identity, handshake, now)?;
                let text = session.decrypt(first)?;
                if receipt::is_receipt(&text) {
                    return Err(ChatError::Refused("a receipt cannot open a session"));
                }
                self.accepted
                    .retain(|_, sent| now.saturating_sub(*sent) <= MAX_HANDSHAKE_AGE);
                self.accepted.insert(id, handshake.sent_at);
                // A new handshake from the same peer replaces the old session:
                // that is how a peer re-keys.
                self.sessions.insert(from, session);
                self.note_received(from, envelope.id()?);
                Ok(Frame::Text(Received { from, text }))
            }
            Payload::Chat(message) => {
                let from = self
                    .sessions
                    .iter()
                    .find(|(_, s)| s.id() == message.session)
                    .map(|(peer, _)| *peer)
                    .ok_or(ChatError::NoSession)?;
                let (text, peer_key) = {
                    let session = self.sessions.get_mut(&from).ok_or(ChatError::NoSession)?;
                    (session.decrypt(message)?, session.peer_key().to_vec())
                };
                if receipt::is_receipt(&text) {
                    let ids = self.apply_receipt(&from, &peer_key, &message.session, &text)?;
                    return Ok(Frame::Receipt { from, ids });
                }
                self.note_received(from, envelope.id()?);
                Ok(Frame::Text(Received { from, text }))
            }
        }
    }

    fn note_received(&mut self, from: Address, id: [u8; 32]) {
        if self.receipts_for(&from) {
            self.owed.entry(from).or_default().push(id);
        }
    }

    /// Verifies a receipt, then removes the outbox entries it names. The
    /// signature is checked before the outbox is touched; an id that is not
    /// there, or is for another peer, is ignored.
    fn apply_receipt(
        &mut self,
        from: &Address,
        peer_key: &[u8],
        session: &[u8; 32],
        plain: &[u8],
    ) -> Result<Vec<[u8; 32]>, ChatError> {
        let (ids, signature) = receipt::parse(plain)?;
        receipt::verify(
            peer_key,
            from,
            &self.identity.address(),
            session,
            &ids,
            signature,
        )?;
        let mut removed = Vec::new();
        for id in ids {
            if self
                .outbox
                .get(&id)
                .is_some_and(|p| !p.receipt && p.to == *from)
            {
                self.outbox.remove(&id);
                removed.push(id);
            }
        }
        Ok(removed)
    }

    /// Ends a collection pass: one receipt per sender, carrying every id
    /// received from them in the pass. Queued, not sent; each is due after a
    /// random delay ([`outbox::RECEIPT_DELAY_MAX`]). Every sender is tried even
    /// when one fails; the first error is returned.
    ///
    /// # Errors
    ///
    /// The first receipt that could not be sealed or queued.
    pub fn end_pass(&mut self, now: u64) -> Result<(), ChatError> {
        self.sweep(now);
        let peers: Vec<Address> = self.owed.keys().copied().collect();
        let mut first_error = None;
        for peer in peers {
            let ids = self.owed.remove(&peer).unwrap_or_default();
            for chunk in ids.chunks(receipt::MAX_IDS) {
                if let Err(e) = self.send_receipt(&peer, chunk, now) {
                    first_error.get_or_insert(e);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn send_receipt(
        &mut self,
        peer: &Address,
        ids: &[[u8; 32]],
        now: u64,
    ) -> Result<(), ChatError> {
        let me = self.identity.address();
        let session = self.sessions.get_mut(peer).ok_or(ChatError::NoSession)?;
        let statement = receipt::digest(&session.id(), &me, peer, ids);
        let signature = self.identity.sign(&statement)?;
        let message = session.encrypt(&receipt::encode(ids, &signature))?;
        let envelope = Envelope {
            to: *peer,
            expires_at: now + ENVELOPE_TTL,
            payload: Payload::Chat(message),
            stamp: 0,
            version: self.send_version,
        };
        let size = envelope.encode()?.len();
        let id = envelope.id()?;
        outbox::check_room(self.outbox.len(), self.outbox_bytes(), size)?;
        let next_at = now + outbox::random_upto(outbox::RECEIPT_DELAY_MAX)?;
        self.outbox.insert(
            id,
            Pending {
                to: *peer,
                envelope,
                receipt: true,
                size,
                first_at: now,
                attempts: 0,
                next_at,
                relayed: false,
            },
        );
        Ok(())
    }

    /// Seals `text` for `to` and queues it in the outbox at `version`, due
    /// now. Nothing leaves the client here: the caller saves
    /// [`Client::save`] and then sends with [`crate::courier::flush`]. A
    /// refusal leaves no session behind that the peer never received.
    ///
    /// # Errors
    ///
    /// As [`Client::seal`], or [`ChatError::OutboxFull`] or a size refusal.
    pub fn queue(
        &mut self,
        to: Address,
        bundle: Option<&PrekeyBundle>,
        text: &[u8],
        now: u64,
        version: u8,
    ) -> Result<[u8; 32], ChatError> {
        self.sweep(now);
        let had_session = self.sessions.contains_key(&to);
        let envelope = self.seal_as(to, bundle, text, now, version)?;
        let size = envelope.encode()?.len();
        let id = envelope.id()?;
        if let Err(e) = outbox::check_room(self.outbox.len(), self.outbox_bytes(), size) {
            if !had_session {
                self.sessions.remove(&to);
            }
            return Err(e);
        }
        self.outbox.insert(
            id,
            Pending {
                to,
                envelope,
                receipt: false,
                size,
                first_at: now,
                attempts: 0,
                next_at: now,
                relayed: false,
            },
        );
        Ok(id)
    }

    fn outbox_bytes(&self) -> usize {
        self.outbox.values().map(|p| p.size).sum()
    }

    /// The outbox entries due for an attempt at `now`: not expired, and past
    /// their next attempt time.
    #[must_use]
    pub fn due(&self, now: u64) -> Vec<[u8; 32]> {
        self.outbox
            .iter()
            .filter(|(_, p)| p.next_at <= now && now < p.first_at + ENVELOPE_TTL)
            .map(|(id, _)| *id)
            .collect()
    }

    /// The envelope to deposit for `id` at `bits` of postage. Re-minted only
    /// if the current stamp is short, so a retry sends the same bytes.
    ///
    /// # Errors
    ///
    /// Not in the outbox, or a minting failure.
    pub fn envelope_for_send(&mut self, id: &[u8; 32], bits: u32) -> Result<Envelope, ChatError> {
        let entry = self
            .outbox
            .get_mut(id)
            .ok_or(ChatError::Refused("not in the outbox"))?;
        if !entry.envelope.stamped(bits)? {
            entry.envelope = entry.envelope.clone().mint(bits)?;
        }
        Ok(entry.envelope.clone())
    }

    /// The relay accepted `id`. A chat message stays queued for its receipt;
    /// a receipt is done and leaves the outbox.
    ///
    /// # Errors
    ///
    /// Entropy for the next retry's jitter.
    pub fn sent(&mut self, id: &[u8; 32], now: u64) -> Result<(), ChatError> {
        let Some(entry) = self.outbox.get(id) else {
            return Ok(());
        };
        if entry.receipt {
            self.outbox.remove(id);
            return Ok(());
        }
        self.reschedule(id, now, true)
    }

    /// An attempt for `id` failed; the next one is scheduled with backoff.
    ///
    /// # Errors
    ///
    /// Entropy for the next retry's jitter.
    pub fn failed(&mut self, id: &[u8; 32], now: u64) -> Result<(), ChatError> {
        self.reschedule(id, now, false)
    }

    fn reschedule(&mut self, id: &[u8; 32], now: u64, accepted: bool) -> Result<(), ChatError> {
        let jitter = outbox::jitter()?;
        if let Some(entry) = self.outbox.get_mut(id) {
            entry.attempts = entry.attempts.saturating_add(1);
            entry.relayed |= accepted;
            entry.next_at = now + outbox::backoff(entry.attempts, jitter);
        }
        Ok(())
    }

    /// The outbox, for display: each entry's id and state.
    pub fn outbox(&self) -> impl Iterator<Item = (&[u8; 32], &Pending)> {
        self.outbox.iter()
    }

    /// The entry for `id`, if still queued.
    #[must_use]
    pub fn pending(&self, id: &[u8; 32]) -> Option<&Pending> {
        self.outbox.get(id)
    }

    /// Where `id` stands at `now`. `None` once it has left both the outbox and
    /// the expired record: a receipt removed it, or the record dropped it.
    /// Telling those apart takes the send time (see [`ENVELOPE_TTL`]).
    #[must_use]
    pub fn standing(&self, id: &[u8; 32], now: u64) -> Option<Standing> {
        match self.outbox.get(id) {
            Some(p) if now < p.first_at + ENVELOPE_TTL => {
                Some(Standing::Queued { relayed: p.relayed })
            }
            Some(_) => Some(Standing::Expired),
            None => self
                .expired
                .iter()
                .any(|(known, _)| known == id)
                .then_some(Standing::Expired),
        }
    }

    /// Moves every entry past its TTL out of the outbox into the expired
    /// record, which keeps the newest [`outbox::MAX_EXPIRED`]. Runs before
    /// every queue and at each send pass, so the outbox bound counts only
    /// pending messages. Expired receipts are dropped: no user sees them.
    pub fn sweep(&mut self, now: u64) {
        let mut stale: Vec<[u8; 32]> = self
            .outbox
            .iter()
            .filter(|(_, p)| now >= p.first_at + ENVELOPE_TTL)
            .map(|(id, _)| *id)
            .collect();
        // Oldest first, so the record keeps the newest when it overflows.
        stale.sort_by_key(|id| (self.outbox[id].first_at, *id));
        for id in stale {
            let Some(entry) = self.outbox.remove(&id) else {
                continue;
            };
            if entry.receipt {
                continue;
            }
            self.expired.push_back((
                id,
                Expired {
                    to: entry.to,
                    first_at: entry.first_at,
                },
            ));
            while self.expired.len() > outbox::MAX_EXPIRED {
                self.expired.pop_front();
            }
        }
    }

    /// Messages that expired unreceipted, oldest first, for the user to see.
    pub fn expired_entries(&self) -> impl Iterator<Item = (&[u8; 32], &Expired)> {
        self.expired.iter().map(|(id, e)| (id, e))
    }

    /// Removes `id` from the outbox or the expired record, once the user has
    /// seen it. Returns whether anything was removed.
    pub fn dismiss(&mut self, id: &[u8; 32]) -> bool {
        let before = self.expired.len();
        self.expired.retain(|(known, _)| known != id);
        let dropped = self.expired.len() != before;
        self.outbox.remove(id).is_some() || dropped
    }

    /// Turns receipts on or off for everyone (ADR-074, decision 3). Off still
    /// receives every message.
    pub fn set_receipts(&mut self, on: bool) {
        self.receipts = on;
    }

    /// Turns receipts on or off for one contact.
    pub fn set_contact_receipts(&mut self, peer: Address, on: bool) {
        if on {
            self.receipts_off.remove(&peer);
        } else {
            self.receipts_off.insert(peer);
        }
    }

    fn receipts_for(&self, peer: &Address) -> bool {
        self.receipts && !self.receipts_off.contains(peer)
    }

    /// The envelope version new messages are sealed at.
    #[must_use]
    pub fn send_version(&self) -> u8 {
        self.send_version
    }

    /// Sets the envelope version for new messages (ADR-074, decision 4). Each
    /// message is sent at the lower of this and the relay's maximum.
    ///
    /// # Errors
    ///
    /// [`ChatError::Refused`] for a version this build does not write.
    pub fn set_send_version(&mut self, version: u8) -> Result<(), ChatError> {
        if !(1..=MAX_ENVELOPE_VERSION).contains(&version) {
            return Err(ChatError::Refused("unknown envelope version"));
        }
        self.send_version = version;
        Ok(())
    }

    /// Whether a session with `peer` exists.
    #[must_use]
    pub fn has_session(&self, peer: &Address) -> bool {
        self.sessions.contains_key(peer)
    }
}
