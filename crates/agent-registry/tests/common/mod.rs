//! Shared fixtures for the agent-registry tests. Keys are derived from fixed
//! seeds, so every run signs the same bytes.
#![allow(dead_code)] // each test binary uses a different subset of these helpers

use maya_agent_registry::card::{AgentCard, Price, SignedCard};
use maya_agent_registry::signer::AgentSigner;
use maya_crypto_pq::suite::MasterSeed;
use primitive_types::U256;

/// A fixed clock reading (Unix seconds) for the card and payment tests.
pub const NOW: u64 = 1_800_000_000;

pub fn signer(seed: u8) -> AgentSigner {
    AgentSigner::from_seed(&MasterSeed::from_bytes([seed; 32]))
}

/// A card that offers `capability` at 5 base units, valid from `at - 60` to `at + 3600`.
pub fn card_at(signer: &AgentSigner, capability: &str, at: u64) -> AgentCard {
    AgentCard {
        name: "translator".into(),
        capabilities: vec![capability.into()],
        prices: vec![Price {
            capability: capability.into(),
            amount: U256::from(5u64),
        }],
        endpoint: "http://127.0.0.1:9000/quote".into(),
        issued_at: at - 60,
        expires_at: at + 3600,
        public_key: signer.public_key().to_vec(),
    }
}

pub fn signed_at(signer: &AgentSigner, capability: &str, at: u64) -> SignedCard {
    SignedCard::sign(card_at(signer, capability, at), signer).expect("a card the signer owns signs")
}
