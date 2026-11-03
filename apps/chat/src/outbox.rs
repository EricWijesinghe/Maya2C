//! The sender's outbox (ADR-074, decision 1): sealed envelopes kept until a
//! receipt arrives, retried with backoff. A retry resends the same bytes; the
//! ciphertext is never sealed again.

use serde::{Deserialize, Serialize};

use crate::relay::{Envelope, MAX_ENVELOPE_BYTES};
use crate::{Address, ChatError};

/// Most entries the outbox holds. Matches the relay's mailbox bound.
pub const MAX_ENTRIES: usize = 1_000;
/// Most sealed bytes the outbox holds.
pub const MAX_BYTES: usize = 16 << 20;
/// First retry delay, seconds; doubles up to [`RETRY_CAP`].
pub const RETRY_BASE: u64 = 5;
/// Longest gap between attempts, seconds.
pub const RETRY_CAP: u64 = 600;
/// Most doublings before the cap, so the shift cannot overflow.
const MAX_DOUBLINGS: u32 = 16;
/// Jitter on each delay, as a percentage either way.
pub const JITTER_PCT: i64 = 20;
/// A receipt waits up to this long, so it does not show the exact moment the
/// app was opened (ADR-074, decision 3).
pub const RECEIPT_DELAY_MAX: u64 = 30;

/// Most expired messages kept for the user to see and dismiss. Older ones
/// are dropped: expired entries never count against [`MAX_ENTRIES`].
pub const MAX_EXPIRED: usize = 100;

/// An expired, unreceipted message: what the user is shown after its TTL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expired {
    /// Recipient.
    pub to: Address,
    /// When it was first queued, Unix seconds.
    pub first_at: u64,
}

/// Where a sent message stands, as its sender sees it (ADR-074).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Queued and unreceipted. `relayed`: the relay has accepted an attempt.
    Queued { relayed: bool },
    /// Past its TTL with no receipt; kept in the expired record.
    Expired,
}

/// One sealed envelope awaiting its receipt, or a receipt awaiting relay.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pending {
    /// Recipient.
    pub to: Address,
    /// The sealed envelope. Its stamp is replaced only if the relay asks for
    /// more work, which keeps the id.
    pub envelope: Envelope,
    /// A receipt (sent once, no receipt expected) or a chat message.
    pub receipt: bool,
    /// Canonical size, for the byte bound.
    pub size: usize,
    /// When it was first queued, Unix seconds.
    pub first_at: u64,
    /// Attempts made so far.
    pub attempts: u32,
    /// Earliest next attempt, Unix seconds.
    pub next_at: u64,
    /// The relay has accepted it at least once. Shown as "relayed": a
    /// relayed message is still retried until its receipt.
    pub relayed: bool,
}

/// Delay before attempt `attempts + 1`, in seconds, given a jitter in
/// percent (`-JITTER_PCT..=JITTER_PCT`). Pure, so the schedule is testable.
#[must_use]
pub fn backoff(attempts: u32, jitter_pct: i64) -> u64 {
    let doublings = attempts.saturating_sub(1).min(MAX_DOUBLINGS);
    let base = RETRY_BASE.saturating_mul(1 << doublings).min(RETRY_CAP);
    let base = i64::try_from(base).unwrap_or(i64::MAX);
    let jitter = base * jitter_pct.clamp(-JITTER_PCT, JITTER_PCT) / 100;
    u64::try_from(base + jitter).unwrap_or(0)
}

/// A uniform value in `0..=max`, from the operating system.
///
/// # Errors
///
/// [`ChatError::Entropy`].
pub fn random_upto(max: u64) -> Result<u64, ChatError> {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).map_err(|_| ChatError::Entropy)?;
    Ok(u64::from_le_bytes(b) % (max + 1))
}

/// Jitter for [`backoff`], in `-JITTER_PCT..=JITTER_PCT`.
///
/// # Errors
///
/// [`ChatError::Entropy`].
pub fn jitter() -> Result<i64, ChatError> {
    let span = u64::try_from(2 * JITTER_PCT + 1).unwrap_or(1);
    let offset = i64::try_from(random_upto(span - 1)?).unwrap_or(0);
    Ok(offset - JITTER_PCT)
}

/// Refuses a new entry that would exceed the outbox's bounds. Old entries are
/// never evicted to make room (ADR-074: silent loss is what this prevents).
///
/// # Errors
///
/// [`ChatError::OutboxFull`] or [`ChatError::Refused`] for an oversized entry.
pub fn check_room(held: usize, held_bytes: usize, size: usize) -> Result<(), ChatError> {
    if size > MAX_ENVELOPE_BYTES {
        return Err(ChatError::Refused("envelope too large"));
    }
    if held >= MAX_ENTRIES || held_bytes + size > MAX_BYTES {
        return Err(ChatError::OutboxFull(held));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // a test that fails should panic, and say where
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_from_the_base_then_caps() {
        let schedule: Vec<u64> = (1..=8).map(|n| backoff(n, 0)).collect();
        assert_eq!(schedule, vec![5, 10, 20, 40, 80, 160, 320, 600]);
        assert_eq!(
            backoff(40, 0),
            RETRY_CAP,
            "a huge attempt count stays capped"
        );
    }

    #[test]
    fn backoff_jitter_stays_within_twenty_percent() {
        for attempts in 1..=8 {
            let base = backoff(attempts, 0);
            let low = backoff(attempts, -JITTER_PCT);
            let high = backoff(attempts, JITTER_PCT);
            assert_eq!(low, base * 80 / 100, "attempt {attempts}");
            assert_eq!(high, base * 120 / 100, "attempt {attempts}");
        }
        assert_eq!(
            backoff(1, 500),
            backoff(1, JITTER_PCT),
            "out-of-range jitter clamps"
        );
    }

    #[test]
    fn jitter_is_drawn_within_bounds() {
        for _ in 0..200 {
            let j = jitter().unwrap();
            assert!((-JITTER_PCT..=JITTER_PCT).contains(&j));
        }
    }

    #[test]
    fn a_full_outbox_refuses_and_an_oversized_entry_is_refused() {
        assert!(check_room(MAX_ENTRIES - 1, 0, 10).is_ok());
        assert_eq!(
            check_room(MAX_ENTRIES, 0, 10),
            Err(ChatError::OutboxFull(MAX_ENTRIES))
        );
        assert_eq!(
            check_room(0, MAX_BYTES - 5, 10),
            Err(ChatError::OutboxFull(0))
        );
        assert_eq!(
            check_room(0, 0, MAX_ENVELOPE_BYTES + 1),
            Err(ChatError::Refused("envelope too large"))
        );
    }
}
