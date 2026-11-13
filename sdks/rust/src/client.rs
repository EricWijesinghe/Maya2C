//! The gateway client.
//!
//! Retries are GET-only. A POST that timed out may still have reached the
//! mempool, and sending it again is a second submission the caller never asked
//! for. Only the 429 and 503 answers, which the gateway sends before doing any
//! work, are retried.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::thread;
use std::time::{Duration, Instant};

use reqwest::blocking::{RequestBuilder, Response};
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::address::to_hex;
use crate::error::{ApiError, Error};
use crate::types::{
    Balance, ChainInfo, FeeInfo, Health, RawTransaction, SealedAccepted, SealedSubmission, Supply,
    TransactionAccepted,
};

/// Default per-request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// Retries after the first attempt. Four attempts in total, at most.
const MAX_RETRIES: u32 = 3;
/// First backoff when the gateway sends no `Retry-After`; doubles per retry.
const BASE_BACKOFF: Duration = Duration::from_millis(200);
/// A `Retry-After` longer than this is not waited out: the call fails with
/// [`Error::RateLimited`] so the caller decides, rather than blocking a thread
/// for minutes.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);
/// How often `wait_for_transaction` asks the chain for new blocks.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// User-Agent sent on every request.
const USER_AGENT: &str = concat!("maya2c-sdk-rust/", env!("CARGO_PKG_VERSION"));

/// A client for one API gateway.
#[derive(Debug, Clone)]
pub struct Client {
    base: Url,
    http: reqwest::blocking::Client,
}

impl Client {
    /// A client for the gateway at `base_url`, with a 10 s timeout.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] if `base_url` is not an HTTP(S) URL.
    pub fn new(base_url: &str) -> Result<Self, Error> {
        Self::with_timeout(base_url, DEFAULT_TIMEOUT)
    }

    /// As [`Client::new`], with an explicit per-request timeout.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] if `base_url` is not an HTTP(S) URL.
    pub fn with_timeout(base_url: &str, timeout: Duration) -> Result<Self, Error> {
        let base = Url::parse(base_url).map_err(|e| Error::Config(e.to_string()))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(Error::Config(format!(
                "scheme {:?} is not http(s)",
                base.scheme()
            )));
        }
        let http = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| Error::Config(e.to_string()))?;
        Ok(Self { base, http })
    }

    fn url(&self, path: &str) -> Result<Url, Error> {
        self.base
            .join(path)
            .map_err(|e| Error::Config(format!("bad path {path}: {e}")))
    }

    /// `GET /health`.
    ///
    /// # Errors
    ///
    /// Any [`Error`] from the request.
    pub fn health(&self) -> Result<Health, Error> {
        self.get("health")
    }

    /// `GET /v1/chain`.
    ///
    /// # Errors
    ///
    /// Any [`Error`] from the request.
    pub fn chain_info(&self) -> Result<ChainInfo, Error> {
        self.get("v1/chain")
    }

    /// `GET /v1/fees`.
    ///
    /// # Errors
    ///
    /// Any [`Error`] from the request.
    pub fn fee_info(&self) -> Result<FeeInfo, Error> {
        self.get("v1/fees")
    }

    /// `GET /v1/supply`.
    ///
    /// # Errors
    ///
    /// Any [`Error`] from the request.
    pub fn supply(&self) -> Result<Supply, Error> {
        self.get("v1/supply")
    }

    /// `GET /v1/accounts/{address}`. Accepts 64 hex characters or Bech32m; the
    /// address is checked before any request is sent.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidAddress`] for a bad address, otherwise any request error.
    pub fn balance(&self, address: &str) -> Result<Balance, Error> {
        let hex = to_hex(address)?;
        self.get(&format!("v1/accounts/{hex}"))
    }

    /// `GET /v1/blocks/{height}`. The body is the node's own block JSON, which
    /// the gateway does not version, so it is returned as parsed JSON.
    ///
    /// # Errors
    ///
    /// Any [`Error`] from the request; [`Error::Api`] with status 404 for an
    /// unknown or pruned height.
    pub fn block(&self, height: u64) -> Result<serde_json::Value, Error> {
        self.get(&format!("v1/blocks/{height}"))
    }

    /// `POST /v1/transactions`. Returns the hash once the gateway has accepted
    /// the transaction into its mempool; inclusion in a block is a separate
    /// fact, see [`Client::wait_for_transaction`].
    ///
    /// Never retried.
    ///
    /// # Errors
    ///
    /// Any [`Error`] from the request.
    pub fn send_raw_transaction(&self, raw_hex: &str) -> Result<String, Error> {
        let accepted: TransactionAccepted =
            self.post("v1/transactions", &RawTransaction { raw: raw_hex })?;
        Ok(accepted.hash)
    }

    /// `POST /v1/sealed`: a threshold-encrypted transaction. Never retried.
    ///
    /// # Errors
    ///
    /// Any [`Error`] from the request.
    pub fn send_sealed_transaction(&self, ciphertext_hex: &str) -> Result<SealedAccepted, Error> {
        self.post(
            "v1/sealed",
            &SealedSubmission {
                ciphertext: ciphertext_hex,
            },
        )
    }

    /// Waits until the transaction `hash` appears in a block, and returns that
    /// block's height.
    ///
    /// The scan starts at the chain height at the time of the call, so a
    /// transaction already included before the call is not reported. Callers
    /// submit and then wait; that is the case this serves.
    ///
    /// # Errors
    ///
    /// [`Error::Timeout`] if no block carries it before `timeout`; any request
    /// error otherwise (a pruned block stops the scan with its 404).
    pub fn wait_for_transaction(&self, hash: &str, timeout: Duration) -> Result<u64, Error> {
        let wanted = hash.strip_prefix("0x").unwrap_or(hash).to_ascii_lowercase();
        let deadline = Instant::now() + timeout;
        let mut next = self.chain_info()?.height;
        loop {
            let tip = self.chain_info()?.height;
            while next <= tip {
                if block_contains(&self.block(next)?, &wanted)? {
                    return Ok(next);
                }
                next += 1;
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
        }
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        let url = self.url(path)?;
        let mut retries = 0;
        loop {
            let response = dispatch(self.http.get(url.clone()))?;
            if is_retryable(response.status()) && retries < MAX_RETRIES {
                let Some(wait) = retry_wait(retries, retry_after(&response)) else {
                    return decode(response);
                };
                retries += 1;
                thread::sleep(wait);
                continue;
            }
            return decode(response);
        }
    }

    fn post<B: serde::Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, Error> {
        let url = self.url(path)?;
        decode(dispatch(self.http.post(url).json(body))?)
    }
}

/// Sends one attempt, mapping transport failures onto [`Error`].
fn dispatch(request: RequestBuilder) -> Result<Response, Error> {
    request.send().map_err(|e| {
        if e.is_timeout() {
            Error::Timeout
        } else {
            Error::Network(e.to_string())
        }
    })
}

const fn is_retryable(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
    )
}

/// Parses `Retry-After` as delta-seconds. The HTTP-date form is not sent by
/// this gateway and is ignored, which falls back to the backoff schedule.
fn retry_after(response: &Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// How long to wait before retry number `attempt` (0-based), or `None` when
/// the gateway asks for longer than [`MAX_RETRY_AFTER`].
fn retry_wait(attempt: u32, retry_after: Option<Duration>) -> Option<Duration> {
    let base = match retry_after {
        Some(asked) if asked > MAX_RETRY_AFTER => return None,
        Some(asked) => asked,
        None => BASE_BACKOFF.saturating_mul(2u32.saturating_pow(attempt)),
    };
    Some(base + jitter(base / 4))
}

/// A random duration in `0..=bound`. Randomness here only spreads retries out;
/// it is not security-relevant, so the std hasher seed is enough.
fn jitter(bound: Duration) -> Duration {
    let bound_ms = u64::try_from(bound.as_millis()).unwrap_or(u64::MAX);
    if bound_ms == 0 {
        return Duration::ZERO;
    }
    let roll = RandomState::new().build_hasher().finish();
    Duration::from_millis(roll % (bound_ms + 1))
}

/// Turns a response into its body, or into the matching [`Error`].
fn decode<T: DeserializeOwned>(response: Response) -> Result<T, Error> {
    let status = response.status();
    let retry = retry_after(&response);
    let text = response.text().map_err(|e| Error::Network(e.to_string()))?;
    if status.is_success() {
        return serde_json::from_str(&text).map_err(|e| Error::Decode(e.to_string()));
    }
    let error = api_error(status, retry, &text);
    if status == StatusCode::TOO_MANY_REQUESTS {
        Err(Error::RateLimited(error))
    } else {
        Err(Error::Api(error))
    }
}

#[derive(Deserialize)]
struct ErrorBody {
    error: Option<String>,
    code: Option<String>,
}

/// Builds an [`ApiError`] from a failure body. A body that is not the
/// gateway's `{"error": ...}` shape keeps its raw text as the detail.
fn api_error(status: StatusCode, retry_after: Option<Duration>, text: &str) -> ApiError {
    let parsed: Option<ErrorBody> = serde_json::from_str(text).ok();
    let (code, detail) = match parsed {
        Some(body) => (body.code, body.error.unwrap_or_else(|| text.to_string())),
        None => (None, text.to_string()),
    };
    ApiError {
        status: status.as_u16(),
        code: code.unwrap_or_else(|| {
            status
                .canonical_reason()
                .unwrap_or("unknown")
                .to_ascii_lowercase()
                .replace(' ', "_")
        }),
        detail,
        retry_after,
    }
}

/// Whether a block's JSON lists `wanted` among its transaction ids.
fn block_contains(block: &serde_json::Value, wanted: &str) -> Result<bool, Error> {
    #[derive(Deserialize)]
    struct Txs {
        #[serde(default)]
        transactions: Vec<Tx>,
    }
    #[derive(Deserialize)]
    struct Tx {
        txid: String,
    }
    let txs: Txs =
        serde_json::from_value(block.clone()).map_err(|e| Error::Decode(e.to_string()))?;
    Ok(txs
        .transactions
        .iter()
        .any(|t| t.txid.eq_ignore_ascii_case(wanted)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_retry_after_is_waited_out_and_jitter_stays_inside_a_quarter() {
        let wait = retry_wait(0, Some(Duration::from_secs(2))).expect("short enough");
        assert!(wait >= Duration::from_secs(2));
        assert!(wait <= Duration::from_millis(2500));
    }

    #[test]
    fn a_long_retry_after_is_not_waited_out() {
        assert_eq!(retry_wait(0, Some(Duration::from_secs(600))), None);
    }

    #[test]
    fn backoff_doubles_per_attempt() {
        let first = retry_wait(0, None).unwrap();
        let third = retry_wait(2, None).unwrap();
        assert!(first >= BASE_BACKOFF && first <= BASE_BACKOFF * 5 / 4);
        assert!(third >= BASE_BACKOFF * 4 && third <= BASE_BACKOFF * 5);
    }

    #[test]
    fn a_non_http_base_url_is_refused() {
        assert!(matches!(Client::new("ftp://x"), Err(Error::Config(_))));
        assert!(matches!(Client::new("not a url"), Err(Error::Config(_))));
    }

    #[test]
    fn gateway_error_body_becomes_detail_and_status_reason_becomes_code() {
        let error = api_error(StatusCode::BAD_REQUEST, None, r#"{"error":"raw is empty"}"#);
        assert_eq!(error.detail, "raw is empty");
        assert_eq!(error.code, "bad_request");
        assert_eq!(error.status, 400);
    }

    #[test]
    fn block_scan_finds_a_txid_case_insensitively() {
        let block = serde_json::json!({"transactions":[{"txid":"AB12"},{"txid":"cd34"}]});
        assert!(block_contains(&block, "ab12").unwrap());
        assert!(!block_contains(&block, "ffff").unwrap());
    }
}
