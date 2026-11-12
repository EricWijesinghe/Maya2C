//! The registry store: verified cards, keyed by agent id.

use std::collections::BTreeMap;

use crate::card::{AgentId, SignedCard};
use crate::error::{Error, Result};

/// Most distinct agents held at once.
/// shortcut: in-memory cap; a full registry refuses new agents until persistence and eviction exist.
pub const MAX_AGENTS: usize = 10_000;
/// How far ahead of the registry's clock a card's `issued_at` may be.
pub const MAX_CLOCK_SKEW_SECS: u64 = 300;

#[derive(Debug, Default)]
pub struct Registry {
    cards: BTreeMap<AgentId, SignedCard>,
}

impl Registry {
    /// Verifies the card, then stores it. A card replaces the held one only when newer.
    pub fn publish(&mut self, card: SignedCard, now: u64) -> Result<AgentId> {
        let id = card.verify(now)?;
        // A far-future `issued_at` would make every later update look stale,
        // so a card that leaked once could pin the slot until it expires.
        if card.card.issued_at > now.saturating_add(MAX_CLOCK_SKEW_SECS) {
            return Err(Error::Bounds);
        }
        let stale = self
            .cards
            .get(&id)
            .is_some_and(|held| card.card.issued_at <= held.card.issued_at);
        if stale {
            return Err(Error::Stale);
        }
        if !self.cards.contains_key(&id) && self.cards.len() >= MAX_AGENTS {
            return Err(Error::Full);
        }
        self.cards.insert(id, card);
        Ok(id)
    }

    /// The card for `id`, unless it has expired by `now`.
    pub fn get(&self, id: &AgentId, now: u64) -> Option<&SignedCard> {
        self.cards.get(id).filter(|card| now < card.card.expires_at)
    }

    /// Unexpired cards offering `capability`, in agent-id order.
    pub fn search(&self, capability: &str, now: u64) -> Vec<&SignedCard> {
        self.cards
            .values()
            .filter(|card| now < card.card.expires_at)
            .filter(|card| card.card.capabilities.iter().any(|c| c == capability))
            .collect()
    }
}
