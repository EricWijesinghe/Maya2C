//! Cooling-off: a large send, or the first one to a payee, waits a delay the
//! person chose before it can be broadcast, and can be cancelled meanwhile.
//!
//! It is not a second hold. It only lengthens the [`UndoHold`] window, so the
//! cancel, the release-once and the zeroizing all stay in one place
//! (`undo.rs`). The limit is the same as undo-send's: the hold lives in this
//! wallet's process, and closing the wallet discards the held send. It guards
//! the person against their own haste and a pushy caller, not against a
//! compromised webview: `sign_transfer` already hands the signed bytes to it.
//!
//! [`UndoHold`]: crate::undo::UndoHold

use primitive_types::U256;
use serde::{Deserialize, Serialize};

use crate::error::{Result, WalletError};
use crate::undo::UNDO_WINDOW_SECS;

/// The longest delay a person may choose: one week. A typed extra zero should
/// not park a payment for months; cancel works at any time either way.
pub const MAX_COOLING_SECS: u64 = 7 * 24 * 60 * 60;

/// When a send waits longer than the undo window, and for how long.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoolingOff {
    /// A send of at least this many base units counts as large.
    #[serde(with = "custom_l1_node::state::amount_serde")]
    pub threshold: U256,
    /// The chosen delay in seconds; 0 turns cooling-off off (the default).
    pub delay_secs: u64,
}

impl CoolingOff {
    /// A policy with `delay_secs` at most [`MAX_COOLING_SECS`].
    ///
    /// # Errors
    ///
    /// [`WalletError::Format`] when the delay is above the cap.
    pub fn new(threshold: U256, delay_secs: u64) -> Result<Self> {
        if delay_secs > MAX_COOLING_SECS {
            return Err(WalletError::Format(format!(
                "a cooling-off delay can be at most {MAX_COOLING_SECS} seconds"
            )));
        }
        Ok(Self { threshold, delay_secs })
    }

    /// Whether this send waits the chosen delay: it is large or goes to a
    /// first-time payee, and cooling-off is on.
    #[must_use]
    pub fn applies(&self, amount: U256, first_time: bool) -> bool {
        self.delay_secs > 0 && (first_time || amount >= self.threshold)
    }

    /// Whether `next` protects less than `self`: a shorter delay or a higher
    /// threshold. The shell asks on a native dialog before such a change, so
    /// a compromised webview cannot quietly switch cooling-off off.
    #[must_use]
    pub fn weakened_by(&self, next: &Self) -> bool {
        // Off protects nothing, so no change from off weakens it.
        self.delay_secs > 0
            && (next.delay_secs < self.delay_secs || next.threshold > self.threshold)
    }

    /// The hold window for this send: the chosen delay when cooling-off
    /// applies, never shorter than [`UNDO_WINDOW_SECS`].
    #[must_use]
    pub fn window_secs(&self, amount: U256, first_time: bool) -> u64 {
        if self.applies(amount, first_time) {
            self.delay_secs.max(UNDO_WINDOW_SECS)
        } else {
            UNDO_WINDOW_SECS
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::payment::SignedTransfer;
    use crate::undo::{UndoError, UndoHold};

    const HOUR: u64 = 3_600;

    fn policy() -> CoolingOff {
        CoolingOff::new(U256::from(1_000_u64), HOUR).unwrap()
    }

    #[test]
    fn off_by_default_keeps_the_undo_window() {
        // Arrange
        let off = CoolingOff::default();
        // Act / Assert
        assert_eq!(off.window_secs(U256::MAX, true), UNDO_WINDOW_SECS);
    }

    #[test]
    fn large_send_waits_the_chosen_delay() {
        // Arrange / Act / Assert: the threshold itself counts as large.
        assert_eq!(policy().window_secs(U256::from(1_000_u64), false), HOUR);
        assert_eq!(policy().window_secs(U256::from(999_u64), false), UNDO_WINDOW_SECS);
    }

    #[test]
    fn first_time_payee_waits_even_when_small() {
        assert_eq!(policy().window_secs(U256::one(), true), HOUR);
    }

    #[test]
    fn delay_shorter_than_undo_window_never_shortens_it() {
        // Arrange
        let short = CoolingOff::new(U256::zero(), 1).unwrap();
        // Act / Assert
        assert_eq!(short.window_secs(U256::one(), false), UNDO_WINDOW_SECS);
    }

    #[test]
    fn shorter_delay_or_higher_threshold_counts_as_weakening() {
        // Arrange
        let now = policy();
        let off = CoolingOff::default();
        let higher = CoolingOff::new(U256::from(2_000_u64), HOUR).unwrap();
        let stricter = CoolingOff::new(U256::from(10_u64), 2 * HOUR).unwrap();
        // Act / Assert
        assert!(now.weakened_by(&off));
        assert!(now.weakened_by(&higher));
        assert!(!now.weakened_by(&stricter));
        assert!(!off.weakened_by(&now));
    }

    #[test]
    fn delay_above_the_cap_is_refused() {
        assert!(CoolingOff::new(U256::zero(), MAX_COOLING_SECS + 1).is_err());
        assert!(CoolingOff::new(U256::zero(), MAX_COOLING_SECS).is_ok());
    }

    #[test]
    fn cooling_off_composes_with_undo_hold_and_cancels() {
        // Arrange: a large send held for the cooling-off window.
        let transfer = SignedTransfer {
            txid: "ab".repeat(32),
            sender: "cd".repeat(32),
            recipient: "ef".repeat(32),
            amount: U256::from(5_000_u64),
            nonce: 1,
            fee: U256::one(),
            raw_hex: "deadbeef".to_string(),
        };
        let window = policy().window_secs(transfer.amount, false);
        let mut hold = UndoHold::new(transfer, 100, window);
        // Act: the undo window alone has passed, then the person cancels.
        let early = hold.release(100 + UNDO_WINDOW_SECS);
        let cancelled = hold.cancel();
        // Assert
        assert_eq!(
            early,
            Err(UndoError::NotDue { remaining_secs: HOUR - UNDO_WINDOW_SECS })
        );
        assert!(cancelled);
        assert_eq!(hold.release(100 + HOUR), Err(UndoError::NothingHeld));
    }
}
