//! Webhook threats: spoofing, tampering, stale and replayed deliveries, and
//! retry pacing.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_checkout::webhook::{
    MAX_ATTEMPTS, NonceCache, TOLERANCE_SECS, VerifyError, backoff_jittered_secs, backoff_secs,
    sign, verify,
};

const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";
const BODY: &[u8] = br#"{"event":"invoice.confirmed","amount":"500"}"#;
const NOW: u64 = 1_800_000_000;
/// Nonces are 128 random bits, hex: the shape `new_nonce` produces.
const N1: &str = "0123456789abcdef0123456789abcdef";
const N2: &str = "fedcba9876543210fedcba9876543210";

fn signed(timestamp: u64, nonce: &str, body: &[u8]) -> String {
    sign(SECRET, timestamp, nonce, body).unwrap()
}

#[test]
fn a_correct_signature_verifies() {
    let sig = signed(NOW, N1, BODY);
    assert_eq!(verify(SECRET, NOW, N1, BODY, &sig, NOW), Ok(()));
}

#[test]
fn spoofed_sender_without_the_secret_is_refused() {
    let forged = sign(b"an-attacker-guessed-key-of-length-32", NOW, N1, BODY).unwrap();
    assert_eq!(
        verify(SECRET, NOW, N1, BODY, &forged, NOW),
        Err(VerifyError::Mismatch)
    );
}

#[test]
fn a_changed_body_is_refused() {
    let sig = signed(NOW, N1, BODY);
    let tampered = br#"{"event":"invoice.confirmed","amount":"5000"}"#;
    assert_eq!(
        verify(SECRET, NOW, N1, tampered, &sig, NOW),
        Err(VerifyError::Mismatch)
    );
}

#[test]
fn a_changed_timestamp_or_nonce_breaks_the_signature() {
    let sig = signed(NOW, N1, BODY);
    assert_eq!(
        verify(SECRET, NOW + 1, N1, BODY, &sig, NOW + 1),
        Err(VerifyError::Mismatch)
    );
    assert_eq!(
        verify(SECRET, NOW, N2, BODY, &sig, NOW),
        Err(VerifyError::Mismatch)
    );
}

#[test]
fn a_nonce_that_is_not_32_hex_is_refused_even_when_the_signature_verifies() {
    // The signed string is `timestamp.nonce.body`. Moving bytes from the body
    // into the nonce leaves the signed bytes unchanged, so the signature still
    // verifies. Only the nonce's shape stops it from passing as a new delivery.
    let sig = signed(NOW, N1, BODY);
    let split = 5;
    let smuggled_nonce = format!("{N1}.{}", std::str::from_utf8(&BODY[..split]).unwrap());
    let rest = &BODY[split..];
    assert_eq!(
        verify(SECRET, NOW, &smuggled_nonce, rest, &sig, NOW),
        Err(VerifyError::Malformed)
    );
}

#[test]
fn a_delivery_outside_the_tolerance_window_is_stale() {
    let sig = signed(NOW, N1, BODY);
    assert_eq!(
        verify(SECRET, NOW, N1, BODY, &sig, NOW + TOLERANCE_SECS),
        Ok(())
    );
    assert_eq!(
        verify(SECRET, NOW, N1, BODY, &sig, NOW + TOLERANCE_SECS + 1),
        Err(VerifyError::Stale)
    );
    // A timestamp from the future is as suspect as an old one.
    let future = signed(NOW + TOLERANCE_SECS + 1, N1, BODY);
    assert_eq!(
        verify(SECRET, NOW + TOLERANCE_SECS + 1, N1, BODY, &future, NOW),
        Err(VerifyError::Stale)
    );
}

#[test]
fn malformed_signature_headers_are_refused() {
    assert_eq!(
        verify(SECRET, NOW, N1, BODY, "deadbeef", NOW),
        Err(VerifyError::Malformed)
    );
    assert_eq!(
        verify(SECRET, NOW, N1, BODY, "sha256=zz", NOW),
        Err(VerifyError::Malformed)
    );
    assert_eq!(
        verify(SECRET, NOW, N1, BODY, "", NOW),
        Err(VerifyError::Malformed)
    );
}

#[test]
fn a_replayed_nonce_inside_the_window_is_refused() {
    let mut cache = NonceCache::default();
    assert_eq!(cache.accept("n1", NOW, NOW), Ok(()));
    assert_eq!(cache.accept("n1", NOW, NOW + 10), Err(VerifyError::Replay));
    assert_eq!(cache.accept("n2", NOW, NOW + 10), Ok(()));
}

#[test]
fn the_nonce_cache_forgets_only_what_the_window_has_passed() {
    let mut cache = NonceCache::default();
    cache.accept("old", NOW, NOW).unwrap();
    // Once the window has passed, the signature check already rejects the
    // timestamp, so forgetting the nonce cannot reopen a replay.
    assert_eq!(cache.accept("old", NOW, NOW + TOLERANCE_SECS + 1), Ok(()));
}

#[test]
fn retry_delays_double_then_cap() {
    assert_eq!(backoff_secs(1), 60);
    assert_eq!(backoff_secs(2), 120);
    assert_eq!(backoff_secs(MAX_ATTEMPTS + 20), 3_600);
    assert!(backoff_secs(3) > backoff_secs(2));
}

#[test]
fn retry_delays_are_jittered_so_shops_do_not_retry_in_lockstep() {
    // Up to a quarter above the schedule, never below it, and not one value.
    let delays: std::collections::BTreeSet<u64> = (0..200u64)
        .map(|seed| backoff_jittered_secs(1, seed))
        .collect();
    assert!(delays.iter().all(|d| (60..=75).contains(d)), "{delays:?}");
    assert!(delays.len() > 1, "jitter produced one delay for every seed");
}
