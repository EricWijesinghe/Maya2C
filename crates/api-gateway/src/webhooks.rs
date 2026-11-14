//! Webhook subscriptions: the gateway POSTs matching chain events to a URL.
//!
//! # Signed with a secret only the subscriber holds
//!
//! Registration returns a 32-byte secret, **once**. Every delivery carries
//! [`SIGNATURE_HEADER`]: `sha256=` and the hex HMAC-SHA256 of the exact body
//! bytes under that secret. A receiver recomputes it ([`verify`]) and drops
//! anything that does not match, so a third party who learns the URL cannot
//! forge an event. The body carries the event id, which a receiver uses to
//! drop a retry it already processed.
//!
//! The secret lives in a [`WebhookSecret`]: zeroized on drop, `Debug` redacted,
//! never logged. It is also the credential for reading or deleting the
//! subscription (`Authorization: Bearer <secret>`).
//!
//! # Delivery
//!
//! One task per subscription reads the event feed in order. A failed delivery
//! is retried with exponential backoff; after the last attempt the
//! subscription enters the dead-letter state, keeps the undelivered event's id
//! and the last error for its owner to read, and stops. Deleting it frees the
//! caller's slot.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use reqwest::Url;
use serde::Serialize;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::error::GatewayError;
use crate::ssrf;

/// The header a delivery's signature travels in.
pub const SIGNATURE_HEADER: &str = "x-maya2c-signature";

/// How long one delivery may take, connection included.
pub const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

const SECRET_LEN: usize = 32;

type HmacSha256 = Hmac<Sha256>;

/// A subscription's signing secret.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct WebhookSecret([u8; SECRET_LEN]);

impl std::fmt::Debug for WebhookSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebhookSecret(redacted)")
    }
}

impl WebhookSecret {
    /// A fresh secret from the operating system's generator.
    ///
    /// # Errors
    ///
    /// [`GatewayError::Upstream`] if the generator fails; nothing is issued
    /// with a weak secret.
    pub fn generate() -> Result<Self, GatewayError> {
        let mut bytes = [0u8; SECRET_LEN];
        getrandom::fill(&mut bytes)
            .map_err(|_| GatewayError::Upstream("entropy unavailable".to_string()))?;
        Ok(Self(bytes))
    }

    /// The secret as hex, for the one response that returns it.
    #[must_use]
    pub fn to_hex(&self) -> Zeroizing<String> {
        Zeroizing::new(hex::encode(self.0))
    }

    /// Whether `presented` (hex) is this secret. Compares digests, so the
    /// comparison's timing says nothing about the secret's bytes.
    #[must_use]
    pub fn matches(&self, presented: &str) -> bool {
        Sha256::digest(presented.as_bytes()) == Sha256::digest(self.to_hex().as_bytes())
    }
}

/// HMAC-SHA256 of `body` under `key`, as hex.
#[must_use]
pub fn mac_hex(key: &[u8], body: &[u8]) -> String {
    let mut mac = keyed(key);
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

fn keyed(key: &[u8]) -> HmacSha256 {
    // HMAC takes a key of any length; `new_from_slice` cannot fail for it.
    #[allow(clippy::expect_used)]
    HmacSha256::new_from_slice(key).expect("HMAC accepts any key length")
}

/// The [`SIGNATURE_HEADER`] value for `body`.
#[must_use]
pub fn sign(secret: &WebhookSecret, body: &[u8]) -> String {
    format!("sha256={}", mac_hex(&secret.0, body))
}

/// What a receiver runs: whether `header` is the signature of `body` under
/// the hex secret registration returned. Constant-time in the MAC.
#[must_use]
pub fn verify(secret_hex: &str, body: &[u8], header: &str) -> bool {
    let Ok(key) = hex::decode(secret_hex).map(Zeroizing::new) else {
        return false;
    };
    let Some(Ok(tag)) = header.strip_prefix("sha256=").map(hex::decode) else {
        return false;
    };
    let mut mac = keyed(&key);
    mac.update(body);
    mac.verify_slice(&tag).is_ok()
}

/// Where deliveries go. A trait so delivery, retry and dead-lettering are
/// tested without a network; [`HttpTransport`] is the real one.
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// POSTs `body` with its signature. `Err` carries a short reason for the
    /// subscription's owner; it never contains the secret.
    async fn post(&self, url: &Url, body: Vec<u8>, signature: String) -> Result<(), String>;

    /// Checked at registration, after [`ssrf::check_url`]. The real transport
    /// resolves the name and refuses a private answer early; deliveries
    /// re-check regardless.
    ///
    /// # Errors
    ///
    /// Why the target is refused.
    async fn admit(&self, _url: &Url) -> Result<(), GatewayError> {
        Ok(())
    }
}

/// Delivers over HTTPS, re-running the SSRF check on every delivery.
pub struct HttpTransport {
    allow_http: bool,
}

impl HttpTransport {
    /// `allow_http` is the dev flag: plain `http` targets. Address rules hold
    /// either way.
    #[must_use]
    pub const fn new(allow_http: bool) -> Self {
        Self { allow_http }
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn post(&self, url: &Url, body: Vec<u8>, signature: String) -> Result<(), String> {
        let url = ssrf::check_url(url.as_str(), self.allow_http).map_err(|e| e.to_string())?;
        let address = ssrf::resolve_public(&url)
            .await
            .map_err(|e| e.to_string())?;
        let host = url.host_str().unwrap_or_default();
        // shortcut: a client per delivery, so the pinned address is this
        // delivery's; pool per subscription if delivery volume ever matters.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(DELIVERY_TIMEOUT)
            .resolve(host, address)
            .build()
            .map_err(|_| "could not build the HTTP client".to_string())?;
        let response = client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(SIGNATURE_HEADER, signature)
            .body(body)
            .send()
            .await
            .map_err(|e| format!("delivery failed: {}", e.without_url()))?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(format!("receiver answered {}", status.as_u16()))
        }
    }

    async fn admit(&self, url: &Url) -> Result<(), GatewayError> {
        ssrf::resolve_public(url).await.map(|_| ())
    }
}

/// Where a subscription stands, as its owner reads it.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Status {
    /// `active` or `dead_letter`.
    pub state: &'static str,
    /// Events delivered.
    pub delivered: u64,
    /// Attempts that failed, including ones a retry later recovered.
    pub failed_attempts: u64,
    /// Events skipped because this subscription fell behind the feed.
    pub skipped: u64,
    /// The most recent failure.
    pub last_error: Option<String>,
    /// The event that exhausted its retries, once dead-lettered.
    pub dead_letter: Option<DeadLetter>,
}

/// The event a dead-lettered subscription could not deliver.
#[derive(Clone, Debug, Serialize)]
pub struct DeadLetter {
    /// Its event id.
    pub event: String,
}

/// One registered webhook.
pub struct Webhook {
    /// Public identifier.
    pub id: String,
    /// The target, as accepted at registration.
    pub url: Url,
    /// Which events it receives.
    pub filter: crate::events::Filter,
    /// Who registered it, for the per-caller cap.
    pub owner: std::net::IpAddr,
    pub(crate) secret: WebhookSecret,
    pub(crate) status: Mutex<Status>,
}

impl Webhook {
    /// A snapshot of its status.
    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn update(&self, change: impl FnOnce(&mut Status)) {
        change(&mut self.status.lock().unwrap_or_else(PoisonError::into_inner));
    }

    /// Delivers `body`, retrying with backoff. `false` once every attempt
    /// failed, after which the subscription is dead-lettered.
    pub(crate) async fn deliver(
        &self,
        transport: &dyn Transport,
        event_id: &str,
        body: Vec<u8>,
        retry: &Retry,
    ) -> bool {
        let signature = sign(&self.secret, &body);
        for attempt in 1..=retry.max_attempts {
            match transport
                .post(&self.url, body.clone(), signature.clone())
                .await
            {
                Ok(()) => {
                    self.update(|s| s.delivered += 1);
                    return true;
                }
                Err(error) => {
                    tracing::debug!(webhook = %self.id, attempt, "delivery failed");
                    self.update(|s| {
                        s.failed_attempts += 1;
                        s.last_error = Some(error);
                    });
                    if attempt < retry.max_attempts {
                        tokio::time::sleep(retry.backoff(attempt)).await;
                    }
                }
            }
        }
        tracing::info!(webhook = %self.id, event = event_id, "webhook dead-lettered");
        self.update(|s| {
            s.state = "dead_letter";
            s.dead_letter = Some(DeadLetter {
                event: event_id.to_string(),
            });
        });
        false
    }
}

/// Retry policy.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Retry {
    pub max_attempts: u32,
    pub base: Duration,
    pub max: Duration,
}

impl Retry {
    /// Wait after failed attempt `attempt` (1-based): `base * 2^(attempt-1)`,
    /// capped at `max`.
    fn backoff(&self, attempt: u32) -> Duration {
        let factor = 1u32
            .checked_shl(attempt.saturating_sub(1))
            .unwrap_or(u32::MAX);
        self.base.saturating_mul(factor).min(self.max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_is_capped() {
        let retry = Retry {
            max_attempts: 5,
            base: Duration::from_secs(1),
            max: Duration::from_secs(5),
        };
        assert_eq!(retry.backoff(1), Duration::from_secs(1));
        assert_eq!(retry.backoff(2), Duration::from_secs(2));
        assert_eq!(retry.backoff(3), Duration::from_secs(4));
        assert_eq!(retry.backoff(4), Duration::from_secs(5));
        assert_eq!(retry.backoff(64), Duration::from_secs(5));
    }
}
