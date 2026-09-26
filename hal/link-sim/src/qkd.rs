//! QKD key supply and how it may touch a handshake (Master Prompt 7 §4).
//!
//! Two rules, both enforced by types rather than by comment:
//!
//! 1. **QKD key is mixed into the post-quantum handshake, never a
//!    replacement for it.** [`mix`] takes the ML-KEM secret unconditionally
//!    and the QKD key optionally; there is no function that derives a session
//!    key from QKD alone. A QBER at or above 11% drops the QKD input and the
//!    session continues on ML-KEM.
//! 2. **Entanglement carries no information by itself** (the no-communication
//!    theorem). Measurements on entangled pairs give correlated but random
//!    bits; turning them into a shared key needs basis reconciliation over a
//!    classical channel, which is limited by light speed like any other
//!    message. So [`EntangledPairs::sift`] *requires* the classical
//!    transcript, and a key cannot be produced without one — which is what
//!    `entanglement_keys_require_classical_channel` tests.
//!
//! The ETSI GS QKD 014 key-delivery interface is a REST API that hands over
//! keys by ID; a client for it is transport plumbing and lives with the node's
//! network code when a device exists. Nothing here talks to hardware: SIM.

/// Quantum bit error rate at or above which a QKD key is discarded, ppm.
/// 11% is the BB84 threshold above which no secret key can be distilled
/// against a general attack.
pub const QBER_ABORT_PPM: u32 = 110_000;

/// Key material from a QKD link, with the error rate observed producing it.
#[derive(Clone, Debug)]
pub struct QkdKey {
    /// Key bytes.
    pub key: [u8; 32],
    /// Observed QBER, ppm.
    pub qber_ppm: u32,
}

/// Session secret from the PQ handshake, with QKD key mixed in only when it
/// is healthy. Returns the secret and whether QKD contributed.
pub fn mix(pq_secret: &[u8; 32], qkd: Option<&QkdKey>) -> ([u8; 32], bool) {
    let healthy = qkd.filter(|k| k.qber_ppm < QBER_ABORT_PPM);
    let mut h = blake3::Hasher::new_derive_key("maya2c/pq-handshake/qkd-mix/v1");
    h.update(pq_secret);
    match healthy {
        Some(k) => {
            h.update(&[1]);
            h.update(&k.key);
        }
        None => {
            h.update(&[0]);
        }
    }
    (*h.finalize().as_bytes(), healthy.is_some())
}

/// Raw measurement outcomes on one side of an entanglement source: a basis
/// choice and a bit per pair. Correlated with the other side's bits only
/// where the bases matched, and nobody knows where that is yet.
#[derive(Clone, Debug)]
pub struct EntangledPairs {
    /// Basis chosen per pair.
    pub bases: Vec<bool>,
    /// Outcome per pair.
    pub bits: Vec<bool>,
}

/// The classical messages basis reconciliation needs: the peer's basis list.
/// Can only be obtained by receiving it over a classical link.
#[derive(Clone, Debug)]
pub struct ClassicalTranscript {
    /// Peer's bases, as announced.
    pub peer_bases: Vec<bool>,
}

impl EntangledPairs {
    /// Keeps the bits where both sides measured in the same basis. Needs the
    /// peer's announced bases: without the classical message there is no
    /// way to know which bits are correlated, so there is no key.
    pub fn sift(&self, transcript: &ClassicalTranscript) -> Vec<bool> {
        self.bases
            .iter()
            .zip(&transcript.peer_bases)
            .zip(&self.bits)
            .filter(|((a, b), _)| a == b)
            .map(|(_, bit)| *bit)
            .collect()
    }
}
