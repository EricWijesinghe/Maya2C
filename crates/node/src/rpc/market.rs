//! Market-data endpoints for listing aggregators.
//!
//! ## What aggregators actually take from a chain
//!
//! CoinGecko's and CoinMarketCap's `/ticker` formats are their **exchange**
//! integration specs: trading pairs, last price, 24-hour volume, order-book
//! depth. A node has none of that. There is no price until the asset trades
//! somewhere, and a node that reported one would be inventing it.
//!
//! What both aggregators consume from a *project* is a **supply endpoint** — a
//! plain number for total and circulating supply, used to verify supply claims
//! and compute market capitalisation. That is real data a node can produce, and
//! it is what [`supply`] computes.
//!
//! This module is the arithmetic and the wire shapes. The HTTP endpoints that
//! actually serve them — `/api/v1/total_supply`, `/api/v1/circulating_supply`,
//! `/api/v1/ticker` — live in [`crate::rpc::market_http`], because aggregators
//! fetch them with a plain `GET` rather than a JSON-RPC call.
//!
//! The exchange-shaped ticker is therefore an adapter, not a data source. With
//! no market feed configured it answers `503` rather than fabricating a price.
//! Populating it requires an exchange integration, which is a listing step
//! rather than a code one.
//!
//! ## Supply under a shielded pool
//!
//! Hiding balances does not hide supply. The shielded pool's holdings are
//! exactly the sum of every `public_in` minus every `public_out` and fee ever
//! applied, all of which are in the clear, so the pool tracks its own balance
//! and total supply stays a matter of arithmetic. Coins inside the pool are
//! counted as circulating: they are spendable by whoever holds the notes, and
//! excluding them would understate supply by the amount most worth hiding.

use std::sync::Arc;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::consensus::chain::Chain;
use crate::error::Result;
use crate::state::Address;

/// Addresses whose balances are not in circulation.
///
/// The fee sink accumulates burned fees. They are unspendable — nobody holds
/// its key — so counting them as circulating would inflate the figure forever.
const NON_CIRCULATING: &[Address] = &[crate::state::shielded::FEE_SINK];

/// A supply figure, in base units.
///
/// Base units, matching every other amount in the codebase — account balances,
/// transfer amounts, the shielded pool's holdings. There is no decimals constant
/// here, so nothing is scaled. Saying "whole coins" in one place and "base
/// units" everywhere else is how a listing form ends up wrong by a power of ten.
///
/// Aggregators expect a bare decimal number, not JSON, on their supply
/// endpoints. This is the structured form, used by the `get_supply` JSON-RPC
/// method and by `/api/v1/supply`; the aggregator endpoints render a single
/// field of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplyReport {
    /// Every unit that exists, including unspendable ones.
    pub total: u64,
    /// Units that can actually move.
    pub circulating: u64,
    /// Units held inside the shielded pool.
    ///
    /// Included in `circulating`; broken out because it is the figure an
    /// auditor most wants and the one a transparent scan cannot recover.
    pub shielded: u64,
    /// Units held at addresses that can never spend them.
    pub burned: u64,
}

/// Computes supply from committed state.
///
/// # Errors
///
/// Propagates storage failures from the account scan.
pub fn supply(chain: &Arc<Mutex<Chain>>) -> Result<SupplyReport> {
    let chain = chain
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = chain.state();

    let mut transparent: u64 = 0;
    let mut burned: u64 = 0;

    for (address, account) in state.all_accounts()? {
        transparent = transparent.saturating_add(account.balance);
        if NON_CIRCULATING.contains(&address) {
            burned = burned.saturating_add(account.balance);
        }
    }

    let shielded = state.stored_pool()?.balance();
    let total = transparent.saturating_add(shielded);

    Ok(SupplyReport {
        total,
        circulating: total.saturating_sub(burned),
        shielded,
        burned,
    })
}

/// A quote from an exchange feed.
///
/// The node does not produce these. It carries whatever an operator has
/// configured, so the shape exists for the adapter to fill rather than for the
/// chain to invent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketQuote {
    /// Trading pair, e.g. `MAYA_USDT`.
    pub pair: String,
    /// Last traded price.
    pub last_price: f64,
    /// Base-asset volume over 24 hours.
    pub base_volume: f64,
    /// Quote-asset volume over 24 hours.
    pub quote_volume: f64,
}

/// A CoinGecko-shaped ticker entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoinGeckoTicker {
    /// Base asset symbol.
    pub base_currency: String,
    /// Quote asset symbol.
    pub target_currency: String,
    /// Last traded price.
    pub last_price: f64,
    /// Base volume over 24 hours.
    pub base_volume: f64,
    /// Quote volume over 24 hours.
    pub target_volume: f64,
}

/// A CoinMarketCap-shaped ticker entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoinMarketCapTicker {
    /// Base asset symbol.
    pub base_id: String,
    /// Quote asset symbol.
    pub quote_id: String,
    /// Last traded price.
    pub last_price: f64,
    /// Base volume over 24 hours.
    pub base_volume: f64,
    /// Quote volume over 24 hours.
    pub quote_volume: f64,
}

/// Splits `MAYA_USDT` into its two sides.
///
/// Returns `None` for a pair that is not exactly two non-empty halves, so a
/// malformed feed entry is dropped rather than producing a ticker with an empty
/// currency code.
fn split_pair(pair: &str) -> Option<(&str, &str)> {
    let (base, quote) = pair.split_once('_')?;
    if base.is_empty() || quote.is_empty() {
        return None;
    }
    Some((base, quote))
}

/// Translates exchange quotes into CoinGecko's ticker shape.
#[must_use]
pub fn to_coingecko(quotes: &[MarketQuote]) -> Vec<CoinGeckoTicker> {
    quotes
        .iter()
        .filter_map(|quote| {
            let (base, target) = split_pair(&quote.pair)?;
            Some(CoinGeckoTicker {
                base_currency: base.to_string(),
                target_currency: target.to_string(),
                last_price: quote.last_price,
                base_volume: quote.base_volume,
                target_volume: quote.quote_volume,
            })
        })
        .collect()
}

/// Translates exchange quotes into CoinMarketCap's ticker shape.
#[must_use]
pub fn to_coinmarketcap(quotes: &[MarketQuote]) -> Vec<CoinMarketCapTicker> {
    quotes
        .iter()
        .filter_map(|quote| {
            let (base, quote_id) = split_pair(&quote.pair)?;
            Some(CoinMarketCapTicker {
                base_id: base.to_string(),
                quote_id: quote_id.to_string(),
                last_price: quote.last_price,
                base_volume: quote.base_volume,
                quote_volume: quote.quote_volume,
            })
        })
        .collect()
}

/// Where market quotes come from, if anywhere.
///
/// Absent by default. A node with no configured feed reports that it has no
/// market data, which is the honest answer for an asset that is not yet traded.
#[derive(Clone, Debug, Default)]
pub struct MarketFeed {
    quotes: Vec<MarketQuote>,
}

impl MarketFeed {
    /// A feed with no quotes.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// A feed carrying operator-supplied quotes.
    #[must_use]
    pub fn with_quotes(quotes: Vec<MarketQuote>) -> Self {
        Self { quotes }
    }

    /// Whether any market data is available.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        !self.quotes.is_empty()
    }

    /// The quotes, if any.
    #[must_use]
    pub fn quotes(&self) -> &[MarketQuote] {
        &self.quotes
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn quote(pair: &str) -> MarketQuote {
        MarketQuote {
            pair: pair.to_string(),
            last_price: 1.25,
            base_volume: 1_000.0,
            quote_volume: 1_250.0,
        }
    }

    #[test]
    fn coingecko_tickers_split_the_pair() {
        let tickers = to_coingecko(&[quote("MAYA_USDT")]);
        assert_eq!(tickers.len(), 1);
        assert_eq!(tickers[0].base_currency, "MAYA");
        assert_eq!(tickers[0].target_currency, "USDT");
        assert_eq!(tickers[0].last_price, 1.25);
        assert_eq!(tickers[0].target_volume, 1_250.0);
    }

    #[test]
    fn coinmarketcap_tickers_split_the_pair() {
        let tickers = to_coinmarketcap(&[quote("MAYA_BTC")]);
        assert_eq!(tickers.len(), 1);
        assert_eq!(tickers[0].base_id, "MAYA");
        assert_eq!(tickers[0].quote_id, "BTC");
        assert_eq!(tickers[0].quote_volume, 1_250.0);
    }

    #[test]
    fn a_malformed_pair_is_dropped_rather_than_emitted_empty() {
        // A ticker with an empty currency code is worse than no ticker: an
        // aggregator would ingest it as a real market.
        for bad in ["MAYA", "_USDT", "MAYA_", ""] {
            assert!(to_coingecko(&[quote(bad)]).is_empty(), "accepted {bad:?}");
            assert!(
                to_coinmarketcap(&[quote(bad)]).is_empty(),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn an_unconfigured_feed_reports_no_market_data() {
        let feed = MarketFeed::empty();
        assert!(!feed.is_configured());
        assert!(feed.quotes().is_empty());
        assert!(to_coingecko(feed.quotes()).is_empty());
    }

    #[test]
    fn a_configured_feed_passes_quotes_through() {
        let feed = MarketFeed::with_quotes(vec![quote("MAYA_USDT"), quote("MAYA_BTC")]);
        assert!(feed.is_configured());
        assert_eq!(to_coingecko(feed.quotes()).len(), 2);
    }

    #[test]
    fn serialized_tickers_use_the_aggregator_field_names() {
        let json = serde_json::to_string(&to_coingecko(&[quote("MAYA_USDT")])[0]).expect("json");
        assert!(json.contains("\"base_currency\""));
        assert!(json.contains("\"target_currency\""));
        assert!(json.contains("\"target_volume\""));

        let json =
            serde_json::to_string(&to_coinmarketcap(&[quote("MAYA_USDT")])[0]).expect("json");
        assert!(json.contains("\"base_id\""));
        assert!(json.contains("\"quote_id\""));
        assert!(json.contains("\"quote_volume\""));
    }
}
