//! Signed webhooks: HMAC-SHA256 over `timestamp.nonce.body`, plus the
//! receiver-side checks a shop must run (freshness, replay).
//!
//! The signed string includes the timestamp and a fresh nonce, so a captured
//! request cannot be replayed later (timestamp window) or twice within the
//! window (nonce cache). The secret never leaves this process: it is held in a
//! zeroizing buffer and never printed.

use std::collections::BTreeMap;

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::{CheckoutError, Result};

/// Header carrying the Unix time the request was signed at.
pub const HEADER_TIMESTAMP: &str = "maya-timestamp";
/// Header carrying a random nonce, unique per delivery attempt.
pub const HEADER_NONCE: &str = "maya-nonce";
/// Header carrying `sha256=<hex>`.
pub const HEADER_SIGNATURE: &str = "maya-signature";
/// How far a timestamp may drift from the receiver's clock, in seconds.
pub const TOLERANCE_SECS: u64 = 300;
/// Shortest secret accepted at startup, in bytes (256 bits).
pub const MIN_SECRET_BYTES: usize = 32;
/// Delivery attempts before a webhook is marked failed.
pub const MAX_ATTEMPTS: u32 = 8;
/// Delay after the first failed attempt, in seconds. Doubles per attempt.
const BACKOFF_BASE_SECS: u64 = 30;
/// Longest delay between attempts, in seconds.
const BACKOFF_CAP_SECS: u64 = 3_600;
/// Jitter is at most one `1/JITTER_DIVISOR` of the delay (a quarter).
const JITTER_DIVISOR: u64 = 4;

/// Why a receiver should refuse a webhook.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    /// Timestamp outside the tolerance window, either way.
    #[error("timestamp outside the tolerance window")]
    Stale,
    /// The signature header is not `sha256=<hex>`.
    #[error("malformed signature")]
    Malformed,
    /// The signature does not match the body.
    #[error("signature mismatch")]
    Mismatch,
    /// This nonce was already accepted inside the window.
    #[error("nonce already seen")]
    Replay,
}

fn new_mac(secret: &[u8]) -> Result<Hmac<Sha256>> {
    Hmac::<Sha256>::new_from_slice(secret)
        .map_err(|e| CheckoutError::Internal(format!("hmac key: {e}")))
}

/// Signs one delivery. Returns the value for [`HEADER_SIGNATURE`].
///
/// # Errors
///
/// [`CheckoutError::Internal`] only if the HMAC key cannot be used.
pub fn sign(secret: &[u8], timestamp: u64, nonce: &str, body: &[u8]) -> Result<String> {
    let mut mac = new_mac(secret)?;
    mac.update(format!("{timestamp}.{nonce}.").as_bytes());
    mac.update(body);
    Ok(format!(
        "sha256={}",
        hex::encode(mac.finalize().into_bytes())
    ))
}

/// Receiver-side check: freshness first, then the signature in constant time.
///
/// # Errors
///
/// A [`VerifyError`] naming the first check that failed.
pub fn verify(
    secret: &[u8],
    timestamp: u64,
    nonce: &str,
    body: &[u8],
    signature_header: &str,
    now_unix: u64,
) -> std::result::Result<(), VerifyError> {
    if now_unix.abs_diff(timestamp) > TOLERANCE_SECS {
        return Err(VerifyError::Stale);
    }
    // A fixed-width nonce keeps `timestamp.nonce.body` unambiguous: a
    // variable-length nonce lets bytes move from the body into the nonce.
    if !is_nonce_shape(nonce) {
        return Err(VerifyError::Malformed);
    }
    let hex_part = signature_header
        .strip_prefix("sha256=")
        .ok_or(VerifyError::Malformed)?;
    let presented = hex::decode(hex_part).map_err(|_| VerifyError::Malformed)?;
    let mut mac = new_mac(secret).map_err(|_| VerifyError::Malformed)?;
    mac.update(format!("{timestamp}.{nonce}.").as_bytes());
    mac.update(body);
    // verify_slice compares in constant time.
    mac.verify_slice(&presented)
        .map_err(|_| VerifyError::Mismatch)
}

/// Remembers nonces accepted within the tolerance window, so a receiver can
/// refuse a second delivery with the same nonce.
#[derive(Debug, Default)]
pub struct NonceCache {
    /// Nonce to the timestamp it was signed with.
    seen: BTreeMap<String, u64>,
}

impl NonceCache {
    /// Accepts a nonce once. Entries older than the window are dropped first,
    /// which is safe because [`verify`] already rejects those timestamps.
    ///
    /// # Errors
    ///
    /// [`VerifyError::Replay`] if the nonce was seen inside the window.
    pub fn accept(
        &mut self,
        nonce: &str,
        timestamp: u64,
        now_unix: u64,
    ) -> std::result::Result<(), VerifyError> {
        self.seen
            .retain(|_, ts| now_unix.abs_diff(*ts) <= TOLERANCE_SECS);
        if self.seen.contains_key(nonce) {
            return Err(VerifyError::Replay);
        }
        self.seen.insert(nonce.to_owned(), timestamp);
        Ok(())
    }
}

/// Length of a nonce in hex characters: 128 random bits.
pub const NONCE_HEX_LEN: usize = 32;

/// Whether `nonce` has the shape [`new_nonce`] produces: exactly 32 lowercase hex.
#[must_use]
pub fn is_nonce_shape(nonce: &str) -> bool {
    nonce.len() == NONCE_HEX_LEN
        && nonce
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A fresh nonce: 128 random bits, hex.
///
/// # Errors
///
/// [`CheckoutError::Internal`] if the OS random source fails.
pub fn new_nonce() -> Result<String> {
    let mut bytes = [0u8; NONCE_HEX_LEN / 2];
    getrandom::fill(&mut bytes).map_err(|e| CheckoutError::Internal(format!("randomness: {e}")))?;
    Ok(hex::encode(bytes))
}

/// A random 64-bit value, for retry jitter.
///
/// # Errors
///
/// [`CheckoutError::Internal`] if the OS random source fails.
pub fn random_u64() -> Result<u64> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|e| CheckoutError::Internal(format!("randomness: {e}")))?;
    Ok(u64::from_le_bytes(bytes))
}

/// Seconds to wait after the `attempt`-th failed attempt (1-based).
#[must_use]
pub fn backoff_secs(attempt: u32) -> u64 {
    BACKOFF_BASE_SECS
        .saturating_mul(1u64 << attempt.min(16))
        .min(BACKOFF_CAP_SECS)
}

/// [`backoff_secs`] plus up to a quarter more, chosen by `seed`. Shops that
/// failed together then do not all retry in the same second.
#[must_use]
pub fn backoff_jittered_secs(attempt: u32, seed: u64) -> u64 {
    let base = backoff_secs(attempt);
    let spread = base / JITTER_DIVISOR + 1;
    base + seed % spread
}
