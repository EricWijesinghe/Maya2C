//! The order book: open intents, the nonces they spent, and solvers' quotes.
//!
//! Every collection is a `BTreeMap`/`BTreeSet`, and the quote order is a total
//! order on the quote's own fields, so two books fed the same quotes in any
//! order report the same best quote and the same list.

use std::collections::{BTreeMap, BTreeSet};

use primitive_types::U256;

use crate::error::{IntentError, Result};
use crate::intent::{IntentId, MAX_RECIPIENT_BYTES, SignedIntent};

/// Open intents the book holds.
// shortcut: in-memory caps chosen, not measured; size them from a load test before a public relay.
pub const MAX_INTENTS: usize = 10_000;
/// Spent `(maker, nonce)` pairs remembered. Not pruned with the intents: a
/// forgotten nonce is a replay the book would accept.
pub const MAX_SPENT_NONCES: usize = 100_000;
/// Quotes per intent: one per solver.
pub const MAX_QUOTES_PER_INTENT: usize = 64;

/// A solver's offer: deliver the intent's `buy_amount` for `price + fee` of
/// the sell asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quote {
    /// Which intent.
    pub intent_id: IntentId,
    /// The solver's account, the tie-breaker.
    pub solver: [u8; 32],
    /// Where the maker's lock pays: a 32-byte account if the sell asset is
    /// native, the other chain's address bytes otherwise.
    pub pay_to: Vec<u8>,
    /// What the solver asks for the trade.
    pub price: U256,
    /// What the solver charges on top.
    pub fee: U256,
}

impl Quote {
    /// `price + fee`, checked.
    ///
    /// # Errors
    ///
    /// [`IntentError::Overflow`] past 2^256 − 1.
    pub fn total(&self) -> Result<U256> {
        self.price
            .checked_add(self.fee)
            .ok_or(IntentError::Overflow)
    }
}

struct Open {
    intent: SignedIntent,
    /// Keyed by solver, so a solver has one quote and the map's order is the
    /// tie-break order.
    quotes: BTreeMap<[u8; 32], (U256, Quote)>,
}

/// The book.
pub struct IntentBook {
    network: [u8; 32],
    open: BTreeMap<IntentId, Open>,
    spent: BTreeSet<([u8; 32], u64)>,
}

impl IntentBook {
    /// An empty book for `network`.
    #[must_use]
    pub fn new(network: [u8; 32]) -> Self {
        Self {
            network,
            open: BTreeMap::new(),
            spent: BTreeSet::new(),
        }
    }

    /// Admits a verified, unexpired, unreplayed intent at `height`.
    ///
    /// # Errors
    ///
    /// [`IntentError::Expired`], [`IntentError::Replay`], [`IntentError::Full`],
    /// or whatever [`SignedIntent::verify`] refuses.
    pub fn post(&mut self, intent: SignedIntent, height: u64) -> Result<IntentId> {
        if height >= intent.intent.deadline_height {
            return Err(IntentError::Expired);
        }
        let spend = (intent.intent.maker, intent.intent.nonce);
        if self.spent.contains(&spend) {
            return Err(IntentError::Replay);
        }
        if self.open.len() >= MAX_INTENTS || self.spent.len() >= MAX_SPENT_NONCES {
            return Err(IntentError::Full);
        }
        intent.verify(&self.network)?;
        let id = intent.id();
        self.spent.insert(spend);
        self.open.insert(
            id,
            Open {
                intent,
                quotes: BTreeMap::new(),
            },
        );
        Ok(id)
    }

    /// The open intent with this id.
    #[must_use]
    pub fn intent(&self, id: &IntentId) -> Option<&SignedIntent> {
        self.open.get(id).map(|o| &o.intent)
    }

    /// Records a quote. A solver may lower its own quote, never raise it:
    /// quotes are unsigned, so a raise in its name could come from anyone.
    ///
    /// # Errors
    ///
    /// [`IntentError::UnknownIntent`], [`IntentError::Expired`],
    /// [`IntentError::Overflow`], [`IntentError::WorseThanLimit`],
    /// [`IntentError::NotAnImprovement`], [`IntentError::Malformed`] for a
    /// bad `pay_to`, or [`IntentError::Full`].
    pub fn offer(&mut self, quote: Quote, height: u64) -> Result<()> {
        let open = self
            .open
            .get_mut(&quote.intent_id)
            .ok_or(IntentError::UnknownIntent)?;
        let intent = &open.intent.intent;
        if height >= intent.deadline_height {
            return Err(IntentError::Expired);
        }
        check_pay_to(&quote.pay_to, intent.sell_asset.is_native())?;
        let total = quote.total()?;
        if total > intent.max_sell {
            return Err(IntentError::WorseThanLimit);
        }
        match open.quotes.get(&quote.solver) {
            Some((earlier, _)) if total >= *earlier => return Err(IntentError::NotAnImprovement),
            None if open.quotes.len() >= MAX_QUOTES_PER_INTENT => return Err(IntentError::Full),
            _ => {}
        }
        open.quotes.insert(quote.solver, (total, quote));
        Ok(())
    }

    /// Quotes for an intent, best first: lowest total, then lowest solver id.
    ///
    /// # Errors
    ///
    /// [`IntentError::UnknownIntent`].
    pub fn quotes(&self, id: &IntentId) -> Result<Vec<Quote>> {
        let open = self.open.get(id).ok_or(IntentError::UnknownIntent)?;
        let mut ranked: Vec<&(U256, Quote)> = open.quotes.values().collect();
        // Solver ids are unique keys, so (total, solver) is a total order.
        ranked.sort_by_key(|(total, quote)| (*total, quote.solver));
        Ok(ranked.into_iter().map(|(_, q)| q.clone()).collect())
    }

    /// The best quote.
    ///
    /// # Errors
    ///
    /// [`IntentError::UnknownIntent`] or [`IntentError::NoQuote`].
    pub fn best(&self, id: &IntentId) -> Result<Quote> {
        self.quotes(id)?
            .into_iter()
            .next()
            .ok_or(IntentError::NoQuote)
    }

    /// Drops intents dead at `height`. Their nonces stay spent.
    pub fn prune(&mut self, height: u64) {
        self.open
            .retain(|_, o| height < o.intent.intent.deadline_height);
    }
}

fn check_pay_to(pay_to: &[u8], native: bool) -> Result<()> {
    let ok = if native {
        pay_to.len() == 32
    } else {
        !pay_to.is_empty() && pay_to.len() <= MAX_RECIPIENT_BYTES
    };
    if ok {
        Ok(())
    } else {
        Err(IntentError::Malformed("pay_to length out of range"))
    }
}
