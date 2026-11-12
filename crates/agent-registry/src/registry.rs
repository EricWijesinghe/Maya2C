//! The registry store: verified cards, keyed by agent id.

use std::collections::BTreeMap;

use crate::card::{AgentId, SignedCard};
use crate::error::{Error, Result};

/// Most distinct agents held at once. Expired cards are dropped when the
/// table is full, so a burst of short-lived cards cannot hold the slots.
/// shortcut: in-memory cap; unexpired cards from many keys can still fill it
/// until persistence and per-publisher quotas exist.
pub const MAX_AGENTS: usize = 10_000;
/// How far ahead of the registry's clock a card's `issued_at` may be.
pub const MAX_CLOCK_SKEW_SECS: u64 = 300;

/// A card whose signature and expiry have been checked. Only [`VerifiedCard::new`]
/// makes one, so [`Registry::insert`] cannot be handed an unchecked card; the
/// HTTP layer verifies before it takes the registry lock.
#[derive(Debug)]
pub struct VerifiedCard {
    id: AgentId,
    card: SignedCard,
}

impl VerifiedCard {
    pub fn new(card: SignedCard, now: u64) -> Result<Self> {
        let id = card.verify(now)?;
        // A far-future `issued_at` would make every later update look stale,
        // so one bad card could pin the agent's slot until it expires.
        if card.card.issued_at > now.saturating_add(MAX_CLOCK_SKEW_SECS) {
            return Err(Error::Bounds);
        }
        Ok(Self { id, card })
    }
}

#[derive(Debug, Default)]
pub struct Registry {
    cards: BTreeMap<AgentId, SignedCard>,
}

impl Registry {
    /// Verifies the card, then stores it. A card replaces the held one only when newer.
    pub fn publish(&mut self, card: SignedCard, now: u64) -> Result<AgentId> {
        self.insert(VerifiedCard::new(card, now)?, now)
    }

    /// Stores an already verified card. A card replaces the held one only when newer.
    pub fn insert(&mut self, verified: VerifiedCard, now: u64) -> Result<AgentId> {
        let VerifiedCard { id, card } = verified;
        let stale = self
            .cards
            .get(&id)
            .is_some_and(|held| card.card.issued_at <= held.card.issued_at);
        if stale {
            return Err(Error::Stale);
        }
        if !self.cards.contains_key(&id) && self.cards.len() >= MAX_AGENTS {
            self.cards.retain(|_, held| now < held.card.expires_at);
            if self.cards.len() >= MAX_AGENTS {
                return Err(Error::Full);
            }
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
