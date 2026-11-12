//! Agent cards: what an agent offers, signed by the agent's own key.
//!
//! A card arrives from an untrusted peer, so its bounds are checked before any
//! hashing or verifying. The signature covers the canonical bytes; the agent id
//! is `BLAKE3(domain || public key)`, so an id is bound to one key.

use maya_crypto_pq::suite::{HybridMlDsa65SlhDsa128s, SignatureSuite};
use primitive_types::U256;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::signer::AgentSigner;
use crate::wire;

pub const CARD_DOMAIN: &[u8] = b"maya2c agent card v1";
const ID_DOMAIN: &[u8] = b"maya2c agent id v1";
/// Longest text field, in bytes.
pub const MAX_TEXT_BYTES: usize = 256;
/// Most capabilities, and most prices, per card.
pub const MAX_LIST_LEN: usize = 32;
/// Longest card lifetime. An agent re-signs to stay listed, so a slot held by
/// a forgotten card frees itself.
pub const MAX_LIFETIME_SECS: u64 = 30 * 24 * 3600;

/// A capability is a token: ASCII letters, digits and `. _ : -`. Card text is
/// shown to a model by the MCP tools, so free text is kept out of the fields
/// it matches on.
pub fn is_capability(text: &str) -> bool {
    (1..=MAX_TEXT_BYTES).contains(&text.len())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c))
}

/// Display text: bounded, and no control or bidirectional-override
/// characters, which could hide or reorder what a person or model reads.
fn is_display_text(text: &str) -> bool {
    const BIDI: [char; 11] = [
        '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}',
        '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
    ];
    (1..=MAX_TEXT_BYTES).contains(&text.len())
        && !text.chars().any(|c| c.is_control() || BIDI.contains(&c))
}

/// An endpoint is `https://…`, or plain HTTP on loopback for local agents;
/// printable ASCII only.
fn is_endpoint(text: &str) -> bool {
    let schemes = ["https://", "http://127.0.0.1:", "http://localhost:"];
    (1..=MAX_TEXT_BYTES).contains(&text.len())
        && text.chars().all(|c| c.is_ascii_graphic())
        && schemes.iter().any(|s| text.starts_with(s))
}

pub type AgentId = [u8; 32];

pub fn agent_id_of(public_key: &[u8]) -> AgentId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ID_DOMAIN);
    hasher.update(public_key);
    *hasher.finalize().as_bytes()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Price {
    pub capability: String,
    #[serde(with = "wire::dec_u256")]
    pub amount: U256,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCard {
    pub name: String,
    pub capabilities: Vec<String>,
    /// Advertised prices. A quote may differ; the quote is what binds.
    pub prices: Vec<Price>,
    /// Where the agent takes quote requests.
    pub endpoint: String,
    pub issued_at: u64,
    /// Unix seconds. The card is refused at and after this instant.
    pub expires_at: u64,
    #[serde(with = "wire::hex_bytes")]
    pub public_key: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedCard {
    pub card: AgentCard,
    #[serde(with = "wire::hex_bytes")]
    pub signature: Vec<u8>,
}

impl AgentCard {
    pub fn agent_id(&self) -> AgentId {
        agent_id_of(&self.public_key)
    }

    /// The bytes that are signed. Refuses a card outside the bounds.
    pub fn canonical(&self) -> Result<Vec<u8>> {
        self.check_bounds()?;
        let mut out = Vec::new();
        wire::put(&mut out, CARD_DOMAIN);
        wire::put(&mut out, self.name.as_bytes());
        wire::put(&mut out, self.endpoint.as_bytes());
        wire::put(&mut out, &self.public_key);
        out.extend_from_slice(&self.issued_at.to_be_bytes());
        out.extend_from_slice(&self.expires_at.to_be_bytes());
        wire::put_len(&mut out, self.capabilities.len());
        for capability in &self.capabilities {
            wire::put(&mut out, capability.as_bytes());
        }
        wire::put_len(&mut out, self.prices.len());
        for price in &self.prices {
            wire::put(&mut out, price.capability.as_bytes());
            wire::put_u256(&mut out, price.amount);
        }
        Ok(out)
    }

    fn check_bounds(&self) -> Result<()> {
        let lists_ok = !self.capabilities.is_empty()
            && self.capabilities.len() <= MAX_LIST_LEN
            && self.prices.len() <= MAX_LIST_LEN;
        let capabilities_ok = self.capabilities.iter().all(|c| is_capability(c));
        // Membership in `capabilities` already implies the token charset.
        let prices_ok = self
            .prices
            .iter()
            .all(|p| self.capabilities.contains(&p.capability) && !p.amount.is_zero());
        let window_ok = self.expires_at > self.issued_at
            && self.expires_at - self.issued_at <= MAX_LIFETIME_SECS;
        if is_display_text(&self.name)
            && is_endpoint(&self.endpoint)
            && lists_ok
            && capabilities_ok
            && prices_ok
            && window_ok
        {
            Ok(())
        } else {
            Err(Error::Bounds)
        }
    }
}

impl SignedCard {
    /// Signs `card` with the agent's key. The key must be the one the card names.
    pub fn sign(card: AgentCard, signer: &AgentSigner) -> Result<Self> {
        if card.public_key.as_slice() != signer.public_key() {
            return Err(Error::KeyMismatch);
        }
        let signature = signer.sign(&card.canonical()?)?;
        Ok(Self { card, signature })
    }

    /// Checks the signature, then the expiry. Returns the agent id when both pass.
    pub fn verify(&self, now: u64) -> Result<AgentId> {
        let canonical = self.card.canonical()?;
        HybridMlDsa65SlhDsa128s::verify(&self.card.public_key, &canonical, &self.signature)
            .map_err(|_| Error::Signature)?;
        if now >= self.card.expires_at {
            return Err(Error::Expired);
        }
        Ok(self.card.agent_id())
    }
}
