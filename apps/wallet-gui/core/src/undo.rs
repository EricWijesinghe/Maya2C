//! Undo-send: a signed transfer waits a short window before it is broadcast,
//! and the person can cancel it inside that window.
//!
//! Why a wallet-side hold and not a vault (ADR-030): a vault Request turns every
//! send from the account into an escrow that is paid only when someone submits
//! `Execute` after the delay, and it changes the account type. A hold needs no
//! node change. The signed bytes never reach a node until the window closes, so
//! a cancel means nothing was ever broadcast. The limit: the hold lives in this
//! wallet's process, so closing the wallet discards the held send.

use thiserror::Error;
use zeroize::Zeroize;

use crate::payment::SignedTransfer;

/// Seconds between signing and broadcast. The confirm screen counts this down.
pub const UNDO_WINDOW_SECS: u64 = 10;

/// Why [`UndoHold::release`] handed nothing over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum UndoError {
    /// Cancelled, already released, or never held.
    #[error("nothing is held to send")]
    NothingHeld,
    /// The window is still open.
    #[error("the send is held for {remaining_secs} more seconds")]
    NotDue { remaining_secs: u64 },
}

/// One signed transfer held before broadcast.
///
/// Holding the transfer in an `Option` makes both guarantees structural: a
/// cancel takes it out and zeroizes its bytes, and a release takes it out once,
/// so there is no second broadcast to get wrong.
pub struct UndoHold {
    transfer: Option<SignedTransfer>,
    release_at: u64,
}

impl UndoHold {
    /// Holds `transfer` from `now_unix` until `now_unix + window_secs`.
    #[must_use]
    pub fn new(transfer: SignedTransfer, now_unix: u64, window_secs: u64) -> Self {
        Self {
            transfer: Some(transfer),
            release_at: now_unix.saturating_add(window_secs),
        }
    }

    /// The Unix second from which [`UndoHold::release`] hands the transfer over.
    #[must_use]
    pub const fn release_at(&self) -> u64 {
        self.release_at
    }

    /// Whether a transfer is still waiting to be sent or cancelled.
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.transfer.is_some()
    }

    /// Hands the transfer over once the window has closed. Consumes it: a
    /// second call returns [`UndoError::NothingHeld`].
    ///
    /// # Errors
    ///
    /// [`UndoError::NotDue`] before the window closes, and
    /// [`UndoError::NothingHeld`] after a cancel or an earlier release.
    pub fn release(&mut self, now_unix: u64) -> Result<SignedTransfer, UndoError> {
        if self.transfer.is_none() {
            return Err(UndoError::NothingHeld);
        }
        if now_unix < self.release_at {
            return Err(UndoError::NotDue {
                remaining_secs: self.release_at - now_unix,
            });
        }
        self.transfer.take().ok_or(UndoError::NothingHeld)
    }

    /// Discards the held transfer and zeroizes its signed bytes. Returns
    /// whether anything was held.
    pub fn cancel(&mut self) -> bool {
        match self.transfer.take() {
            Some(mut transfer) => {
                transfer.raw_hex.zeroize();
                true
            }
            None => false,
        }
    }
}

impl Drop for UndoHold {
    fn drop(&mut self) {
        if let Some(transfer) = self.transfer.as_mut() {
            transfer.raw_hex.zeroize();
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use primitive_types::U256;

    use super::*;

    fn sample() -> SignedTransfer {
        SignedTransfer {
            txid: "ab".repeat(32),
            sender: "cd".repeat(32),
            recipient: "ef".repeat(32),
            amount: U256::from(1_u64),
            nonce: 7,
            fee: U256::from(2_u64),
            raw_hex: "deadbeef".to_string(),
        }
    }

    #[test]
    fn cancel_within_window_never_releases() {
        // Arrange
        let mut hold = UndoHold::new(sample(), 100, UNDO_WINDOW_SECS);
        let mut broadcast = Vec::new();
        // Act: poll through the window, then cancel inside it.
        for now in [100, 105, 109] {
            if let Ok(transfer) = hold.release(now) {
                broadcast.push(transfer);
            }
        }
        let cancelled = hold.cancel();
        for now in [110, 111, 500] {
            if let Ok(transfer) = hold.release(now) {
                broadcast.push(transfer);
            }
        }
        // Assert
        assert!(cancelled);
        assert!(broadcast.is_empty(), "a cancelled send must never be released");
        assert!(!hold.is_held());
    }

    #[test]
    fn releases_exactly_once_after_window() {
        // Arrange
        let mut hold = UndoHold::new(sample(), 100, UNDO_WINDOW_SECS);
        // Act
        let first = hold.release(110);
        let second = hold.release(111);
        let third = hold.release(999);
        // Assert
        assert_eq!(first.unwrap().nonce, 7);
        assert_eq!(second, Err(UndoError::NothingHeld));
        assert_eq!(third, Err(UndoError::NothingHeld));
    }

    #[test]
    fn early_release_keeps_the_hold_and_reports_time_left() {
        // Arrange
        let mut hold = UndoHold::new(sample(), 100, UNDO_WINDOW_SECS);
        // Act
        let early = hold.release(107);
        // Assert
        assert_eq!(early, Err(UndoError::NotDue { remaining_secs: 3 }));
        assert!(hold.is_held(), "an early poll must not consume the hold");
        assert!(hold.release(110).is_ok());
    }

    #[test]
    fn cancel_after_release_reports_nothing_held() {
        // Arrange
        let mut hold = UndoHold::new(sample(), 0, 0);
        // Act
        let sent = hold.release(0).is_ok();
        let cancelled = hold.cancel();
        // Assert
        assert!(sent);
        assert!(!cancelled);
    }

    #[test]
    fn window_end_saturates_instead_of_wrapping() {
        // Arrange / Act
        let hold = UndoHold::new(sample(), u64::MAX - 1, UNDO_WINDOW_SECS);
        // Assert
        assert_eq!(hold.release_at(), u64::MAX);
    }
}
