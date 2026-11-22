//! Market-data endpoints for listing aggregators.
//!
//! ## What aggregators actually take from a chain
//!
//! `CoinGecko`'s and `CoinMarketCap`'s `/ticker` formats are their **exchange**
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

use primitive_types::U256;
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
/// transfer amounts, the shielded pool's holdings. No decimals are applied
/// here (the coin's 24 are `maya_amount::COIN_DECIMALS`, ADR-068), so nothing is scaled. Saying "whole coins" in one place and "base
/// units" everywhere else is how a listing form ends up wrong by a power of ten.
///
/// Aggregators expect a bare decimal number, not JSON, on their supply
/// endpoints. This is the structured form, used by the `get_supply` JSON-RPC
/// method and by `/api/v1/supply`; the aggregator endpoints render a single
/// field of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplyReport {
    /// Every unit that exists, including unspendable ones.
    #[serde(with = "crate::state::amount_serde")]
    pub total: U256,
    /// Units that can actually move.
    #[serde(with = "crate::state::amount_serde")]
    pub circulating: U256,
    /// Units held inside the shielded pool.
    ///
    /// Included in `circulating`; broken out because it is the figure an
    /// auditor most wants and the one a transparent scan cannot recover.
    #[serde(with = "crate::state::amount_serde")]
    pub shielded: U256,
    /// Units held at addresses that can never spend them.
    #[serde(with = "crate::state::amount_serde")]
    pub burned: U256,
}

/// Computes supply from committed state.
///
/// The account walk runs outside the chain lock: block apply and DAG-BFT
/// commit take that mutex, and holding it for a walk over every account
/// stalled them for as long as the walk took. The walk reads a `RocksDB`
/// snapshot taken under the lock with the tip ([`StateDB::view`]), so it is
/// consistent with that tip however many blocks land while it runs, and it
/// never retries or falls back to walking under the lock.
///
/// [`StateDB::view`]: crate::state::StateDB::view
///
/// # Errors
///
/// Propagates storage failures from the account scan.
pub fn supply(chain: &Arc<Mutex<Chain>>) -> Result<SupplyReport> {
    supply_at(chain).map(|cached| cached.report)
}

/// [`supply`], with the tip it describes.
fn supply_at(chain: &Arc<Mutex<Chain>>) -> Result<Cached> {
    supply_at_with(chain, || {})
}

/// [`supply_at`], calling `during_walk` once the walk's view is fixed and the
/// chain lock is released: where a test advances the tip mid-walk.
fn supply_at_with(chain: &Arc<Mutex<Chain>>, mut during_walk: impl FnMut()) -> Result<Cached> {
    let guard = lock(chain);
    let (state, height, tip) = (Arc::clone(guard.state()), guard.height(), guard.tip());
    let view = state.view();
    drop(guard);
    during_walk();
    Ok(Cached {
        height,
        tip,
        report: supply_of(&view)?,
    })
}

fn lock(chain: &Mutex<Chain>) -> std::sync::MutexGuard<'_, Chain> {
    chain
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A supply report and the tip it was computed at.
#[derive(Clone, Copy, Debug)]
struct Cached {
    height: u64,
    tip: [u8; 32],
    report: SupplyReport,
}

/// The supply report for the newest tip asked about, computed once per tip.
///
/// Supply walks every account, the most expensive read the node offers, and
/// it cannot change while the tip stands still, so caching on the tip trades
/// no freshness. Single-flight: a caller that arrives while a walk runs waits
/// for it and takes its result, so N requests after a block cost one walk,
/// not N. Every public surface (`get_supply`, the market endpoints) reads
/// through one of these.
#[derive(Debug, Default)]
pub struct SupplyCache {
    last: Mutex<Option<Cached>>,
    scan: Mutex<()>,
}

impl SupplyCache {
    /// The supply at the current tip, from the cache when it is current.
    ///
    /// # Errors
    ///
    /// Propagates storage failures from the account scan.
    pub fn get(&self, chain: &Arc<Mutex<Chain>>) -> Result<SupplyReport> {
        let (height, tip) = {
            let chain = lock(chain);
            (chain.height(), chain.tip())
        };
        if let Some(report) = self.hit(height, tip) {
            return Ok(report);
        }
        let _scan = self
            .scan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Whoever held the scan before us may have just stored this tip.
        if let Some(report) = self.hit(height, tip) {
            return Ok(report);
        }
        let fresh = supply_at(chain)?;
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Walks are serialized by `scan` and each reads the chain when it
        // starts, so the newest walk is the current tip, even a lower one
        // after a heaviest-work reorg: always replace.
        *last = Some(fresh);
        Ok(fresh.report)
    }

    fn hit(&self, height: u64, tip: [u8; 32]) -> Option<SupplyReport> {
        let last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Exact match only: a cached height above the chain's is another
        // branch after a reorg, not a newer view of this one.
        last.filter(|old| old.height == height && old.tip == tip)
            .map(|old| old.report)
    }
}

/// Supply from one consistent view of committed state, read without any lock.
fn supply_of(state: &crate::state::db::StateView<'_>) -> Result<SupplyReport> {
    let mut transparent = U256::zero();
    let mut burned = U256::zero();

    for (address, account) in state.all_accounts()? {
        transparent = transparent.saturating_add(account.balance);
        if NON_CIRCULATING.contains(&address) {
            burned = burned.saturating_add(account.balance);
        }
    }

    let shielded = state.stored_pool()?.balance();
    // Bonds, delegations and unbonding funds were debited from accounts but
    // still exist; genesis counts them in total supply, so this must too.
    let staked = state
        .committed_staking()?
        .and_then(|r| r.staking.held())
        .map_or(U256::zero(), maya_stateless_core::amount::to_u256);
    let total = transparent.saturating_add(shielded).saturating_add(staked);

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

/// Translates exchange quotes into `CoinGecko`'s ticker shape.
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

/// Translates exchange quotes into `CoinMarketCap`'s ticker shape.
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

    #[test]
    fn a_tip_that_moves_during_every_walk_never_puts_the_walk_under_the_lock() {
        use crate::consensus::ChainConfig;
        use crate::core::{Block, BlockHeader};
        use crate::state::{Account, StateDB};
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(StateDB::open(dir.path()).unwrap());
        let fund = |balance: u64| {
            let account = Account {
                balance: U256::from(balance),
                nonce: 0,
            };
            state.put_account(&[1; 32], &account).unwrap();
        };
        fund(1_000);
        let genesis = Block::new(
            BlockHeader {
                version: 0,
                prev_hash: [0; 32],
                state_root: [0; 32],
                timestamp: 1_700_000_000,
                nonce: 0,
                difficulty_target: crate::crypto::pow::target_from_leading_zero_bits(6),
                tx_root: [0; 32],
            },
            Vec::new(),
        );
        let chain = Arc::new(Mutex::new(
            Chain::open(
                Arc::clone(&state),
                genesis,
                ChainConfig::without_pow_verification(),
            )
            .unwrap(),
        ));
        let (start_height, start_tip) = {
            let c = lock(&chain);
            (c.height(), c.tip())
        };
        let start = supply_at(&chain).unwrap().report;

        // Every walk sees a new block land and a balance change behind it.
        let mut walks = 0_u64;
        let got = supply_at_with(&chain, || {
            walks += 1;
            let mut c = chain
                .try_lock()
                .expect("the walk must not hold the chain lock");
            let timestamp = 1_700_000_000 + c.height() + 1;
            let block = c.candidate_block(timestamp, Vec::new()).unwrap();
            c.insert_block(block).unwrap();
            fund(1_000 + walks * 7);
        })
        .unwrap();
        // The report is the one consistent view taken with the tip it names.
        assert_eq!((got.height, got.tip), (start_height, start_tip));
        assert_eq!(got.report.total, start.total);
        assert_eq!(walks, 1, "one walk, never a retry ending under the lock");
        // And the next walk sees the blocks and the balance that landed.
        let after = supply_at(&chain).unwrap();
        assert_eq!(after.height, start_height + 1);
        assert_eq!(after.report.total, U256::from(1_007));
    }

    #[test]
    fn a_reorg_to_a_lower_height_recomputes_the_supply() {
        // Why: fork choice is heaviest work, so a reorg can leave the chain
        // lower than the cached height. A cache that served any newer height
        // kept the left branch's supply until the chain grew past it.
        use crate::consensus::ChainConfig;
        use crate::core::{Block, BlockHeader};
        use crate::state::{Account, StateDB};
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(StateDB::open(dir.path()).unwrap());
        let account = Account {
            balance: U256::from(1_000_u64),
            nonce: 0,
        };
        state.put_account(&[1; 32], &account).unwrap();
        let genesis = Block::new(
            BlockHeader {
                version: 0,
                prev_hash: [0; 32],
                state_root: [0; 32],
                timestamp: 1_700_000_000,
                nonce: 0,
                difficulty_target: crate::crypto::pow::target_from_leading_zero_bits(6),
                tx_root: [0; 32],
            },
            Vec::new(),
        );
        let chain = Arc::new(Mutex::new(
            Chain::open(state, genesis, ChainConfig::without_pow_verification()).unwrap(),
        ));
        let now = supply_at(&chain).unwrap();
        // The cache holds a report from a branch above the chain's height.
        let left_branch = Cached {
            height: now.height + 5,
            tip: [0xAB; 32],
            report: SupplyReport {
                total: U256::from(7_u64),
                ..now.report
            },
        };
        let cache = SupplyCache::default();
        *cache.last.lock().unwrap() = Some(left_branch);
        assert_eq!(cache.get(&chain).unwrap().total, now.report.total);
        // And the cache now holds the chain's own tip, so the next call hits.
        let held = cache.last.lock().unwrap().unwrap();
        assert_eq!((held.height, held.tip), (now.height, now.tip));
    }

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
