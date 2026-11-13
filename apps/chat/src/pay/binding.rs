//! Wallet-signed bindings (ADR-088 amendment 1, review H2).
//!
//! A chat identity holds no spending key. A wallet account `A` signs "chat
//! identity `X` may name `A` as the receiving account for payment requests,
//! on chain `C`, from `issued_at` until `expires_at`". Every request carries
//! its payee's binding, and every receipt its payer's, so money moves only
//! between wallet accounts and the chat only names them.
//!
//! - **Signature:** the wallet's hybrid key (suite `0x30`, ML-DSA-65 and
//!   SLH-DSA-SHA2-128s, the chain's transaction scheme) over [`message`].
//!   The account is the chain address of that key, so the binding is signed
//!   by the account it names and by nothing else.
//! - **Version and order:** `issued_at` is the binding's sequence. A contact
//!   keeps the newest binding it has seen for each chat identity
//!   ([`super::Ledger::observe_binding`]) and refuses an older one, so a newer
//!   binding supersedes every older one wherever it has been seen.
//! - **Revocation:** a [`Kind::Revoke`] binding, signed the same way and sent
//!   to contacts as its own frame, supersedes the bindings before it without
//!   naming a new account: until a newer [`Kind::Bind`] arrives, that contact
//!   refuses requests from the identity.
//! - **Expiry:** at most [`MAX_BINDING_TTL`] after issue, so a binding no
//!   contact heard revoked still ends.

use maya_crypto_pq::suite::{self, SuiteId};
use serde::{Deserialize, Serialize};

use super::ChainTag;
use crate::{Address, ChatError};

/// Marks a session plaintext as a binding sent on its own (a revocation, or
/// a new binding announced ahead of a request).
pub const MAGIC: &[u8] = b"\0maya-chat binding v1\0";
/// Longest a binding may run from issue, seconds. A policy bound on how long
/// a binding nobody heard revoked stays good, not a measurement.
pub const MAX_BINDING_TTL: u64 = 90 * 24 * 3_600;
/// How far ahead of the checker's clock a binding's issue time may be,
/// seconds. A policy bound, not a measurement.
pub const MAX_CLOCK_SKEW: u64 = 600;
/// The wallet's signature suite: the chain's transaction scheme.
pub const SUITE: SuiteId = SuiteId::HybridMlDsa65SlhDsa128s;

/// What the wallet signs before the fields. Not a prefix of any transaction's
/// signing bytes, which start with the node's transaction domain.
const MESSAGE_DOMAIN: &[u8] = b"maya2c wallet chat binding v1\0";
/// The node's account address domain (`crypto::hybrid::address_of`).
/// `crates/node/tests/hybrid_parity_tests.rs` pins its value; the wallet
/// core's binding tests pin this module's use of it.
const ACCOUNT_DOMAIN: &[u8] = b"custom-l1-node.address.v3";
const FIELDS_LEN: usize = 1 + 32 + 32 + 32 + 8 + 8;

/// What a binding does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// The chat identity may name the account.
    Bind,
    /// Every earlier binding of the chat identity is void.
    Revoke,
}

impl Kind {
    fn byte(self) -> u8 {
        match self {
            Self::Bind => 1,
            Self::Revoke => 2,
        }
    }

    fn from_byte(byte: u8) -> Result<Self, ChatError> {
        match byte {
            1 => Ok(Self::Bind),
            2 => Ok(Self::Revoke),
            _ => Err(ChatError::Refused("unknown binding kind")),
        }
    }
}

/// The chain account a wallet public key controls: the node's
/// `blake3(domain ‖ ml_dsa_pk ‖ slh_pk)`.
#[must_use]
pub fn account_of(public_key: &[u8]) -> Address {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ACCOUNT_DOMAIN);
    hasher.update(public_key);
    *hasher.finalize().as_bytes()
}

/// The bytes the wallet signs. The wallet builds the same bytes itself from
/// what it shows its user; it never signs bytes a chat hands it.
#[must_use]
pub fn message(
    kind: Kind,
    chain: &ChainTag,
    chat_id: &Address,
    account: &Address,
    issued_at: u64,
    expires_at: u64,
) -> Vec<u8> {
    let mut out = MESSAGE_DOMAIN.to_vec();
    out.push(kind.byte());
    out.extend_from_slice(chain);
    out.extend_from_slice(chat_id);
    out.extend_from_slice(account);
    out.extend_from_slice(&issued_at.to_le_bytes());
    out.extend_from_slice(&expires_at.to_le_bytes());
    out
}

/// A wallet account's statement about one chat identity. See the module
/// docs for the rules.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    /// Bind or revoke.
    pub kind: Kind,
    /// The chain's genesis id.
    pub chain: ChainTag,
    /// The chat identity's address.
    pub chat_id: Address,
    /// The wallet account: the chain address of `public_key`.
    pub account: Address,
    /// Unix seconds; also the binding's sequence.
    pub issued_at: u64,
    /// Unix seconds after which the binding is void.
    pub expires_at: u64,
    /// The wallet account's hybrid public key.
    #[serde(with = "serde_bytes")]
    pub public_key: Vec<u8>,
    /// The wallet's hybrid signature over [`message`].
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}

impl Binding {
    /// The bytes the signature covers.
    #[must_use]
    pub fn message(&self) -> Vec<u8> {
        message(
            self.kind,
            &self.chain,
            &self.chat_id,
            &self.account,
            self.issued_at,
            self.expires_at,
        )
    }

    /// Checks that the wallet that controls `account` signed this binding of
    /// `chat_id` on `chain`, and that it is not expired or post-dated at
    /// `now`. Either kind passes.
    ///
    /// # Errors
    ///
    /// [`ChatError::BadSignature`] for a forged or misattributed binding,
    /// [`ChatError::Expired`], or [`ChatError::Refused`] for another chat
    /// identity, another chain or a lifetime over the limit.
    pub fn verify(&self, chat_id: &Address, chain: &ChainTag, now: u64) -> Result<(), ChatError> {
        if self.chat_id != *chat_id {
            return Err(ChatError::Refused("binding is for another chat identity"));
        }
        if self.chain != *chain {
            return Err(ChatError::Refused("binding is for another chain"));
        }
        if self.expires_at <= self.issued_at || self.expires_at - self.issued_at > MAX_BINDING_TTL {
            return Err(ChatError::Refused("binding lifetime is over the limit"));
        }
        if now >= self.expires_at || self.issued_at > now.saturating_add(MAX_CLOCK_SKEW) {
            return Err(ChatError::Expired);
        }
        if account_of(&self.public_key) != self.account {
            return Err(ChatError::BadSignature);
        }
        suite::verify(SUITE, &self.public_key, &self.message(), &self.signature)
            .map_err(|_| ChatError::BadSignature)
    }

    /// [`Binding::verify`], and the binding must be a [`Kind::Bind`].
    ///
    /// # Errors
    ///
    /// As [`Binding::verify`], or [`ChatError::Refused`] for a revocation.
    pub fn verify_bind(
        &self,
        chat_id: &Address,
        chain: &ChainTag,
        now: u64,
    ) -> Result<(), ChatError> {
        if self.kind != Kind::Bind {
            return Err(ChatError::Refused("binding is a revocation"));
        }
        self.verify(chat_id, chain, now)
    }

    /// The fields, the key, then the signature. Fixed length.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.message().split_off(MESSAGE_DOMAIN.len());
        out.extend_from_slice(&self.public_key);
        out.extend_from_slice(&self.signature);
        out
    }

    /// Length of [`Binding::encode`].
    #[must_use]
    pub const fn encoded_len() -> usize {
        FIELDS_LEN + SUITE.info().public_key_len + SUITE.info().signature_len
    }

    /// Reads a binding off the front of `rest`. Shape only; the signature
    /// and the rules are [`Binding::verify`]'s.
    ///
    /// # Errors
    ///
    /// [`ChatError::Refused`] for a short or malformed binding.
    pub fn take(rest: &mut &[u8]) -> Result<Self, ChatError> {
        if rest.len() < Self::encoded_len() {
            return Err(ChatError::Refused("malformed binding"));
        }
        let (bytes, tail) = rest.split_at(Self::encoded_len());
        *rest = tail;
        let (fields, keys) = bytes.split_at(FIELDS_LEN);
        let (public_key, signature) = keys.split_at(SUITE.info().public_key_len);
        let word = |at: usize| -> [u8; 32] { fields[at..at + 32].try_into().unwrap_or([0; 32]) };
        let time = |at: usize| -> u64 {
            u64::from_le_bytes(fields[at..at + 8].try_into().unwrap_or([0; 8]))
        };
        Ok(Self {
            kind: Kind::from_byte(fields[0])?,
            chain: word(1),
            chat_id: word(33),
            account: word(65),
            issued_at: time(97),
            expires_at: time(105),
            public_key: public_key.to_vec(),
            signature: signature.to_vec(),
        })
    }

    /// The frame that sends this binding on its own: [`MAGIC`] then
    /// [`Binding::encode`].
    #[must_use]
    pub fn frame(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&self.encode());
        out
    }

    /// Reads a frame from [`Binding::frame`].
    ///
    /// # Errors
    ///
    /// [`ChatError::Refused`] for anything else.
    pub fn parse_frame(plain: &[u8]) -> Result<Self, ChatError> {
        let mut rest = plain
            .strip_prefix(MAGIC)
            .ok_or(ChatError::Refused("not a binding"))?;
        let binding = Self::take(&mut rest)?;
        if !rest.is_empty() {
            return Err(ChatError::Refused("malformed binding"));
        }
        Ok(binding)
    }
}

/// Whether a session plaintext is a binding frame.
#[must_use]
pub fn is_binding(plain: &[u8]) -> bool {
    plain.starts_with(MAGIC)
}

/// A wallet stand-in for tests: signs bindings with a hybrid key from a
/// fixed seed. The chat itself never signs one.
#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod test_wallet {
    use maya_crypto_pq::suite::{HybridMlDsa65SlhDsa128s, MasterSeed, SignatureSuite};

    use std::sync::Mutex;

    use super::{Binding, Kind, account_of, message};
    use crate::Address;
    use crate::pay::ChainTag;

    /// The tests' clock and chain.
    pub(crate) const NOW: u64 = 1_800_000_000;
    pub(crate) const CHAIN: ChainTag = [0xc1; 32];

    type Cache = Mutex<Vec<(([u8; 32], Address), &'static Binding)>>;
    static CACHE: Cache = Mutex::new(Vec::new());

    /// A [`Kind::Bind`] of `chat_id` on [`CHAIN`], live around [`NOW`], from
    /// the wallet with `seed`. Signed once per pair: a hybrid signature is
    /// the slow part of these tests.
    pub(crate) fn bound(seed: [u8; 32], chat_id: Address) -> &'static Binding {
        let mut cache = CACHE.lock().unwrap();
        if let Some((_, b)) = cache.iter().find(|(k, _)| *k == (seed, chat_id)) {
            return b;
        }
        let b: &'static Binding = Box::leak(Box::new(sign(
            seed,
            Kind::Bind,
            CHAIN,
            chat_id,
            NOW - 10,
            NOW + 24 * 3_600,
        )));
        cache.push(((seed, chat_id), b));
        b
    }

    /// A binding of `chat_id` on `chain`, signed by the wallet with `seed`.
    pub(crate) fn sign(
        seed: [u8; 32],
        kind: Kind,
        chain: ChainTag,
        chat_id: Address,
        issued_at: u64,
        expires_at: u64,
    ) -> Binding {
        let key = HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes(seed));
        let public_key = HybridMlDsa65SlhDsa128s::public_key(&key);
        let account = account_of(&public_key);
        let msg = message(kind, &chain, &chat_id, &account, issued_at, expires_at);
        Binding {
            kind,
            chain,
            chat_id,
            account,
            issued_at,
            expires_at,
            signature: HybridMlDsa65SlhDsa128s::sign(&key, &msg).unwrap(),
            public_key,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // a test that fails should panic, and say where
mod tests {
    use super::*;
    use crate::identity::Identity;

    const NOW: u64 = 1_800_000_000;
    const CHAIN: ChainTag = [0xc1; 32];
    const WALLET: [u8; 32] = [9; 32];

    fn chat() -> Address {
        Identity::from_seed([1; 32]).address()
    }

    fn bind() -> Binding {
        test_wallet::sign(WALLET, Kind::Bind, CHAIN, chat(), NOW - 10, NOW + 3_600)
    }

    #[test]
    fn a_valid_binding_verifies_and_round_trips() {
        let b = bind();
        b.verify_bind(&chat(), &CHAIN, NOW).unwrap();
        assert_eq!(Binding::parse_frame(&b.frame()).unwrap(), b);
        let encoded = b.encode();
        assert_eq!(encoded.len(), Binding::encoded_len());
    }

    /// Pins the wallet account and the signed bytes against the wallet core,
    /// which pins the same two values from the node's own key code
    /// (`apps/wallet-gui/core/tests/chat_binding_tests.rs`).
    #[test]
    fn the_account_and_message_match_the_wallet_core_vectors() {
        let b = bind();
        assert_eq!(
            hex::encode(b.account),
            "cee1ccb8ef679d5f4d8904f87232b12d547b657421da73a862cf22030773014d"
        );
        let msg = message(Kind::Bind, &[0xc1; 32], &[1; 32], &[2; 32], 3, 4);
        assert_eq!(
            blake3::hash(&msg).to_hex().as_str(),
            "5facf263fa6fd5ffa709a77c61129505fa814c1294e524c01dc1b5cec34f0d03"
        );
        // The whole signed binding, as the wallet core's node-key signer
        // writes it: both signature halves are deterministic.
        let signed = test_wallet::sign(WALLET, Kind::Bind, [0xc1; 32], [1; 32], 3, 4);
        assert_eq!(
            blake3::hash(&signed.encode()).to_hex().as_str(),
            "3831df4e0e53c276deaff61c1fbfc86d7e7a5960831e9b4d393cd27b65ddebd0"
        );
    }

    #[test]
    fn a_forged_binding_is_refused() {
        let mut b = bind();
        b.signature[0] ^= 1;
        assert_eq!(b.verify(&chat(), &CHAIN, NOW), Err(ChatError::BadSignature));
        // A key that is not the account's: a binding "for" someone else's
        // wallet, signed by the forger's own key.
        let mut stolen = bind();
        stolen.account = [7; 32];
        assert_eq!(
            stolen.verify(&chat(), &CHAIN, NOW),
            Err(ChatError::BadSignature)
        );
        // Every signed field is covered.
        let mut moved = bind();
        moved.expires_at += 1;
        assert_eq!(
            moved.verify(&chat(), &CHAIN, NOW),
            Err(ChatError::BadSignature)
        );
    }

    #[test]
    fn a_binding_for_another_chat_identity_is_refused() {
        let other = Identity::from_seed([3; 32]).address();
        assert_eq!(
            bind().verify(&other, &CHAIN, NOW),
            Err(ChatError::Refused("binding is for another chat identity"))
        );
    }

    #[test]
    fn a_binding_for_another_chain_is_refused() {
        assert_eq!(
            bind().verify(&chat(), &[0xc2; 32], NOW),
            Err(ChatError::Refused("binding is for another chain"))
        );
    }

    #[test]
    fn an_expired_post_dated_or_over_long_binding_is_refused() {
        let b = bind();
        assert_eq!(
            b.verify(&chat(), &CHAIN, NOW + 3_600),
            Err(ChatError::Expired)
        );
        assert_eq!(
            b.verify(&chat(), &CHAIN, NOW - 10 - MAX_CLOCK_SKEW - 1),
            Err(ChatError::Expired)
        );
        let long = test_wallet::sign(
            WALLET,
            Kind::Bind,
            CHAIN,
            chat(),
            NOW,
            NOW + MAX_BINDING_TTL + 1,
        );
        assert_eq!(
            long.verify(&chat(), &CHAIN, NOW),
            Err(ChatError::Refused("binding lifetime is over the limit"))
        );
    }

    #[test]
    fn a_revocation_verifies_but_is_not_a_binding() {
        let revoke = test_wallet::sign(WALLET, Kind::Revoke, CHAIN, chat(), NOW, NOW + 60);
        revoke.verify(&chat(), &CHAIN, NOW).unwrap();
        assert_eq!(
            revoke.verify_bind(&chat(), &CHAIN, NOW),
            Err(ChatError::Refused("binding is a revocation"))
        );
    }

    #[test]
    fn malformed_frames_are_refused() {
        let frame = bind().frame();
        assert!(Binding::parse_frame(&frame[..frame.len() - 1]).is_err());
        let mut long = frame.clone();
        long.push(0);
        assert!(Binding::parse_frame(&long).is_err());
        let mut kind = frame;
        kind[MAGIC.len()] = 3;
        assert!(Binding::parse_frame(&kind).is_err());
    }
}
