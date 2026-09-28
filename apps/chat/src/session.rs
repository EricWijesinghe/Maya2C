//! One-to-one sessions (ADR-031): an authenticated X-Wing handshake, then
//! one symmetric chain per direction. Each message key is used once and
//! deleted, so a key stolen later cannot read what was already received.

use std::collections::BTreeMap;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use maya_crypto_pq::kem_suite::{KemSuite, XWing};
use maya_crypto_pq::suite::{self, SuiteId};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::identity::{Identity, PrekeyBundle, address_of, put};
use crate::{Address, ChatError};

const HANDSHAKE_DOMAIN: &[u8] = b"maya-chat handshake v1";
const ROOT_DOMAIN: &str = "maya-chat 2026-09-28 session root v1";
const CHAIN_DOMAIN: &str = "maya-chat 2026-09-28 chain step v1";
const MESSAGE_DOMAIN: &str = "maya-chat 2026-09-28 message key v1";
/// Most messages a receiver will skip ahead over (out-of-order delivery).
pub const MAX_SKIP: u64 = 1_000;
/// How old a handshake may be when accepted, seconds.
pub const MAX_HANDSHAKE_AGE: u64 = 7 * 24 * 3_600;
/// How far in the future a handshake's clock may be, seconds.
pub const MAX_CLOCK_SKEW: u64 = 300;

type Secret = Zeroizing<[u8; 32]>;

fn derive(domain: &str, parts: &[&[u8]]) -> Secret {
    let mut h = blake3::Hasher::new_derive_key(domain);
    for p in parts {
        h.update(p);
    }
    Zeroizing::new(*h.finalize().as_bytes())
}

/// The first message of a session: the initiator's identity, a
/// ciphertext to the responder's prekey, and the initiator's signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handshake {
    /// The initiator's ML-DSA-65 identity key.
    pub initiator_key: Vec<u8>,
    /// The responder's address.
    pub responder: Address,
    /// Which of the responder's prekeys.
    pub prekey_epoch: u32,
    /// X-Wing ciphertext.
    pub ciphertext: Vec<u8>,
    /// Unix seconds.
    pub sent_at: u64,
    /// The initiator's signature over the fields above.
    pub signature: Vec<u8>,
}

impl Handshake {
    fn signing_bytes(&self) -> Vec<u8> {
        let mut out = HANDSHAKE_DOMAIN.to_vec();
        put(&mut out, &self.initiator_key);
        out.extend_from_slice(&self.responder);
        out.extend_from_slice(&self.prekey_epoch.to_le_bytes());
        put(&mut out, &self.ciphertext);
        out.extend_from_slice(&self.sent_at.to_le_bytes());
        out
    }

    /// The session id: a hash of the whole signed handshake.
    #[must_use]
    pub fn session_id(&self) -> [u8; 32] {
        let mut bytes = self.signing_bytes();
        put(&mut bytes, &self.signature);
        *blake3::hash(&bytes).as_bytes()
    }
}

/// An encrypted message within a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// The session it belongs to.
    pub session: [u8; 32],
    /// Its number in the sender's chain.
    pub n: u64,
    /// ChaCha20-Poly1305 ciphertext and tag.
    pub ciphertext: Vec<u8>,
}

/// One side of a session.
pub struct Session {
    id: [u8; 32],
    peer: Address,
    send_chain: Secret,
    recv_chain: Secret,
    send_n: u64,
    recv_n: u64,
    skipped: BTreeMap<u64, Secret>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("peer", &self.peer)
            .field("send_n", &self.send_n)
            .field("recv_n", &self.recv_n)
            .finish_non_exhaustive()
    }
}

/// Opens a session to the owner of `bundle`: returns the session and the
/// handshake to deliver with the first message.
///
/// # Errors
///
/// An expired or forged bundle, a malformed prekey, or a signing failure.
pub fn initiate(
    me: &Identity,
    bundle: &PrekeyBundle,
    now: u64,
) -> Result<(Session, Handshake), ChatError> {
    let responder = bundle.verify(now)?;
    let (ciphertext, secret) = XWing::encapsulate(&bundle.prekey).map_err(|_| ChatError::BadKey)?;
    let mut handshake = Handshake {
        initiator_key: me.public_key().to_vec(),
        responder,
        prekey_epoch: bundle.epoch,
        ciphertext,
        sent_at: now,
        signature: Vec::new(),
    };
    handshake.signature = me.sign(&handshake.signing_bytes())?;
    let session = Session::new(&handshake, secret.as_bytes(), responder, true);
    Ok((session, handshake))
}

/// Accepts a handshake addressed to `me`; returns the session and the
/// initiator's address.
///
/// # Errors
///
/// Another recipient, a forged or stale handshake, or a malformed ciphertext.
pub fn accept(
    me: &Identity,
    handshake: &Handshake,
    now: u64,
) -> Result<(Session, Address), ChatError> {
    if handshake.responder != me.address() {
        return Err(ChatError::NotForMe);
    }
    if handshake.sent_at > now + MAX_CLOCK_SKEW
        || now.saturating_sub(handshake.sent_at) > MAX_HANDSHAKE_AGE
    {
        return Err(ChatError::Expired);
    }
    suite::verify(
        SuiteId::MlDsa65,
        &handshake.initiator_key,
        &handshake.signing_bytes(),
        &handshake.signature,
    )
    .map_err(|_| ChatError::BadSignature)?;
    let (prekey, _) = me.prekey(handshake.prekey_epoch);
    let secret =
        XWing::decapsulate(&prekey, &handshake.ciphertext).map_err(|_| ChatError::BadKey)?;
    let initiator = address_of(&handshake.initiator_key);
    Ok((
        Session::new(handshake, secret.as_bytes(), initiator, false),
        initiator,
    ))
}

fn aead(
    key: &[u8; 32],
    n: u64,
    session: &[u8; 32],
    data: &[u8],
    encrypt: bool,
) -> Result<Vec<u8>, ChatError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let mut nonce = [0u8; 12];
    nonce[..8].copy_from_slice(&n.to_le_bytes());
    let aad = [&session[..], &n.to_le_bytes()].concat();
    let payload = Payload {
        msg: data,
        aad: &aad,
    };
    let out = if encrypt {
        cipher.encrypt(Nonce::from_slice(&nonce), payload)
    } else {
        cipher.decrypt(Nonce::from_slice(&nonce), payload)
    };
    out.map_err(|_| ChatError::Decrypt)
}

impl Session {
    fn new(handshake: &Handshake, secret: &[u8; 32], peer: Address, initiator: bool) -> Self {
        let id = handshake.session_id();
        let root = derive(ROOT_DOMAIN, &[secret, &id]);
        let forward = derive(CHAIN_DOMAIN, &[root.as_ref(), b"initiator to responder"]);
        let backward = derive(CHAIN_DOMAIN, &[root.as_ref(), b"responder to initiator"]);
        let (send_chain, recv_chain) = if initiator {
            (forward, backward)
        } else {
            (backward, forward)
        };
        Self {
            id,
            peer,
            send_chain,
            recv_chain,
            send_n: 0,
            recv_n: 0,
            skipped: BTreeMap::new(),
        }
    }

    /// The session id.
    #[must_use]
    pub fn id(&self) -> [u8; 32] {
        self.id
    }

    /// The other side's address.
    #[must_use]
    pub fn peer(&self) -> Address {
        self.peer
    }

    /// Encrypts `plaintext` under the next message key, which is then gone.
    ///
    /// # Errors
    ///
    /// [`ChatError::Decrypt`] if the cipher refuses (not expected).
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Message, ChatError> {
        let key = derive(MESSAGE_DOMAIN, &[self.send_chain.as_ref()]);
        let ciphertext = aead(&key, self.send_n, &self.id, plaintext, true)?;
        self.send_chain = derive(CHAIN_DOMAIN, &[self.send_chain.as_ref()]);
        let message = Message {
            session: self.id,
            n: self.send_n,
            ciphertext,
        };
        self.send_n += 1;
        Ok(message)
    }

    /// Decrypts a message: in order, out of order within [`MAX_SKIP`], but
    /// never twice. A message that fails to decrypt changes nothing.
    ///
    /// # Errors
    ///
    /// Another session, a replay, too far ahead, or a forged ciphertext.
    pub fn decrypt(&mut self, message: &Message) -> Result<Vec<u8>, ChatError> {
        if message.session != self.id {
            return Err(ChatError::WrongSession);
        }
        if message.n < self.recv_n {
            let key = self.skipped.get(&message.n).ok_or(ChatError::Replayed)?;
            let plaintext = aead(key, message.n, &self.id, &message.ciphertext, false)?;
            self.skipped.remove(&message.n);
            return Ok(plaintext);
        }
        if message.n - self.recv_n > MAX_SKIP {
            return Err(ChatError::TooFarAhead);
        }
        // Step a copy of the chain; commit only if the message authenticates.
        let mut chain = self.recv_chain.clone();
        let mut skipped = Vec::new();
        for n in self.recv_n..message.n {
            skipped.push((n, derive(MESSAGE_DOMAIN, &[chain.as_ref()])));
            chain = derive(CHAIN_DOMAIN, &[chain.as_ref()]);
        }
        let key = derive(MESSAGE_DOMAIN, &[chain.as_ref()]);
        let plaintext = aead(&key, message.n, &self.id, &message.ciphertext, false)?;
        self.recv_chain = derive(CHAIN_DOMAIN, &[chain.as_ref()]);
        self.recv_n = message.n + 1;
        self.skipped.extend(skipped);
        // Bounded in total, not only per message: the oldest skipped keys
        // go first, and a message that late is refused as a replay.
        while self.skipped.len() > usize::try_from(MAX_SKIP).unwrap_or(usize::MAX) {
            self.skipped.pop_first();
        }
        Ok(plaintext)
    }
}
