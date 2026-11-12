//! The agent's Maya2C key: hybrid ML-DSA-65 + SLH-DSA-128s, the suite the node
//! already verifies. The seed is a `MasterSeed` (a secret box); the key type
//! zeroizes on drop. Nothing here logs key material.

use maya_crypto_pq::suite::{
    HybridMlDsa65SlhDsa128s, HybridSigningKey, MasterSeed, SignatureSuite,
};

use crate::card::{AgentId, agent_id_of};
use crate::error::{Error, Result};

pub struct AgentSigner {
    key: HybridSigningKey,
    public_key: Vec<u8>,
}

impl AgentSigner {
    pub fn from_seed(seed: &MasterSeed) -> Self {
        let key = HybridMlDsa65SlhDsa128s::signing_key_from_seed(seed);
        let public_key = HybridMlDsa65SlhDsa128s::public_key(&key);
        Self { key, public_key }
    }

    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    pub fn agent_id(&self) -> AgentId {
        agent_id_of(&self.public_key)
    }

    pub fn sign(&self, message: &[u8]) -> Result<Vec<u8>> {
        HybridMlDsa65SlhDsa128s::sign(&self.key, message).map_err(|_| Error::Signing)
    }
}

impl core::fmt::Debug for AgentSigner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("AgentSigner(redacted)")
    }
}
