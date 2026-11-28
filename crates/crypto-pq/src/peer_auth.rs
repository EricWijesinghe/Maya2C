//! Post-quantum peer authentication by static ML-KEM-1024 keys (ADR-152).
//!
//! Sans-IO: four messages, each a byte string the caller carries over a
//! channel that is already encrypted (inside the Noise session and the
//! ephemeral ML-KEM stage). Nothing here reads a clock, a socket or global
//! state, so the whole exchange is tested in memory.
//!
//! ```text
//! responder → initiator   m0 = ek_R
//! initiator → responder   m1 = ek_I ‖ Encaps(ek_R)
//! responder → initiator   m2 = Encaps(ek_I) ‖ confirm_R
//! initiator → responder   m3 = confirm_I
//! ```
//!
//! Authentication is implicit through decapsulation, then made explicit by
//! key confirmation. Only the holder of a static decapsulation key derives the
//! secret encapsulated to it, so only that holder can produce its side's
//! confirmation tag. A peer that does not hold the key it presented fails at
//! `m2` or `m3`, before any application byte is sent. No signature is made, so
//! a transcript is no transferable proof of who talked to whom.
//!
//! `binding` ties the exchange to the channel it runs in: the caller passes a
//! digest of what that channel already authenticated (both `PeerId`s and the
//! ephemeral stage's transcript), so the messages cannot be relayed into
//! another session.

use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

use crate::kem_suite::{KemSuite, KemSuiteError, MlKem1024};

/// Length of a static encapsulation key and of a ciphertext (ML-KEM-1024).
pub const KEY_LEN: usize = <MlKem1024 as KemSuite>::ENCAPSULATION_KEY_LEN;
/// Length of a ciphertext.
pub const CIPHERTEXT_LEN: usize = <MlKem1024 as KemSuite>::CIPHERTEXT_LEN;
/// Length of a confirmation tag.
pub const TAG_LEN: usize = 32;

const ID_DOMAIN: &str = "maya2c p2p pq identity v1";
const SEED_DOMAIN: &str = "maya2c p2p pq identity seed v1";
const KDF_DOMAIN: &str = "maya2c 2026-11-24 p2p pq peer auth v1";

/// A peer's post-quantum identity: `BLAKE3-derive-key(ID_DOMAIN, ek)`.
pub type PqId = [u8; 32];

/// Why an exchange was refused. Every variant closes the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PeerAuthError {
    /// A message of the wrong length.
    #[error("peer-auth message {0} has the wrong length")]
    Length(u8),
    /// The peer's static key is not the one pinned for it.
    #[error("the peer's static key does not match its pin")]
    PinMismatch,
    /// The peer's confirmation tag is wrong: it does not hold its key.
    #[error("the peer did not prove possession of its static key")]
    NotAuthenticated,
    /// ML-KEM refused a key or a ciphertext, or entropy failed.
    #[error("ML-KEM: {0}")]
    Kem(#[from] KemSuiteError),
}

/// The identity of `ek`.
#[must_use]
pub fn pq_id(ek: &[u8]) -> PqId {
    blake3::derive_key(ID_DOMAIN, ek)
}

/// A node's static ML-KEM-1024 identity. The decapsulation key wipes itself.
pub struct StaticIdentity {
    dk: <MlKem1024 as KemSuite>::DecapsulationKey,
    ek: Vec<u8>,
}

impl core::fmt::Debug for StaticIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "StaticIdentity({}…)", hex_prefix(&self.pq_id()))
    }
}

fn hex_prefix(id: &PqId) -> String {
    format!("{:08x}", u32::from_be_bytes([id[0], id[1], id[2], id[3]]))
}

impl StaticIdentity {
    /// The identity a 32-byte seed derives, the same on every start.
    #[must_use]
    pub fn from_seed(seed: &Zeroizing<[u8; 32]>) -> Self {
        let mut expanded = Zeroizing::new([0u8; 64]);
        blake3::Hasher::new_derive_key(SEED_DOMAIN)
            .update(seed.as_slice())
            .finalize_xof()
            .fill(expanded.as_mut_slice());
        let (dk, ek) = MlKem1024::key_from_seed(&expanded);
        Self { dk, ek }
    }

    /// The public half, sent in the handshake.
    #[must_use]
    pub fn encapsulation_key(&self) -> &[u8] {
        &self.ek
    }

    /// This identity's [`PqId`], what other nodes pin.
    #[must_use]
    pub fn pq_id(&self) -> PqId {
        pq_id(&self.ek)
    }
}

/// The result of a completed exchange: who the peer is and the keys both
/// sides now share. The keys wipe themselves.
pub struct Authenticated {
    /// The peer's proven identity.
    pub peer: PqId,
    /// Key for frames the initiator sends.
    pub initiator_to_responder: Zeroizing<[u8; 32]>,
    /// Key for frames the responder sends.
    pub responder_to_initiator: Zeroizing<[u8; 32]>,
}

impl core::fmt::Debug for Authenticated {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Authenticated({}…)", hex_prefix(&self.peer))
    }
}

struct Derived {
    i2r: Zeroizing<[u8; 32]>,
    r2i: Zeroizing<[u8; 32]>,
    confirm: Zeroizing<[u8; 32]>,
}

/// Session and confirmation keys from both secrets and the whole transcript.
fn derive(
    binding: &[u8; 32],
    ek_r: &[u8],
    ek_i: &[u8],
    ct_r: &[u8],
    ct_i: &[u8],
    ss_r: &[u8; 32],
    ss_i: &[u8; 32],
) -> Derived {
    let mut h = blake3::Hasher::new_derive_key(KDF_DOMAIN);
    for part in [binding.as_slice(), ek_r, ek_i, ct_r, ct_i, ss_r, ss_i] {
        h.update(part);
    }
    let mut okm = Zeroizing::new([0u8; 96]);
    h.finalize_xof().fill(okm.as_mut_slice());
    let take = |i: usize| {
        let mut k = Zeroizing::new([0u8; 32]);
        k.copy_from_slice(&okm[i * 32..(i + 1) * 32]);
        k
    };
    Derived {
        i2r: take(0),
        r2i: take(1),
        confirm: take(2),
    }
}

fn tag(confirm: &[u8; 32], role: &[u8]) -> [u8; TAG_LEN] {
    *blake3::keyed_hash(confirm, role).as_bytes()
}

const RESPONDER: &[u8] = b"responder confirms";
const INITIATOR: &[u8] = b"initiator confirms";

fn check_pin(ek: &[u8], pin: Option<&PqId>) -> Result<PqId, PeerAuthError> {
    let id = pq_id(ek);
    match pin {
        Some(pin) if !bool::from(id.ct_eq(pin)) => Err(PeerAuthError::PinMismatch),
        _ => Ok(id),
    }
}

/// The responder before `m1`.
pub struct Responder<'a> {
    me: &'a StaticIdentity,
}

/// The responder after sending `m2`, waiting for `m3`.
pub struct ResponderConfirm {
    peer: PqId,
    keys: Derived,
}

/// The initiator after sending `m1`, waiting for `m2`.
pub struct InitiatorWait<'a> {
    me: &'a StaticIdentity,
    binding: [u8; 32],
    peer: PqId,
    ek_r: Vec<u8>,
    ct_r: Vec<u8>,
    ss_r: Zeroizing<[u8; 32]>,
}

impl<'a> Responder<'a> {
    /// Starts an exchange; returns `m0`.
    #[must_use]
    pub fn start(me: &'a StaticIdentity) -> (Self, Vec<u8>) {
        (Self { me }, me.ek.clone())
    }

    /// Takes `m1`, checks the initiator against `pin` when there is one, and
    /// returns `m2`.
    ///
    /// # Errors
    ///
    /// A malformed message, a pin mismatch, or an ML-KEM refusal.
    pub fn respond(
        self,
        binding: &[u8; 32],
        pin: Option<&PqId>,
        m1: &[u8],
    ) -> Result<(ResponderConfirm, Vec<u8>), PeerAuthError> {
        if m1.len() != KEY_LEN + CIPHERTEXT_LEN {
            return Err(PeerAuthError::Length(1));
        }
        let (ek_i, ct_r) = m1.split_at(KEY_LEN);
        let peer = check_pin(ek_i, pin)?;
        let ss_r = MlKem1024::decapsulate(&self.me.dk, ct_r)?;
        let (ct_i, ss_i) = MlKem1024::encapsulate(ek_i)?;
        let keys = derive(
            binding,
            &self.me.ek,
            ek_i,
            ct_r,
            &ct_i,
            ss_r.as_bytes(),
            ss_i.as_bytes(),
        );
        let mut m2 = ct_i;
        m2.extend_from_slice(&tag(&keys.confirm, RESPONDER));
        Ok((ResponderConfirm { peer, keys }, m2))
    }
}

impl ResponderConfirm {
    /// Takes `m3`; the exchange is complete only if it confirms.
    ///
    /// # Errors
    ///
    /// [`PeerAuthError::NotAuthenticated`] when the initiator could not
    /// decapsulate under the key it presented.
    pub fn finish(self, m3: &[u8]) -> Result<Authenticated, PeerAuthError> {
        if m3.len() != TAG_LEN {
            return Err(PeerAuthError::Length(3));
        }
        if !bool::from(tag(&self.keys.confirm, INITIATOR).ct_eq(m3)) {
            return Err(PeerAuthError::NotAuthenticated);
        }
        Ok(Authenticated {
            peer: self.peer,
            initiator_to_responder: self.keys.i2r,
            responder_to_initiator: self.keys.r2i,
        })
    }
}

/// Takes `m0` as the initiator, checks the responder against `pin` when there
/// is one, and returns `m1`.
///
/// # Errors
///
/// A malformed message, a pin mismatch, or an ML-KEM refusal.
pub fn initiate<'a>(
    me: &'a StaticIdentity,
    binding: &[u8; 32],
    pin: Option<&PqId>,
    m0: &[u8],
) -> Result<(InitiatorWait<'a>, Vec<u8>), PeerAuthError> {
    if m0.len() != KEY_LEN {
        return Err(PeerAuthError::Length(0));
    }
    let peer = check_pin(m0, pin)?;
    let (ct_r, ss_r) = MlKem1024::encapsulate(m0)?;
    let mut m1 = me.ek.clone();
    m1.extend_from_slice(&ct_r);
    let ss = Zeroizing::new(*ss_r.as_bytes());
    Ok((
        InitiatorWait {
            me,
            binding: *binding,
            peer,
            ek_r: m0.to_vec(),
            ct_r,
            ss_r: ss,
        },
        m1,
    ))
}

impl InitiatorWait<'_> {
    /// Takes `m2`; returns the keys and `m3`.
    ///
    /// # Errors
    ///
    /// [`PeerAuthError::NotAuthenticated`] when the responder could not
    /// decapsulate under the key it presented.
    pub fn finish(self, m2: &[u8]) -> Result<(Authenticated, Vec<u8>), PeerAuthError> {
        if m2.len() != CIPHERTEXT_LEN + TAG_LEN {
            return Err(PeerAuthError::Length(2));
        }
        let (ct_i, confirm_r) = m2.split_at(CIPHERTEXT_LEN);
        let ss_i = MlKem1024::decapsulate(&self.me.dk, ct_i)?;
        let keys = derive(
            &self.binding,
            &self.ek_r,
            &self.me.ek,
            &self.ct_r,
            ct_i,
            &self.ss_r,
            ss_i.as_bytes(),
        );
        if !bool::from(tag(&keys.confirm, RESPONDER).ct_eq(confirm_r)) {
            return Err(PeerAuthError::NotAuthenticated);
        }
        let m3 = tag(&keys.confirm, INITIATOR).to_vec();
        Ok((
            Authenticated {
                peer: self.peer,
                initiator_to_responder: keys.i2r,
                responder_to_initiator: keys.r2i,
            },
            m3,
        ))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const BINDING: [u8; 32] = [9; 32];

    fn identity(byte: u8) -> StaticIdentity {
        StaticIdentity::from_seed(&Zeroizing::new([byte; 32]))
    }

    /// Runs a whole honest exchange with the given pins.
    fn run(
        i: &StaticIdentity,
        r: &StaticIdentity,
        pin_r: Option<&PqId>,
        pin_i: Option<&PqId>,
    ) -> Result<(Authenticated, Authenticated), PeerAuthError> {
        let (responder, m0) = Responder::start(r);
        let (wait, m1) = initiate(i, &BINDING, pin_r, &m0)?;
        let (confirm, m2) = responder.respond(&BINDING, pin_i, &m1)?;
        let (at_i, m3) = wait.finish(&m2)?;
        let at_r = confirm.finish(&m3)?;
        Ok((at_i, at_r))
    }

    #[test]
    fn a_seed_always_derives_the_same_identity() {
        assert_eq!(identity(1).pq_id(), identity(1).pq_id());
        assert_ne!(identity(1).pq_id(), identity(2).pq_id());
        assert_eq!(identity(1).encapsulation_key().len(), KEY_LEN);
    }

    #[test]
    fn pinned_peers_authenticate_each_other_and_share_keys() {
        let (i, r) = (identity(1), identity(2));
        let (at_i, at_r) = run(&i, &r, Some(&r.pq_id()), Some(&i.pq_id())).unwrap();
        assert_eq!(at_i.peer, r.pq_id());
        assert_eq!(at_r.peer, i.pq_id());
        assert_eq!(*at_i.initiator_to_responder, *at_r.initiator_to_responder);
        assert_eq!(*at_i.responder_to_initiator, *at_r.responder_to_initiator);
        assert_ne!(*at_i.initiator_to_responder, *at_i.responder_to_initiator);
    }

    #[test]
    fn a_peer_presenting_someone_elses_key_is_refused_at_confirmation() {
        // The impersonator sends the victim's public key (so the pin matches)
        // but holds only its own decapsulation key.
        let (i, victim, mallory) = (identity(1), identity(2), identity(3));
        let (_, m0) = Responder::start(&victim);
        let (wait, m1) = initiate(&i, &BINDING, Some(&victim.pq_id()), &m0).unwrap();
        let forged = Responder { me: &mallory }.respond(&BINDING, None, &m1);
        // Mallory cannot decapsulate the ciphertext meant for the victim's key,
        // so the secret it mixes in is wrong and its tag does not verify.
        let (_, m2) = forged.unwrap();
        assert!(matches!(
            wait.finish(&m2),
            Err(PeerAuthError::NotAuthenticated)
        ));
    }

    #[test]
    fn an_initiator_that_does_not_hold_its_key_is_refused() {
        let (victim, r, mallory) = (identity(1), identity(2), identity(3));
        let (responder, m0) = Responder::start(&r);
        let (_, mut m1) = initiate(&mallory, &BINDING, None, &m0).unwrap();
        // Claim the victim's key in m1 (pins on the responder match it).
        m1[..KEY_LEN].copy_from_slice(victim.encapsulation_key());
        let (confirm, m2) = responder
            .respond(&BINDING, Some(&victim.pq_id()), &m1)
            .unwrap();
        // Mallory cannot decapsulate m2's ciphertext (sent to the victim), so
        // the best it can return is a guess.
        let _ = m2;
        assert!(matches!(
            confirm.finish(&[0u8; TAG_LEN]),
            Err(PeerAuthError::NotAuthenticated)
        ));
    }

    #[test]
    fn a_key_that_is_not_the_pinned_one_is_refused_before_any_secret() {
        let (i, r, other) = (identity(1), identity(2), identity(4));
        assert!(matches!(
            run(&i, &r, Some(&other.pq_id()), None),
            Err(PeerAuthError::PinMismatch)
        ));
        assert!(matches!(
            run(&i, &r, None, Some(&other.pq_id())),
            Err(PeerAuthError::PinMismatch)
        ));
    }

    #[test]
    fn a_tampered_ciphertext_or_another_binding_fails_confirmation() {
        let (i, r) = (identity(1), identity(2));
        let (responder, m0) = Responder::start(&r);
        let (wait, m1) = initiate(&i, &BINDING, None, &m0).unwrap();
        let (_, mut m2) = responder.respond(&BINDING, None, &m1).unwrap();
        m2[0] ^= 1;
        assert!(matches!(
            wait.finish(&m2),
            Err(PeerAuthError::NotAuthenticated)
        ));

        // The same messages relayed into a session with another binding.
        let (responder, m0) = Responder::start(&r);
        let (wait, m1) = initiate(&i, &BINDING, None, &m0).unwrap();
        let (_, m2) = responder.respond(&[8; 32], None, &m1).unwrap();
        assert!(matches!(
            wait.finish(&m2),
            Err(PeerAuthError::NotAuthenticated)
        ));
    }

    #[test]
    fn wrong_length_messages_are_refused() {
        let (i, r) = (identity(1), identity(2));
        assert!(matches!(
            initiate(&i, &BINDING, None, &[0; KEY_LEN - 1]),
            Err(PeerAuthError::Length(0))
        ));
        let (responder, _) = Responder::start(&r);
        assert!(matches!(
            responder.respond(&BINDING, None, &[0; 3]),
            Err(PeerAuthError::Length(1))
        ));
    }
}
