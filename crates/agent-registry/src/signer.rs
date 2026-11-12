//! The agent's key: hybrid ML-DSA-65 + SLH-DSA-128s, the suite the node
//! already verifies. Nothing here logs key material; the key type zeroizes on
//! drop.
//!
//! The agent key is *derived* from the master seed under its own domain, never
//! the seed itself: `HybridMlDsa65SlhDsa128s::signing_key_from_seed` treats the
//! seed as the account's chain key, so using it directly would make every card,
//! quote and receipt a signature by the key that spends the account. An agent
//! key that leaks from a hot service then cannot move funds.

use maya_crypto_pq::suite::{
    HybridMlDsa65SlhDsa128s, HybridSigningKey, MasterSeed, SignatureSuite,
};
use zeroize::Zeroizing;

use crate::card::{AgentId, agent_id_of};
use crate::error::{Error, Result};

/// BLAKE3 derive-key context for the agent key. Changing it changes every agent id.
pub const AGENT_KEY_DOMAIN: &str = "maya2c 2026-11-12 agent signing key v1";

pub struct AgentSigner {
    key: HybridSigningKey,
    public_key: Vec<u8>,
}

impl AgentSigner {
    pub fn from_seed(seed: &MasterSeed) -> Self {
        let child = Zeroizing::new(blake3::derive_key(AGENT_KEY_DOMAIN, seed.expose()));
        let agent_seed = MasterSeed::from_bytes(*child);
        let key = HybridMlDsa65SlhDsa128s::signing_key_from_seed(&agent_seed);
        let public_key = HybridMlDsa65SlhDsa128s::public_key(&key);
        Self { key, public_key }
    }

    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    pub fn agent_id(&self) -> AgentId {
        agent_id_of(&self.public_key)
    }

    /// Crate-private: every message signed here starts with a length-prefixed
    /// card, quote or receipt domain, so the key never signs another protocol's
    /// bytes (a transaction's `signing_bytes`, say).
    pub(crate) fn sign(&self, message: &[u8]) -> Result<Vec<u8>> {
        HybridMlDsa65SlhDsa128s::sign(&self.key, message).map_err(|_| Error::Signing)
    }
}

impl core::fmt::Debug for AgentSigner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("AgentSigner(redacted)")
    }
}
