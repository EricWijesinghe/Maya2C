//! The aggregator-facing HTTP endpoints.
//!
//! ## Why these are not JSON-RPC methods
//!
//! `get_supply` already exists on the JSON-RPC server, and it is the wrong shape
//! for the consumers that need it. CoinGecko and CoinMarketCap fetch a supply
//! figure with an unauthenticated `GET` and parse the response body as a number.
//! They do not POST a JSON-RPC envelope, and they will not be taught to. An
//! endpoint they cannot call is an endpoint that does not exist as far as a
//! listing is concerned.
//!
//! So this is a small HTTP surface built on the same hyper server the metrics
//! exporter uses, serving the handful of paths the aggregators document.
//!
//! ## Why a separate port from the exporter
//!
//! These are the only endpoints the node serves that are *meant* to be public
//! and unauthenticated. The exporter on 9600 is the opposite — it publishes peer
//! topology and mempool contents, and the NetworkPolicy confines it to the
//! monitoring namespace. Putting public supply data behind that policy would
//! mean either exposing the exporter or not answering aggregators, so the two
//! get separate listeners and the firewall rules can differ.
//!
//! ## Units
//!
//! Every figure is in base units, the same unit `AccountInfo::balance` and the
//! chain's transfer amounts use. There is no decimals constant in this codebase,
//! so no scaling is applied. If one is ever introduced, both aggregators must be
//! told the exponent — an unannounced change of unit shows up as a supply figure
//! wrong by orders of magnitude, and it is the kind of error that gets a listing
//! flagged rather than corrected.
//!
//! ## Prices
//!
//! A node has no price. There is none until the asset trades somewhere, and a
//! node that reported one would be inventing it. The ticker endpoints are
//! adapters over an operator-supplied [`MarketFeed`]; with no feed configured
//! they answer `503` rather than fabricating a quote.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use crate::consensus::chain::Chain;
use crate::rpc::market::{self, MarketFeed, SupplyReport};

/// Content type for the bare-number supply endpoints.
const PLAIN: &str = "text/plain; charset=utf-8";

/// Content type for the structured endpoints.
const JSON: &str = "application/json; charset=utf-8";

/// A running market server.
#[derive(Debug)]
pub struct MarketServer {
    /// Address actually bound, which matters when the caller asked for port 0.
    pub address: SocketAddr,
}

/// Everything a request handler needs.
///
/// No `Debug`: `Chain` does not implement it, and a derive that printed the
/// chain would be dumping state rather than describing a handle.
pub struct MarketState {
    /// Chain the supply figures are computed from.
    chain: Arc<Mutex<Chain>>,
    /// Operator-supplied quotes, if any.
    feed: MarketFeed,
    /// Last computed supply, and the height it was computed at.
    ///
    /// Supply is a full scan of every account (see [`market::supply`]), which is
    /// the most expensive read the node offers. An aggregator polling it every
    /// minute is fine; a public endpoint being scraped by anyone is a free way
    /// to make every node behind the load balancer walk its whole account set.
    /// The figure cannot change without the height changing, so caching on
    /// height is exact rather than approximate — this trades no freshness at
    /// all.
    cache: Mutex<Option<(u64, SupplyReport)>>,
}

impl MarketState {
    /// Builds the shared state.
    #[must_use]
    pub fn new(chain: Arc<Mutex<Chain>>, feed: MarketFeed) -> Self {
        Self {
            chain,
            feed,
            cache: Mutex::new(None),
        }
    }

    /// Current tip height.
    fn height(&self) -> u64 {
        self.chain
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .height()
    }

    /// Returns the supply report, recomputing only when the tip has moved.
    ///
    /// # Errors
    ///
    /// Propagates storage failures from the account scan.
    fn supply(&self) -> crate::error::Result<SupplyReport> {
        let height = self.height();

        {
            let cache = self
                .cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((cached_height, report)) = *cache
                && cached_height == height
            {
                return Ok(report);
            }
        }

        // Computed outside the cache lock. Holding it across a full account scan
        // would serialise every concurrent request behind the first one, which
        // turns a slow endpoint into a stalled one.
        let report = market::supply(&self.chain)?;

        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // A concurrent request may have stored a newer height meanwhile. Keep
        // whichever is further along rather than moving the cache backwards.
        let newer = cache.is_some_and(|(cached_height, _)| cached_height > height);
        if !newer {
            *cache = Some((height, report));
        }

        Ok(report)
    }
}

/// Binds the market server and serves it on a background task.
///
/// # Errors
///
/// Returns an error if the address cannot be bound.
pub async fn serve(address: SocketAddr, state: Arc<MarketState>) -> std::io::Result<MarketServer> {
    let listener = TcpListener::bind(address).await?;
    let bound = listener.local_addr()?;

    tokio::spawn(async move {
        loop {
            let (stream, _peer) = match listener.accept().await {
                Ok(accepted) => accepted,
                // A failed accept is not a reason to stop answering; the next
                // poll should still find a listener.
                Err(error) => {
                    eprintln!("market: accept failed: {error}");
                    continue;
                }
            };

            let state = Arc::clone(&state);
            tokio::spawn(async move {
                let service = service_fn(move |request| {
                    let state = Arc::clone(&state);
                    async move { route(&request, &state) }
                });

                if let Err(error) = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await
                {
                    // Clients hang up abruptly all the time.
                    let _ = error;
                }
            });
        }
    });

    Ok(MarketServer { address: bound })
}

/// Builds a response, falling back to a bare 500 if the builder rejects it.
fn respond(status: StatusCode, content_type: &str, body: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, content_type)
        // These endpoints are public, unauthenticated, and read-only, and block
        // explorers fetch them from the browser. Nothing here is sensitive, and
        // omitting this only means the data must be proxied to be usable.
        .header(hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Full::new(Bytes::from(body)))
        .unwrap_or_else(|_| {
            // A builder failure means a header this code constructed is
            // malformed, which is a bug rather than a request problem — but the
            // server must still answer rather than drop the connection.
            let mut fallback = Response::new(Full::new(Bytes::new()));
            *fallback.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            fallback
        })
}

/// A JSON error body.
fn error_json(message: &str) -> String {
    // Built with serde rather than `format!` so a message containing a quote
    // cannot produce a body that is not valid JSON.
    serde_json::json!({ "error": message }).to_string()
}

/// Serialises a value, or produces a 500 if serialisation fails.
fn json_response<T: serde::Serialize>(value: &T) -> Response<Full<Bytes>> {
    match serde_json::to_string(value) {
        Ok(body) => respond(StatusCode::OK, JSON, body),
        Err(error) => respond(
            StatusCode::INTERNAL_SERVER_ERROR,
            JSON,
            error_json(&format!("serialisation failed: {error}")),
        ),
    }
}

/// Answers a supply request, projecting the report through `select`.
fn supply_response(state: &MarketState, select: fn(&SupplyReport) -> u64) -> Response<Full<Bytes>> {
    match state.supply() {
        // A bare decimal with no newline, no JSON, no units suffix. Both
        // aggregators parse the whole body as a number.
        Ok(report) => respond(StatusCode::OK, PLAIN, select(&report).to_string()),
        Err(error) => respond(
            StatusCode::INTERNAL_SERVER_ERROR,
            PLAIN,
            format!("supply unavailable: {error}"),
        ),
    }
}

/// Answers a ticker request, or explains why there is nothing to report.
fn ticker_response<T: serde::Serialize>(
    state: &MarketState,
    translate: fn(&[market::MarketQuote]) -> Vec<T>,
) -> Response<Full<Bytes>> {
    if !state.feed.is_configured() {
        // 503 rather than an empty array: an empty array says "this asset trades
        // nowhere", which an aggregator may cache as a fact. 503 says "ask
        // again", which is the truth for an asset that is not yet listed.
        return respond(
            StatusCode::SERVICE_UNAVAILABLE,
            JSON,
            error_json(
                "no market feed configured; a node cannot produce a price for an asset \
                 that does not trade yet",
            ),
        );
    }

    json_response(&translate(state.feed.quotes()))
}

/// Routes a single request.
fn route(
    request: &Request<hyper::body::Incoming>,
    state: &MarketState,
) -> Result<Response<Full<Bytes>>, Infallible> {
    // Only GET. These endpoints are reads, and answering a POST would be a
    // surface with no purpose.
    if request.method() != hyper::Method::GET {
        return Ok(respond(
            StatusCode::METHOD_NOT_ALLOWED,
            JSON,
            error_json("only GET is supported"),
        ));
    }

    let response = match request.uri().path() {
        "/api/v1/total_supply" => supply_response(state, |report| report.total),
        "/api/v1/circulating_supply" => supply_response(state, |report| report.circulating),

        // The structured form, for anyone who wants the breakdown rather than
        // one number. Not part of either aggregator's spec.
        "/api/v1/supply" => match state.supply() {
            Ok(report) => json_response(&report),
            Err(error) => respond(
                StatusCode::INTERNAL_SERVER_ERROR,
                JSON,
                error_json(&format!("supply unavailable: {error}")),
            ),
        },

        "/api/v1/ticker" => ticker_response(state, market::to_coingecko),
        "/api/v1/cmc/ticker" => ticker_response(state, market::to_coinmarketcap),

        "/healthz" => respond(StatusCode::OK, PLAIN, "ok".to_string()),

        _ => respond(StatusCode::NOT_FOUND, JSON, error_json("no such endpoint")),
    };

    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::consensus::{Chain, ChainConfig};
    use crate::core::{Block, BlockHeader};
    use crate::crypto::pow::target_from_leading_zero_bits;
    use crate::rpc::market::MarketQuote;
    use crate::state::{Account, Address, StateDB};

    /// A chain on a temporary RocksDB, optionally with funded accounts.
    ///
    /// Proof-of-work verification is off: every attempt is a 32 MiB Argon2id
    /// pass, and none of these tests are about the work.
    fn chain(dir: &std::path::Path, funded: &[(Address, u64)]) -> Arc<Mutex<Chain>> {
        let state = Arc::new(StateDB::open(dir).expect("open state"));

        for (address, balance) in funded {
            state
                .put_account(
                    address,
                    &Account {
                        balance: *balance,
                        nonce: 0,
                    },
                )
                .expect("fund");
        }

        let genesis = Block::new(
            BlockHeader {
                prev_hash: [0u8; 32],
                state_root: [0u8; 32],
                timestamp: 1_700_000_000,
                nonce: 0,
                difficulty_target: target_from_leading_zero_bits(6),
                tx_root: [0u8; 32],
            },
            Vec::new(),
        );

        Arc::new(Mutex::new(
            Chain::open(state, genesis, ChainConfig::without_pow_verification())
                .expect("open chain"),
        ))
    }

    fn quote(pair: &str) -> MarketQuote {
        MarketQuote {
            pair: pair.to_string(),
            last_price: 1.25,
            base_volume: 1_000.0,
            quote_volume: 1_250.0,
        }
    }

    /// Binds on an ephemeral port so tests can run concurrently.
    async fn start(feed: MarketFeed) -> (MarketServer, tempfile::TempDir) {
        start_funded(feed, &[]).await
    }

    async fn start_funded(
        feed: MarketFeed,
        funded: &[(Address, u64)],
    ) -> (MarketServer, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(MarketState::new(chain(dir.path(), funded), feed));
        let server = serve("127.0.0.1:0".parse().expect("valid address"), state)
            .await
            .expect("bind");
        (server, dir)
    }

    /// Minimal HTTP/1.1 request, so the test needs no client dependency.
    async fn request(address: SocketAddr, method: &str, path: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut stream = tokio::net::TcpStream::connect(address)
            .await
            .expect("connect");
        let text =
            format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        stream.write_all(text.as_bytes()).await.expect("write");

        let mut response = String::new();
        stream.read_to_string(&mut response).await.expect("read");
        response
    }

    async fn get(address: SocketAddr, path: &str) -> String {
        request(address, "GET", path).await
    }

    /// The body of a response, i.e. everything after the header terminator.
    fn body(response: &str) -> &str {
        response.split_once("\r\n\r\n").map_or("", |(_, rest)| rest)
    }

    #[tokio::test]
    async fn total_supply_is_a_bare_number() {
        // The aggregators parse the whole body as a decimal. A JSON wrapper, a
        // unit suffix, or a trailing newline would each break that parse.
        let (server, _dir) = start(MarketFeed::empty()).await;

        let response = get(server.address, "/api/v1/total_supply").await;
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("text/plain"));

        let value = body(&response);
        assert!(
            value.parse::<u64>().is_ok(),
            "not parseable as a number: {value:?}"
        );
    }

    #[tokio::test]
    async fn circulating_supply_is_a_bare_number() {
        let (server, _dir) = start(MarketFeed::empty()).await;

        let response = get(server.address, "/api/v1/circulating_supply").await;
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(body(&response).parse::<u64>().is_ok());
    }

    #[tokio::test]
    async fn the_structured_supply_endpoint_names_every_component() {
        let (server, _dir) = start(MarketFeed::empty()).await;

        let response = get(server.address, "/api/v1/supply").await;
        assert!(response.starts_with("HTTP/1.1 200"));
        for field in ["total", "circulating", "shielded", "burned"] {
            assert!(response.contains(field), "missing {field}");
        }
    }

    #[tokio::test]
    async fn total_supply_sums_the_funded_accounts() {
        // The zero-supply cases above would pass against a handler that always
        // answered "0"; this one would not.
        let (server, _dir) = start_funded(
            MarketFeed::empty(),
            &[([1u8; 32], 400), ([2u8; 32], 350), ([3u8; 32], 250)],
        )
        .await;

        let response = get(server.address, "/api/v1/total_supply").await;
        assert_eq!(body(&response), "1000");
    }

    #[tokio::test]
    async fn circulating_supply_excludes_the_fee_sink() {
        // Fees burned to the sink are unspendable — nobody holds its key — so
        // counting them would inflate the circulating figure forever.
        let (server, _dir) = start_funded(
            MarketFeed::empty(),
            &[([1u8; 32], 900), (crate::state::shielded::FEE_SINK, 100)],
        )
        .await;

        assert_eq!(
            body(&get(server.address, "/api/v1/total_supply").await),
            "1000"
        );
        assert_eq!(
            body(&get(server.address, "/api/v1/circulating_supply").await),
            "900"
        );
    }

    #[tokio::test]
    async fn an_unconfigured_feed_answers_503_rather_than_an_empty_market() {
        // An empty array reads as "trades nowhere", which an aggregator may
        // cache. 503 reads as "ask again".
        let (server, _dir) = start(MarketFeed::empty()).await;

        for path in ["/api/v1/ticker", "/api/v1/cmc/ticker"] {
            let response = get(server.address, path).await;
            assert!(
                response.starts_with("HTTP/1.1 503"),
                "{path} answered: {response}"
            );
        }
    }

    #[tokio::test]
    async fn a_configured_feed_is_translated_into_each_aggregator_shape() {
        let (server, _dir) = start(MarketFeed::with_quotes(vec![quote("MAYA_USDT")])).await;

        let coingecko = get(server.address, "/api/v1/ticker").await;
        assert!(coingecko.starts_with("HTTP/1.1 200"));
        assert!(coingecko.contains("base_currency"));
        assert!(coingecko.contains("target_volume"));

        let cmc = get(server.address, "/api/v1/cmc/ticker").await;
        assert!(cmc.starts_with("HTTP/1.1 200"));
        assert!(cmc.contains("base_id"));
        assert!(cmc.contains("quote_volume"));
    }

    #[tokio::test]
    async fn repeated_reads_at_one_height_agree() {
        // The cache keys on height, so it must not be able to serve a figure
        // that disagrees with an uncached read.
        let (server, _dir) = start(MarketFeed::empty()).await;

        let first = get(server.address, "/api/v1/total_supply").await;
        let second = get(server.address, "/api/v1/total_supply").await;
        assert_eq!(body(&first), body(&second));
    }

    #[tokio::test]
    async fn unknown_paths_are_not_found() {
        let (server, _dir) = start(MarketFeed::empty()).await;

        assert!(get(server.address, "/").await.starts_with("HTTP/1.1 404"));
        assert!(
            get(server.address, "/api/v1")
                .await
                .starts_with("HTTP/1.1 404")
        );
        assert!(
            get(server.address, "/metrics")
                .await
                .starts_with("HTTP/1.1 404")
        );
    }

    #[tokio::test]
    async fn writes_are_refused() {
        let (server, _dir) = start(MarketFeed::empty()).await;

        let response = request(server.address, "POST", "/api/v1/total_supply").await;
        assert!(response.starts_with("HTTP/1.1 405"));
    }

    #[tokio::test]
    async fn healthz_answers() {
        let (server, _dir) = start(MarketFeed::empty()).await;
        assert!(
            get(server.address, "/healthz")
                .await
                .starts_with("HTTP/1.1 200")
        );
    }

    #[tokio::test]
    async fn the_endpoints_are_readable_from_a_browser() {
        // Block explorers fetch these client-side; without the header the data
        // is only usable through a proxy.
        let (server, _dir) = start(MarketFeed::empty()).await;

        let response = get(server.address, "/api/v1/total_supply").await;
        assert!(
            response
                .to_lowercase()
                .contains("access-control-allow-origin: *")
        );
    }

    #[tokio::test]
    async fn the_server_survives_many_requests() {
        let (server, _dir) = start(MarketFeed::empty()).await;

        for _ in 0..25 {
            assert!(
                get(server.address, "/api/v1/total_supply")
                    .await
                    .starts_with("HTTP/1.1 200")
            );
        }
    }
}
