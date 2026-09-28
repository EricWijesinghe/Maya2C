//! A user's side of chat: sessions keyed by peer, turned into envelopes and
//! back. Transport-agnostic, so the same client runs over an in-process
//! relay in tests and over libp2p in [`crate::net`].

use std::collections::BTreeMap;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::identity::{Identity, PrekeyBundle};
use crate::relay::{Envelope, Payload};
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

const STATE_DOMAIN: &str = "maya-chat 2026-09-28 local state v1";
const STATE_NONCE: usize = 12;

/// What [`Client::save`] keeps: without it every run starts with no
/// sessions, so follow-up messages cannot be read and a replayed handshake
/// is accepted again.
#[derive(Serialize, Deserialize)]
struct State {
    sessions: BTreeMap<Address, Session>,
    accepted: BTreeMap<[u8; 32], u64>,
}

/// One identity's sessions.
pub struct Client {
    identity: Identity,
    sessions: BTreeMap<Address, Session>,
    /// Handshakes already accepted, by session id, with their send time.
    /// Without this, replaying an old `Open` would reset a live session to
    /// stale keys and deliver its first message again. [`session::accept`] refuses a
    /// handshake older than [`MAX_HANDSHAKE_AGE`], so an id can be forgotten
    /// after that.
    accepted: BTreeMap<[u8; 32], u64>,
}

impl Client {
    /// A client for `identity` with no sessions yet.
    #[must_use]
    pub fn new(identity: Identity) -> Self {
        Self {
            identity,
            sessions: BTreeMap::new(),
            accepted: BTreeMap::new(),
        }
    }

    fn state_key(identity: &Identity) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(blake3::derive_key(STATE_DOMAIN, identity.seed()))
    }

    /// The client's sessions and replay record, encrypted under a key
    /// derived from the identity seed, for storing between runs.
    ///
    /// # Errors
    ///
    /// [`ChatError::Encoding`] or [`ChatError::Entropy`].
    pub fn save(&self) -> Result<Vec<u8>, ChatError> {
        let state = State {
            sessions: self.sessions.clone(),
            accepted: self.accepted.clone(),
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
    /// [`ChatError::Decrypt`] if the bytes are not this identity's state.
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
        Ok(Self {
            identity,
            sessions: state.sessions,
            accepted: state.accepted,
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
        })
    }

    /// Opens an envelope addressed to this identity.
    ///
    /// # Errors
    ///
    /// Not for this identity, an unknown session, or a message that fails
    /// to authenticate.
    pub fn open(&mut self, envelope: &Envelope, now: u64) -> Result<Received, ChatError> {
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
                self.accepted
                    .retain(|_, sent| now.saturating_sub(*sent) <= MAX_HANDSHAKE_AGE);
                self.accepted.insert(id, handshake.sent_at);
                // A new handshake from the same peer replaces the old session:
                // that is how a peer re-keys.
                self.sessions.insert(from, session);
                Ok(Received { from, text })
            }
            Payload::Chat(message) => {
                let (from, session) = self
                    .sessions
                    .iter_mut()
                    .find(|(_, s)| s.id() == message.session)
                    .ok_or(ChatError::NoSession)?;
                let text = session.decrypt(message)?;
                Ok(Received { from: *from, text })
            }
        }
    }

    /// Whether a session with `peer` exists.
    #[must_use]
    pub fn has_session(&self, peer: &Address) -> bool {
        self.sessions.contains_key(peer)
    }
}
