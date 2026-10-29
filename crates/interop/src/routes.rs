//! Per-route value caps that grow with incident-free time.
//!
//! A route (a connection to another chain) may move at most `cap(now)` per
//! window. The cap starts at `base` and grows linearly with clean days, up
//! to `ceiling`. An incident — a failed proof, a paused relayer, a detected
//! forgery — resets the clean clock to zero. So a bug in a young route can
//! lose at most its small starting cap, and a route earns trust only by
//! running without trouble.

use maya_amount::Amount;

/// A route's limits and history.
#[derive(Clone, Debug)]
pub struct Route {
    /// Cap per window on day zero.
    pub base: Amount,
    /// Cap added per clean day.
    pub per_clean_day: Amount,
    /// Highest the cap can grow.
    pub ceiling: Amount,
    /// Window length, blocks.
    pub window: u64,
    /// Blocks per day, for the clean clock.
    pub blocks_per_day: u64,
    clean_since: u64,
    used: (u64, Amount),
}

/// Why a transfer was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteError {
    /// This window's cap is spent.
    OverCap {
        /// Cap at this height.
        cap: Amount,
        /// Already moved this window.
        used: Amount,
    },
}

impl Route {
    /// A route opened at `height`.
    #[must_use]
    pub fn new(
        base: Amount,
        per_clean_day: Amount,
        ceiling: Amount,
        window: u64,
        blocks_per_day: u64,
        height: u64,
    ) -> Self {
        Self {
            base,
            per_clean_day,
            ceiling,
            window: window.max(1),
            blocks_per_day: blocks_per_day.max(1),
            clean_since: height,
            used: (u64::MAX, Amount::default()),
        }
    }

    /// The cap at `height`.
    #[must_use]
    pub fn cap(&self, height: u64) -> Amount {
        let days = Amount::from_u64(height.saturating_sub(self.clean_since) / self.blocks_per_day);
        self.base
            .saturating_add(self.per_clean_day.checked_mul(days).unwrap_or(Amount::MAX))
            .min(self.ceiling)
    }

    /// Moves `amount` across the route if the window's cap allows.
    ///
    /// # Errors
    ///
    /// [`RouteError::OverCap`] if it does not.
    pub fn send(&mut self, amount: Amount, height: u64) -> Result<(), RouteError> {
        let window = height / self.window;
        let used = if self.used.0 == window {
            self.used.1
        } else {
            Amount::default()
        };
        let cap = self.cap(height);
        // A sum past 2^256 - 1 is over any cap, never a wrapped or clamped one.
        let total = used
            .checked_add(amount)
            .filter(|total| *total <= cap)
            .ok_or(RouteError::OverCap { cap, used })?;
        self.used = (window, total);
        Ok(())
    }

    /// Records an incident: the clean clock restarts, so the cap falls to `base`.
    pub fn incident(&mut self, height: u64) {
        self.clean_since = height;
    }
}
